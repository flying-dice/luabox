//! Control-flow legality over the lowered HIR: `goto`, `::label::`, `break`
//! (#44).
//!
//! Three programs parse fine — they are in the union grammar and legal in the
//! configured edition — yet every reference Lua refuses to *load* them:
//!
//! ```lua
//! local function f() goto nowhere end   -- no visible label      (LB0020)
//! local function g() ::a:: ::a:: end    -- label already defined (LB0021)
//! for i = 1, 3 do end break             -- break outside a loop  (LB0022)
//! ```
//!
//! [`crate::lower`] already resolves every `goto` to the visible label it
//! names (or to `None`), by the same rule the reference parser applies, so
//! this pass reads that resolution rather than re-deriving it. What it adds
//! is the *legality* judgement `lower` deliberately left out.
//!
//! # The rules, as reference Lua implements them (`lparser.c`)
//!
//! - **`goto`** — a label is visible in the block that declares it and every
//!   block nested inside it, within the same function. `leaveblock` hands a
//!   block's unmatched gotos to the enclosing block (`movegotosout`); at the
//!   function's outermost block there is nowhere left to hand them, and
//!   `undefgoto` raises *"no visible label 'x' for <goto>"*. A function
//!   boundary is therefore a hard stop, and so is a sibling block.
//! - **`break`** — Lua 5.4 compiles `break` as `goto break` and gives each
//!   loop body a hidden `break` label, so an unmatched one falls out of the
//!   same `undefgoto` path as *"break outside loop"*; 5.1 walks the block
//!   chain for a breakable block and says *"no loop to break"*. Both stop at
//!   the function boundary, which is why a `break` inside a closure nested in
//!   a loop is an error in every edition.
//! - **duplicate labels** — `checkrepeated` rejects a label whose name is
//!   already *visible*. Which labels count as visible is the one place the
//!   editions differ, and it is the one place this pass is dialect-aware; see
//!   [`repeated_label_scope`].
//!
//! # Deliberately not checked
//!
//! Reference Lua also rejects a forward `goto` that jumps *into* the scope of
//! a local (`goto skip local x = 1 ::skip::` → *"jumps into the scope of
//! local 'x'"*). That rule is a comparison of the *number of active locals*
//! at the goto against the number at the label, with a special case for a
//! label that is the last void statement of its block. The HIR keeps the
//! statement order needed to reconstruct that, but not the notion of a void
//! statement (`;` is erased on lowering), so the reconstruction would be a
//! re-implementation rather than a reading of what lowering already knows —
//! and a wrong one produces false errors on legal code. It is left to the
//! runtime, and recorded in the reference limitations (#44).

use std::collections::HashMap;

use luabox_diag::{Code, Diagnostic, Label as DiagLabel, Span};
use luabox_syntax::Dialect;
use rowan::TextRange;

use crate::file::LoweredFile;
use crate::hir::{Block, Body, BodyId, HirId, LabelId, Stmt, StmtId};

/// `goto` naming a label that is not visible from it.
const UNRESOLVED_GOTO: u16 = 20;
/// A label whose name is already defined in scope.
const DUPLICATE_LABEL: u16 = 21;
/// `break` with no enclosing loop in the same function.
const BREAK_OUTSIDE_LOOP: u16 = 22;

/// Where a repeated `::label::` is looked for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RepeatScope {
    /// Only the block that declares it — Lua 5.2/5.3 and LuaJIT scan from
    /// `fs->bl->firstlabel`, so `::a:: do ::a:: end` is legal there.
    Block,
    /// Every block open in the function — Lua 5.4 scans from
    /// `fs->firstlabel`, so the same program is *"label 'a' already defined"*.
    Function,
}

/// How far back a duplicate `::label::` is searched, per edition.
///
/// Lua 5.4 tightened `checkrepeated`: 5.3 searched `fs->bl->firstlabel` (the
/// current block only), 5.4 searches `fs->firstlabel` (every block still open
/// in the function), which makes a nested label that shadows an outer one an
/// error. Verified against `luac5.4 -p`; the older editions are given the
/// looser rule they shipped, so this pass never rejects what their own
/// compiler accepts.
fn repeated_label_scope(dialect: Dialect) -> RepeatScope {
    match dialect {
        Dialect::Lua54 => RepeatScope::Function,
        _ => RepeatScope::Block,
    }
}

