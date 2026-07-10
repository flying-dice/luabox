//! Rule: `goto`/labels (5.2+/LuaJIT) → Lua 5.1 control-flow restructure
//! (SPEC.md §2.1: "loop/flag rewrite; error if irreducible").
//!
//! # The algorithm
//!
//! Every `goto` is resolved to its label per Lua's own visibility rule
//! (innermost enclosing block outward, stopping at function boundaries).
//! Call `B` the block that directly contains the label. A `goto` is
//! *anchored* in `B` either as a direct child statement, or as the last
//! statement of an `if`/`elseif`/`else` branch whose `if` statement is a
//! direct child of `B`. Exactly two reducible shapes are rewritten;
//! anything else is a hard `LB0601`:
//!
//! **Backward goto as loop** — the label precedes its single goto in `B`.
//!
//! - Conditional (`if <cond> then goto L end`, a plain `if` with no
//!   `elseif`/`else` and the goto as the branch's only statement):
//!   `::L::` → `repeat` and the whole `if` → `until not (<cond>)`.
//!   *Semantics*: both forms execute the statements between label and
//!   `if`, then evaluate `<cond>` once per iteration in the same scope —
//!   Lua's `repeat` scope extends over its `until` expression exactly as
//!   the original condition saw the block's locals — looping while the
//!   condition holds and falling through to the statement after the `if`
//!   otherwise. Locals in the body are re-declared per iteration in both
//!   forms. No flag is needed: the `until` condition *is* the goto
//!   condition, evaluated at the same point in the same order.
//! - Unconditional (`goto L` as the last statement of `B`):
//!   `::L::` → `while true do` and `goto L` → `end` — a literal infinite
//!   loop over the same region.
//!
//! In both backward shapes, a `break` in the region that is not enclosed
//! by an inner loop would re-bind from an enclosing loop to the new
//! wrapper, so it is `LB0601`.
//!
//! **Forward goto as skip** — the label follows the goto(s) in `B`.
//! Each goto gets a fresh flag: `local __luabox_skip_n = false` is
//! inserted before the anchor, the `goto` becomes `__luabox_skip_n =
//! true`, `if not __luabox_skip_n then` opens right after the anchor, and
//! the label becomes the matching `end`(s).
//! *Semantics*: the flag is `true` exactly when the original program
//! counter would have jumped; because the goto is the *last* statement of
//! its branch, setting the flag and falling out of the `if` reaches the
//! skip-wrapper immediately, so the wrapped region executes iff the goto
//! was not taken. A `break`/`return` inside the region keeps its meaning
//! (an `if` is not a loop boundary). Locals declared in the skipped
//! region become scoped to the wrapper — but jumping *into* the scope of
//! a local is already illegal Lua, so no valid source can observe them
//! after the label. Multiple forward gotos to one label are supported
//! (the common `goto continue` idiom); wrappers nest by processing gotos
//! with the farther label first, and the label emits one `end` per goto.
//!
//! Rewrite regions in the same block must be strictly nested or disjoint
//! (shared labels/anchors between forward rewrites excepted) — interleaved
//! regions are `LB0601`.
//!
//! Unreferenced labels are simply deleted (a label has no effect of its
//! own, and `::L::` is not 5.1 syntax).
//!
//! The `---@luabox-allow lossy-lowering` escape hatch does **not** apply
//! here: an irreducible goto is a control-flow error, not a fidelity
//! trade-off, so `LB0601` stands regardless of annotations (SPEC.md §2.1
//! reserves the hatch for constructs like `<close>` fidelity).

use std::collections::HashMap;

use luabox_syntax::lua::ast::{AstNode, GotoStmt, IfStmt, LabelStmt};
use luabox_syntax::lua::{SyntaxKind, SyntaxNode};

use crate::diag::{self, LowerDiagnostic};
use crate::edit::{self, Edit};
use crate::{Ctx, rewrite};

/// One planned rewrite: a goto with its anchor statement in the label's
/// block, plus the statement-index interval it spans.
struct Plan {
    goto_node: SyntaxNode,
    /// The direct child of the label's block the goto is anchored at: the
    /// goto itself or its enclosing `if`.
    anchor: SyntaxNode,
    /// Anchored via an `if`/`elseif`/`else` branch (vs a direct child).
    branch: bool,
    label: SyntaxNode,
    /// The label's block.
    block: SyntaxNode,
    /// Statement indices in `block`: `min`..`max` covers anchor and label.
    min: usize,
    max: usize,
    backward: bool,
}

