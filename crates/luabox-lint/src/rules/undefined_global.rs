//! `undefined-global` (suspicious): a global *read* with no known
//! declaration — luals' `undefined-global` finding (SPEC.md §9, ticket #103).

use std::collections::HashSet;

use luabox_diag::Code;
use luabox_hir::{Expr, HirId, Resolution, Stmt};

use crate::context::LintContext;
use crate::diagnostic::LintDiagnostic;
use crate::rule::{Rule, Tier};

/// A name reference resolving to [`Resolution::Global`] that is not:
///
/// - the bare-name target of an assignment (that's a *write* — `global-write`
///   LB0504's business, not this rule's);
/// - a name this same file assigns anywhere, by the same bare-name-target
///   test (a file that does `foo = 1` and later reads `foo` is
///   self-defining);
/// - a known global — the dialect stdlib or an ambient `[types] defs`
///   package (`ctx.known_globals`, built by the Frontend the way `luabox
///   check` builds its `Ambient` layer);
/// - on the `[lint] globals` allow-list (`ctx.config.is_allowed_global`).
///
/// Silent for the whole file when it is a `---@meta` definition file
/// (ticket #76): declaring globals — `love = {}` and the like — is such a
/// file's entire purpose, exactly as for `global-write`.
pub struct UndefinedGlobal;

impl Rule for UndefinedGlobal {
    fn id(&self) -> &'static str {
        "undefined-global"
    }

    fn tier(&self) -> Tier {
        Tier::Suspicious
    }

    fn code(&self) -> Code {
        Code::new(509)
    }

    fn description(&self) -> &'static str {
        "read of a global with no known declaration"
    }

    fn check(&self, ctx: &LintContext<'_>) -> Vec<LintDiagnostic> {
        if ctx.facts.is_meta() {
            return Vec::new();
        }

        // Pass 1: this file's own global write-targets (mirrors
        // `global_write`'s detection exactly) — bare-name assign targets
        // resolving to a global. Collected up front so pass 2 can both
        // exclude these positions from being read-checked and treat their
        // names as self-defined.
        let mut write_targets: HashSet<HirId> = HashSet::new();
        let mut file_globals: HashSet<&str> = HashSet::new();
        for (body_id, body) in ctx.lowered.bodies() {
            for (_, stmt) in body.stmts() {
                let Stmt::Assign { targets, .. } = stmt else {
                    continue;
                };
                for &target in targets {
                    if !matches!(body.expr(target), Expr::Name(_)) {
                        continue;
                    }
                    let hir = HirId::expr(body_id, target);
                    write_targets.insert(hir);
                    if let Some(Resolution::Global(name)) = ctx.lowered.resolution(hir) {
                        file_globals.insert(name.as_str());
                    }
                }
            }
        }

        // Pass 2: every global-resolved name reference that isn't itself a
        // write target from pass 1 is a read.
        let mut out = Vec::new();
        for (body_id, body) in ctx.lowered.bodies() {
            for (expr_id, expr) in body.exprs() {
                if !matches!(expr, Expr::Name(_)) {
                    continue;
                }
                let hir = HirId::expr(body_id, expr_id);
                if write_targets.contains(&hir) {
                    continue;
                }
                let Some(Resolution::Global(name)) = ctx.lowered.resolution(hir) else {
                    continue;
                };
                if ctx.known_globals.contains(name)
                    || file_globals.contains(name.as_str())
                    || ctx.config.is_allowed_global(name)
                {
                    continue;
                }
                let Some(range) = ctx.node_range(hir) else {
                    continue;
                };
                let mut diag =
                    LintDiagnostic::new(range, format!("read of undefined global `{name}`"));
                if let Some(candidate) = did_you_mean(name, ctx.known_globals, &file_globals) {
                    diag = diag.with_note(format!("did you mean `{candidate}`?"));
                }
                diag = diag.with_note(format!(
                    "a typo? declare it via a defs package, `[lint] globals = [\"{name}\"]`, or assign it"
                ));
                out.push(diag);
            }
        }
        out
    }
}

/// The closest known-global name within edit distance 1-2, if any — the
/// "did you mean" hint (ticket #103's `prnit` → `print` example). Ties are
/// broken by shortest distance, then alphabetically, for determinism.
fn did_you_mean(
    name: &str,
    known: &HashSet<String>,
    file_globals: &HashSet<&str>,
) -> Option<String> {
    let mut best: Option<(usize, &str)> = None;
    for candidate in known
        .iter()
        .map(String::as_str)
        .chain(file_globals.iter().copied())
    {
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
    best.map(|(_, name)| name.to_owned())
}

/// Plain Levenshtein edit distance (insert/delete/substitute). Identifiers
/// are ASCII, so byte-wise comparison is exact — no need for Unicode
/// grapheme handling.
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