/// Diagnose every control-flow legality violation in `lowered`, in source
/// order.
///
/// `file` names the source in the emitted spans, exactly as
/// `luabox_types::check_file` takes it. `dialect` is the edition the file is
/// written in: `goto`/label checks are skipped for editions without `goto`
/// (Lua 5.1 — the construct is already reported as `LB0010` by dialect
/// legality, and a second complaint about the same tokens is noise), while
/// the `break` check runs for every edition because every edition has
/// `break`.
///
/// Callers should skip this pass on a file with parse errors: a recovered
/// tree can lose the `end` that closed a loop, and a legality verdict over a
/// guessed block structure is worse than none.
#[must_use]
pub fn control_flow(file: &str, lowered: &LoweredFile, dialect: Dialect) -> Vec<Diagnostic> {
    let mut walker = Walker {
        file,
        lowered,
        dialect,
        out: Vec::new(),
    };
    for (id, body) in lowered.bodies() {
        // Each body starts fresh: a function boundary resets the visible
        // labels, the declared ones, and the enclosing-loop context.
        let mut scopes = Scopes::default();
        walker.walk_block(id, body, &body.block, &mut scopes, 0);
    }
    walker
        .out
        .sort_by_key(|diag| diag.primary_label().map_or(0, |l| l.span.range.start));
    walker.out
}

struct Walker<'a> {
    file: &'a str,
    lowered: &'a LoweredFile,
    dialect: Dialect,
    out: Vec<Diagnostic>,
}

/// The two label stacks a walk carries, innermost frame last.
///
/// They are not the same set, and conflating them is the bug that makes
/// `do ::a:: end ::a::` look like a duplicate:
///
/// - `visible` holds *every* label of each enclosing block, collected before
///   the block's statements are walked — a `goto` may jump forwards, so a
///   label further down the block is already a candidate. This is exactly
///   what [`crate::lower`]'s own goto resolution builds.
/// - `declared` holds only the labels *already reached* in each enclosing
///   block. `checkrepeated` runs while the parser is at the second `::a::`,
///   at which point a label in a sibling block has been dropped and one later
///   in an enclosing block does not exist yet.
#[derive(Default)]
struct Scopes {
    visible: Vec<Vec<String>>,
    declared: Vec<HashMap<String, TextRange>>,
}