pub(crate) fn run(root: &SyntaxNode, ctx: &mut Ctx<'_>, edits: &mut Vec<Edit>) {
    if !ctx.gotos {
        return;
    }

    // Group gotos by resolved label.
    let mut by_label: HashMap<SyntaxNode, Vec<SyntaxNode>> = HashMap::new();
    for goto_node in root
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::GOTO_STMT)
    {
        match resolve_label(&goto_node) {
            Some(label) => by_label.entry(label).or_default().push(goto_node),
            None => ctx.diags.push(LowerDiagnostic::error(
                diag::IRREDUCIBLE_GOTO,
                "irreducible `goto`: no visible matching label".to_owned(),
                goto_node.text_range(),
            )),
        }
    }

    // Classify per label, in source order.
    let mut plans: Vec<Plan> = Vec::new();
    for label in root
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::LABEL_STMT)
    {
        let Some(gotos) = by_label.remove(&label) else {
            // Unreferenced label: not 5.1 syntax, no effect — delete it.
            edit::push(edits, label.text_range(), String::new());
            continue;
        };
        classify_label(&label, gotos, ctx, &mut plans);
    }

    // Regions in one block must be strictly nested or disjoint.
    check_nesting(&plans, ctx);

    // Forward rewrites: same-anchor gotos must open their wrappers
    // outermost-label-first so the `end`s at nearer labels close the right
    // `if`s. Sorting by (anchor position, label index descending, goto
    // position) achieves that and is deterministic.
    let mut forwards: Vec<&Plan> = plans.iter().filter(|p| !p.backward).collect();
    forwards.sort_by_key(|p| {
        (
            p.anchor.text_range().start(),
            std::cmp::Reverse(p.max),
            p.goto_node.text_range().start(),
        )
    });
    let mut ends_per_label: HashMap<SyntaxNode, usize> = HashMap::new();
    for plan in forwards {
        rewrite_forward(plan, ctx, edits);
        *ends_per_label.entry(plan.label.clone()).or_default() += 1;
    }
    for (label, count) in ends_per_label {
        let indent = ctx.indent_at(label.text_range().start());
        let text = std::iter::repeat_n("end", count)
            .collect::<Vec<_>>()
            .join(&format!("\n{indent}"));
        edit::push(edits, label.text_range(), text);
    }

    for plan in plans.iter().filter(|p| p.backward) {
        rewrite_backward(plan, ctx, edits);
    }

    // Publish intervals for the `<close>` nesting check (attribs rule).
    for plan in &plans {
        ctx.goto_intervals
            .entry(plan.block.clone())
            .or_default()
            .push((plan.min, plan.max));
    }
}

/// Resolve a goto's label per Lua visibility: innermost enclosing block
/// outward, stopping at the enclosing function. (Also used by the
/// `<close>` rule to detect gotos crossing the scope-tail boundary.)
pub(crate) fn resolve_label(goto_node: &SyntaxNode) -> Option<SyntaxNode> {
    let name = GotoStmt::cast(goto_node.clone())?.label()?;
    let name = name.text().to_owned();
    let mut current = goto_node.parent();
    while let Some(node) = current {
        if node.kind() == SyntaxKind::BLOCK
            && let Some(label) = node.children().find(|c| {
                c.kind() == SyntaxKind::LABEL_STMT
                    && LabelStmt::cast(c.clone())
                        .and_then(|l| l.name())
                        .is_some_and(|t| t.text() == name)
            })
        {
            return Some(label);
        }
        if matches!(
            node.kind(),
            SyntaxKind::FUNCTION_EXPR
                | SyntaxKind::FUNCTION_DECL_STMT
                | SyntaxKind::LOCAL_FUNCTION_STMT
        ) {
            return None;
        }
        current = node.parent();
    }
    None
}