impl Walker<'_> {
    /// Walk one block; `loops` counts enclosing loop bodies within the same
    /// function.
    fn walk_block(
        &mut self,
        body_id: BodyId,
        body: &Body,
        block: &Block,
        scopes: &mut Scopes,
        loops: usize,
    ) {
        scopes
            .visible
            .push(visible_labels(self.lowered, body, block));
        scopes.declared.push(HashMap::new());

        for &stmt in &block.stmts {
            match body.stmt(stmt) {
                Stmt::Label { label, .. } => self.check_label(*label, scopes),
                Stmt::Goto { name, target } => {
                    self.check_goto(body_id, stmt, name, target.is_some(), scopes);
                }
                Stmt::Break if loops == 0 => {
                    self.report_break(body_id, stmt);
                }
                Stmt::Do { body: inner } => self.walk_block(body_id, body, inner, scopes, loops),
                Stmt::While { body: inner, .. }
                | Stmt::Repeat { body: inner, .. }
                | Stmt::NumericFor { body: inner, .. }
                | Stmt::GenericFor { body: inner, .. } => {
                    self.walk_block(body_id, body, inner, scopes, loops + 1);
                }
                Stmt::If {
                    branches,
                    else_block,
                } => {
                    for branch in branches {
                        self.walk_block(body_id, body, &branch.block, scopes, loops);
                    }
                    if let Some(else_block) = else_block {
                        self.walk_block(body_id, body, else_block, scopes, loops);
                    }
                }
                _ => {}
            }
        }

        scopes.declared.pop();
        scopes.visible.pop();
    }

    /// Report a `::label::` whose name is already defined, and record it.
    fn check_label(&mut self, label: LabelId, scopes: &mut Scopes) {
        if !self.dialect.has_goto() {
            return;
        }
        let label = self.lowered.label(label);
        // An empty name is a parse-recovery artifact, not a label.
        if label.name.is_empty() {
            return;
        }
        let (name, range) = (label.name.clone(), label.range);
        let previous = match repeated_label_scope(self.dialect) {
            RepeatScope::Function => scopes
                .declared
                .iter()
                .rev()
                .find_map(|frame| frame.get(&name))
                .copied(),
            RepeatScope::Block => scopes
                .declared
                .last()
                .and_then(|frame| frame.get(&name))
                .copied(),
        };
        match previous {
            // The first definition stays recorded, so a third occurrence
            // still points back at the one the reader should keep.
            Some(first) => self.report_duplicate(&name, range, first),
            None => {
                if let Some(frame) = scopes.declared.last_mut() {
                    frame.insert(name, range);
                }
            }
        }
    }

    fn check_goto(
        &mut self,
        body_id: BodyId,
        stmt: StmtId,
        name: &str,
        resolved: bool,
        scopes: &Scopes,
    ) {
        if resolved || name.is_empty() || !self.dialect.has_goto() {
            return;
        }
        let id = HirId::stmt(body_id, stmt);
        let range = self
            .lowered
            .goto_name_range(id)
            .or_else(|| self.lowered.source_map().range(id));
        let Some(range) = range else { return };
        let visible: Vec<&str> = scopes
            .visible
            .iter()
            .flat_map(|frame| frame.iter().map(String::as_str))
            .collect();
        let mut diag = Diagnostic::error(
            Code::new(UNRESOLVED_GOTO),
            format!("no visible label `{name}` for `goto`"),
        )
        .with_label(DiagLabel::primary(
            Span::new(self.file, to_range(range)),
            "no matching label is visible here",
        ));
        if let Some(candidate) = closest(name, visible.iter().copied()) {
            diag = diag.with_note(format!("did you mean `{candidate}`?"));
        }
        self.out.push(diag.with_note(
            "a label is visible in the block that declares it and its nested blocks, \
             never across a function boundary",
        ));
    }

    fn report_duplicate(&mut self, name: &str, range: TextRange, first: TextRange) {
        self.out.push(
            Diagnostic::error(
                Code::new(DUPLICATE_LABEL),
                format!("label `{name}` is already defined"),
            )
            .with_label(DiagLabel::primary(
                Span::new(self.file, to_range(range)),
                "duplicate label",
            ))
            .with_label(DiagLabel::secondary(
                Span::new(self.file, to_range(first)),
                "first defined here",
            )),
        );
    }

    fn report_break(&mut self, body_id: BodyId, stmt: StmtId) {
        let Some(range) = self.lowered.source_map().range(HirId::stmt(body_id, stmt)) else {
            return;
        };
        self.out.push(
            Diagnostic::error(Code::new(BREAK_OUTSIDE_LOOP), "`break` outside a loop")
                .with_label(DiagLabel::primary(
                    Span::new(self.file, to_range(range)),
                    "no enclosing loop to break out of",
                ))
                .with_note(
                    "`break` binds to the innermost enclosing `while`/`repeat`/`for` \
                     in the same function; a nested function does not see the loop \
                     around it",
                ),
        );
    }
}

/// Every label declared directly in `block`, in source order — the candidate
/// set a `goto` in this block or any block nested in it can name.
fn visible_labels(lowered: &LoweredFile, body: &Body, block: &Block) -> Vec<String> {
    block
        .stmts
        .iter()
        .filter_map(|&stmt| match body.stmt(stmt) {
            Stmt::Label { label, .. } => Some(lowered.label(*label).name.clone()),
            _ => None,
        })
        .filter(|name| !name.is_empty())
        .collect()
}

fn to_range(range: TextRange) -> std::ops::Range<usize> {
    usize::from(range.start())..usize::from(range.end())
}

/// The closest visible label name within edit distance 1-2, if any.
///
/// A private copy of the nudge `luabox-lint` and `luabox-manifest` each keep
/// privately: they are different bounded contexts (SPEC.md §16) and none of
/// them owns a shared string-distance utility. Ties break on shortest
/// distance, then alphabetically, so the hint is deterministic.
fn closest<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let mut best: Option<(usize, &str)> = None;
    for candidate in candidates {
        let dist = levenshtein(name, candidate);
        if dist == 0 || dist > 2 {
            continue;
        }
        let better = match best {
            None => true,
            Some((best_dist, best_name)) => {
                dist < best_dist || (dist == best_dist && candidate < best_name)
            }
        };
        if better {
            best = Some((dist, candidate));
        }
    }
    best.map(|(_, name)| name)
}

/// Plain Levenshtein edit distance. Label names are Lua identifiers, so
/// byte-wise comparison is exact.
fn levenshtein(a: &str, b: &str) -> usize {
    let a = a.as_bytes();
    let b = b.as_bytes();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "test code — panics document assumptions"
)]
mod tests {
    use luabox_syntax::lua::parse;

    use super::*;
    use crate::lower;

    /// The control-flow codes reported for `source` at `dialect`, in order.
    fn codes(source: &str, dialect: Dialect) -> Vec<u16> {
        let parsed = parse(source, dialect);
        assert!(
            parsed.errors().is_empty(),
            "fixture does not parse cleanly: {:?}",
            parsed.errors()
        );
        let lowered = lower(&parsed);
        control_flow("t.lua", &lowered, dialect)
            .iter()
            .map(|d| d.code.number())
            .collect()
    }

    fn diags(source: &str, dialect: Dialect) -> Vec<Diagnostic> {
        let parsed = parse(source, dialect);
        let lowered = lower(&parsed);
        control_flow("t.lua", &lowered, dialect)
    }

    /// The three programs from #44, each rejected by reference Lua at load
    /// time. Verified against `luac5.4 -p`.
    #[test]
    fn the_three_reported_repros_are_diagnosed() {
        assert_eq!(
            codes("local function f() goto nowhere end", Dialect::Lua54),
            [UNRESOLVED_GOTO]
        );
        assert_eq!(
            codes("local function f() ::a:: ::a:: end", Dialect::Lua54),
            [DUPLICATE_LABEL]
        );
        assert_eq!(
            codes("local x = 1 break", Dialect::Lua54),
            [BREAK_OUTSIDE_LOOP]
        );
        assert_eq!(
            codes("for i = 1, 3 do end break", Dialect::Lua54),
            [BREAK_OUTSIDE_LOOP]
        );
    }

    #[test]
    fn a_goto_reaching_an_enclosing_block_label_is_legal() {
        assert!(codes("do goto a end ::a::", Dialect::Lua54).is_empty());
        assert!(codes("do do goto a end end ::a::", Dialect::Lua54).is_empty());
        assert!(codes("::a:: do goto a end", Dialect::Lua54).is_empty());
        assert!(codes("::top:: goto top", Dialect::Lua54).is_empty());
        assert!(codes("goto done print(1) ::done::", Dialect::Lua54).is_empty());
    }

    /// The `::continue::` idiom — a forward `goto` to a label at the end of a
    /// loop body — is the reason `goto` exists in Lua at all.
    #[test]
    fn the_continue_idiom_is_legal_in_every_loop_kind() {
        for loop_head in [
            "while cond do",
            "repeat",
            "for i = 1, 3 do",
            "for k in pairs(t) do",
        ] {
            let tail = if loop_head == "repeat" {
                "until done"
            } else {
                "end"
            };
            let source =
                format!("{loop_head} if skip then goto continue end work() ::continue:: {tail}");
            assert!(
                codes(&source, Dialect::Lua54).is_empty(),
                "unexpected finding in `{source}`"
            );
        }
    }

    #[test]
    fn a_goto_never_crosses_a_function_boundary() {
        assert_eq!(
            codes("::a:: local f = function() goto a end", Dialect::Lua54),
            [UNRESOLVED_GOTO]
        );
        assert_eq!(
            codes(
                "for i = 1, 3 do local f = function() goto continue end ::continue:: end",
                Dialect::Lua54
            ),
            [UNRESOLVED_GOTO]
        );
    }