/// Classify one label's gotos into plans, or report `LB0601`.
fn classify_label(
    label: &SyntaxNode,
    gotos: Vec<SyntaxNode>,
    ctx: &mut Ctx<'_>,
    plans: &mut Vec<Plan>,
) {
    let Some(block) = label.parent() else {
        return;
    };
    let stmts: Vec<SyntaxNode> = block.children().collect();
    let Some(label_idx) = stmts.iter().position(|s| s == label) else {
        return;
    };

    let mut label_plans = Vec::new();
    for goto_node in gotos {
        let Some((anchor, branch)) = anchor_in(&goto_node, &block) else {
            ctx.diags.push(irreducible(
                &goto_node,
                "the goto is nested deeper than an `if` branch directly in the label's block",
            ));
            return;
        };
        let Some(anchor_idx) = stmts.iter().position(|s| *s == anchor) else {
            return;
        };
        let backward = anchor_idx > label_idx;
        label_plans.push(Plan {
            goto_node,
            anchor,
            branch,
            label: label.clone(),
            block: block.clone(),
            min: anchor_idx.min(label_idx),
            max: anchor_idx.max(label_idx),
            backward,
        });
    }

    let backwards = label_plans.iter().filter(|p| p.backward).count();
    if backwards > 0 && (backwards > 1 || label_plans.len() > 1) {
        for plan in label_plans.iter().filter(|p| p.backward) {
            ctx.diags.push(irreducible(
                &plan.goto_node,
                "a backward goto must be the label's only goto to rewrite as a loop",
            ));
        }
        return;
    }
    if backwards == 1 {
        let plan = &label_plans[0];
        if let Some(reason) = backward_shape_error(plan, &stmts) {
            ctx.diags.push(irreducible(&plan.goto_node, reason));
            return;
        }
        check_region_breaks(&stmts[plan.min + 1..plan.max], ctx);
    }
    plans.append(&mut label_plans);
}

/// The direct child of `block` a goto is anchored at: the goto itself, or
/// the `if` whose branch ends with the goto.
fn anchor_in(goto_node: &SyntaxNode, block: &SyntaxNode) -> Option<(SyntaxNode, bool)> {
    let parent = goto_node.parent()?;
    if parent == *block {
        return Some((goto_node.clone(), false));
    }
    if parent.kind() != SyntaxKind::BLOCK {
        return None;
    }
    // The goto must be the last statement of its branch: everything after
    // it in the branch would run in the rewrite but was unreachable (or
    // ill-formed) in the original.
    if parent.children().last()? != *goto_node {
        return None;
    }
    let owner = parent.parent()?;
    let if_stmt = match owner.kind() {
        SyntaxKind::IF_STMT => owner,
        SyntaxKind::ELSEIF_CLAUSE | SyntaxKind::ELSE_CLAUSE => owner.parent()?,
        _ => return None,
    };
    if if_stmt.kind() != SyntaxKind::IF_STMT || if_stmt.parent()? != *block {
        return None;
    }
    Some((if_stmt, true))
}

/// Why a backward plan does not fit the two loop shapes, if it doesn't.
fn backward_shape_error(plan: &Plan, stmts: &[SyntaxNode]) -> Option<&'static str> {
    if plan.branch {
        let if_stmt = IfStmt::cast(plan.anchor.clone())?;
        if if_stmt.elseif_clauses().next().is_some() || if_stmt.else_clause().is_some() {
            return Some(
                "a backward goto's `if` must have no `elseif`/`else` to rewrite as repeat/until",
            );
        }
        let branch_stmts = plan.goto_node.parent().map(|p| p.children().count());
        if branch_stmts != Some(1) {
            return Some(
                "a backward goto must be its `if` branch's only statement to rewrite as \
                 repeat/until",
            );
        }
        if_stmt.condition()?;
        None
    } else if stmts.last() == Some(&plan.anchor) {
        None
    } else {
        Some(
            "an unconditional backward goto must be its block's last statement to rewrite as \
             `while true`",
        )
    }
}