    #[test]
    fn a_goto_never_reaches_a_sibling_block() {
        assert_eq!(
            codes("do ::a:: end do goto a end", Dialect::Lua54),
            [UNRESOLVED_GOTO]
        );
        assert_eq!(
            codes("if x then ::a:: else goto a end", Dialect::Lua54),
            [UNRESOLVED_GOTO]
        );
    }

    #[test]
    fn an_unresolved_goto_underlines_its_name_token_only() {
        let found = diags("goto nowhere", Dialect::Lua54);
        let label = found[0].primary_label().unwrap();
        assert_eq!(label.span.range, 5..12, "expected the `nowhere` token");
        assert_eq!(label.span.file, "t.lua");
    }

    #[test]
    fn a_near_miss_label_becomes_a_did_you_mean_note() {
        let found = diags(
            "for i = 1, 3 do goto continu ::continue:: end",
            Dialect::Lua54,
        );
        assert!(
            found[0]
                .notes
                .iter()
                .any(|n| n == "did you mean `continue`?"),
            "no suggestion in {:?}",
            found[0].notes
        );
    }

    #[test]
    fn an_unrelated_label_name_suggests_nothing() {
        let found = diags("goto nowhere ::completely_different::", Dialect::Lua54);
        assert!(
            !found[0].notes.iter().any(|n| n.starts_with("did you mean")),
            "unexpected suggestion in {:?}",
            found[0].notes
        );
    }

    #[test]
    fn labels_of_sibling_and_nested_function_scopes_do_not_collide() {
        assert!(codes("do ::a:: end do ::a:: end", Dialect::Lua54).is_empty());
        assert!(codes("if x then ::a:: else ::a:: end", Dialect::Lua54).is_empty());
        assert!(
            codes(
                "local function f() ::a:: end local function g() ::a:: end",
                Dialect::Lua54
            )
            .is_empty()
        );
    }

    /// A label declared *after* an inner block does not exist yet while that
    /// block is being read — `checkrepeated` runs at the second `::a::`, not
    /// over a pre-collected set. Verified: `luac5.4 -p` accepts this.
    #[test]
    fn a_label_after_a_sibling_block_is_not_a_duplicate() {
        assert!(codes("do ::a:: end ::a::", Dialect::Lua54).is_empty());
        assert!(codes("local function f() ::a:: end ::a::", Dialect::Lua54).is_empty());
    }

    /// Lua 5.4 searches every open block for a repeated label; 5.2/5.3 and
    /// LuaJIT search only the declaring block, so luabox applies each
    /// edition's own rule and never rejects what its compiler accepts.
    #[test]
    fn shadowing_a_label_in_a_nested_block_is_a_5_4_only_error() {
        assert_eq!(
            codes("::a:: do ::a:: end", Dialect::Lua54),
            [DUPLICATE_LABEL]
        );
        for dialect in [Dialect::Lua52, Dialect::Lua53, Dialect::LuaJit] {
            assert!(
                codes("::a:: do ::a:: end", dialect).is_empty(),
                "unexpected finding under {dialect:?}"
            );
        }
        // The same-block form is an error in every edition with `goto`.
        for dialect in [
            Dialect::Lua52,
            Dialect::Lua53,
            Dialect::Lua54,
            Dialect::LuaJit,
        ] {
            assert_eq!(
                codes("::a:: ::a::", dialect),
                [DUPLICATE_LABEL],
                "missing finding under {dialect:?}"
            );
        }
    }

    #[test]
    fn a_duplicate_label_points_at_the_second_and_names_the_first() {
        let found = diags("::a:: ::b:: ::a::", Dialect::Lua54);
        assert_eq!(found.len(), 1);
        let primary = found[0].primary_label().unwrap();
        assert_eq!(primary.span.range, 14..15, "expected the third `a`");
        let secondary = found[0].labels.iter().find(|l| !l.primary).unwrap();
        assert_eq!(secondary.span.range, 2..3, "expected the first `a`");
        assert_eq!(secondary.message, "first defined here");
    }