/// A `break` in a to-be-wrapped backward region that is not enclosed by an
/// inner loop would re-bind to the new wrapper — `LB0601`.
fn check_region_breaks(region: &[SyntaxNode], ctx: &mut Ctx<'_>) {
    fn scan(node: &SyntaxNode, ctx: &mut Ctx<'_>) {
        for child in node.children() {
            match child.kind() {
                SyntaxKind::BREAK_STMT => ctx.diags.push(irreducible(
                    &child,
                    "a `break` inside the region would re-bind to the loop the rewrite inserts",
                )),
                SyntaxKind::WHILE_STMT
                | SyntaxKind::REPEAT_STMT
                | SyntaxKind::NUMERIC_FOR_STMT
                | SyntaxKind::GENERIC_FOR_STMT
                | SyntaxKind::FUNCTION_EXPR
                | SyntaxKind::FUNCTION_DECL_STMT
                | SyntaxKind::LOCAL_FUNCTION_STMT => {}
                _ => scan(&child, ctx),
            }
        }
    }
    for stmt in region {
        if matches!(
            stmt.kind(),
            SyntaxKind::WHILE_STMT
                | SyntaxKind::REPEAT_STMT
                | SyntaxKind::NUMERIC_FOR_STMT
                | SyntaxKind::GENERIC_FOR_STMT
        ) {
            continue;
        }
        scan(stmt, ctx);
    }
}

/// Regions in the same block must be strictly nested or disjoint. Forward
/// rewrites may share a label or an anchor with each other (handled by end
/// stacking / open ordering); any sharing with a backward region is
/// irreducible.
fn check_nesting(plans: &[Plan], ctx: &mut Ctx<'_>) {
    for (i, a) in plans.iter().enumerate() {
        for b in &plans[i + 1..] {
            if a.block != b.block || compatible(a, b) {
                continue;
            }
            ctx.diags.push(irreducible(
                &b.goto_node,
                "goto regions interleave; rewrites cannot nest",
            ));
        }
    }
}

fn compatible(a: &Plan, b: &Plan) -> bool {
    if a.backward || b.backward {
        // Strictly disjoint or strictly nested.
        let (outer, inner) = if a.min <= b.min { (a, b) } else { (b, a) };
        outer.max < inner.min || (outer.min < inner.min && inner.max < outer.max)
    } else {
        // Forward pair: disjoint or nested; shared endpoints are the same
        // statement (same label / same anchor) and are supported.
        a.max <= b.min
            || b.max <= a.min
            || (a.min <= b.min && b.max <= a.max)
            || (b.min <= a.min && a.max <= b.max)
    }
}

fn irreducible(goto_node: &SyntaxNode, reason: &str) -> LowerDiagnostic {
    LowerDiagnostic::error(
        diag::IRREDUCIBLE_GOTO,
        format!(
            "irreducible `goto` for a Lua 5.1 target: {reason}; restructure the control flow \
             by hand (`---@luabox-allow lossy-lowering` does not apply to goto)"
        ),
        goto_node.text_range(),
    )
}

/// Emit the flag-and-skip edits for one forward goto.
fn rewrite_forward(plan: &Plan, ctx: &mut Ctx<'_>, edits: &mut Vec<Edit>) {
    let flag = ctx.fresh_skip_flag();
    let indent = ctx.indent_at(plan.anchor.text_range().start());
    if plan.branch {
        edit::insert(
            edits,
            plan.anchor.text_range().start(),
            format!("local {flag} = false\n{indent}"),
        );
        edit::push(edits, plan.goto_node.text_range(), format!("{flag} = true"));
    } else {
        edit::push(
            edits,
            plan.goto_node.text_range(),
            format!("local {flag} = true"),
        );
    }
    edit::insert(
        edits,
        plan.anchor.text_range().end(),
        format!("\n{indent}if not {flag} then"),
    );
}

/// Emit the loop edits for the single backward goto of a label.
fn rewrite_backward(plan: &Plan, ctx: &mut Ctx<'_>, edits: &mut Vec<Edit>) {
    if plan.branch {
        let cond = IfStmt::cast(plan.anchor.clone())
            .and_then(|i| i.condition())
            .map_or_else(
                || "true".to_owned(),
                |c| rewrite::render_or_text(c.syntax(), ctx),
            );
        edit::push(edits, plan.label.text_range(), "repeat".to_owned());
        edit::push(
            edits,
            plan.anchor.text_range(),
            format!("until not ({cond})"),
        );
        ctx.replaced.push(plan.anchor.text_range());
    } else {
        edit::push(edits, plan.label.text_range(), "while true do".to_owned());
        edit::push(edits, plan.goto_node.text_range(), "end".to_owned());
    }
}