    #[test]
    fn break_is_legal_in_every_loop_kind_and_through_plain_blocks() {
        for source in [
            "while cond do break end",
            "repeat break until done",
            "for i = 1, 3 do break end",
            "for k in pairs(t) do break end",
            "while cond do do break end end",
            "while cond do if x then break end end",
            "for i = 1, 3 do while cond do break end break end",
        ] {
            assert!(
                codes(source, Dialect::Lua54).is_empty(),
                "unexpected finding in `{source}`"
            );
        }
    }

    #[test]
    fn break_never_sees_the_loop_around_an_enclosing_function() {
        assert_eq!(
            codes(
                "while cond do local f = function() break end end",
                Dialect::Lua54
            ),
            [BREAK_OUTSIDE_LOOP]
        );
        assert_eq!(
            codes(
                "for i = 1, 3 do local f = function() do break end end end",
                Dialect::Lua54
            ),
            [BREAK_OUTSIDE_LOOP]
        );
        // …and a loop inside that function makes it legal again.
        assert!(
            codes(
                "while cond do local f = function() while c do break end end end",
                Dialect::Lua54
            )
            .is_empty()
        );
    }

    /// `break` exists in every edition, `goto` does not: under 5.1 the break
    /// check still runs while the goto/label checks stand down (the construct
    /// is already `LB0010` from dialect legality).
    #[test]
    fn the_break_check_runs_in_every_edition_and_goto_checks_do_not() {
        assert_eq!(
            codes("local x = 1 break", Dialect::Lua51),
            [BREAK_OUTSIDE_LOOP]
        );
        assert!(codes("while cond do break end", Dialect::Lua51).is_empty());
        // `goto`/`::label::` parse into the union grammar even under 5.1.
        let parsed = parse("::a:: ::a::", Dialect::Lua54);
        let lowered = lower(&parsed);
        assert!(control_flow("t.lua", &lowered, Dialect::Lua51).is_empty());
    }

    #[test]
    fn findings_come_back_in_source_order() {
        let found = diags("break ::a:: ::a:: goto nowhere", Dialect::Lua54);
        let starts: Vec<usize> = found
            .iter()
            .map(|d| d.primary_label().unwrap().span.range.start)
            .collect();
        let mut sorted = starts.clone();
        sorted.sort_unstable();
        assert_eq!(starts, sorted);
        assert_eq!(found.len(), 3);
    }

    /// A recovered parse can leave a `goto`/label with no name token; that is
    /// a syntax error, not a legality one, so nothing is reported for it.
    #[test]
    fn a_nameless_goto_or_label_from_a_broken_parse_reports_nothing() {
        for source in ["goto", "::::", "::"] {
            let parsed = parse(source, Dialect::Lua54);
            let lowered = lower(&parsed);
            let found = control_flow("t.lua", &lowered, Dialect::Lua54);
            assert!(
                found.is_empty(),
                "unexpected finding in `{source}`: {found:?}"
            );
        }
    }

    /// The rule reference Lua has and this pass deliberately does not: a
    /// forward `goto` into the scope of a local. Pinned so the deviation is a
    /// decision on record rather than an accident (see the module docs).
    #[test]
    fn a_goto_jumping_into_a_local_scope_is_knowingly_accepted() {
        assert!(codes("goto skip local x = 1 ::skip:: print(x)", Dialect::Lua54).is_empty());
    }

    #[test]
    fn levenshtein_basics() {
        assert_eq!(levenshtein("continue", "continue"), 0);
        assert_eq!(levenshtein("continu", "continue"), 1);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
    }

    #[test]
    fn closest_skips_exact_matches_and_breaks_ties_alphabetically() {
        assert_eq!(closest("top", ["top"]), None);
        assert_eq!(closest("top", ["nowhere", "bottom"]), None);
        // Both distance 1; the alphabetically smaller wins, deterministically.
        assert_eq!(closest("tob", ["tox", "toa"]), Some("toa"));
    }
}
