//! `concat-in-loop` (perf): a loop-carried accumulator grown with `..`.

use std::ops::Range;

use luabox_diag::Code;
use luabox_hir::{BinOp, Body, BodyId, Expr, ExprId, HirId, Stmt};

use crate::context::{LintContext, binding_of};
use crate::diagnostic::LintDiagnostic;
use crate::rule::{Rule, Tier};

/// `s = s .. expr` where `s` is declared outside the enclosing loop (SPEC.md
/// §9): quadratic. Suggests `table.concat`; no autofix (the rewrite is
/// context-dependent).
pub struct ConcatInLoop;

impl Rule for ConcatInLoop {
    fn id(&self) -> &'static str {
        "concat-in-loop"
    }

    fn tier(&self) -> Tier {
        Tier::Perf
    }

    fn code(&self) -> Code {
        Code::new(506)
    }

    fn description(&self) -> &'static str {
        "string concatenation of a loop-carried variable is quadratic"
    }

    fn check(&self, ctx: &LintContext<'_>) -> Vec<LintDiagnostic> {
        let mut out = Vec::new();
        for (body_id, body) in ctx.lowered.bodies() {
            let loops = loop_ranges(ctx, body_id, body);
            if loops.is_empty() {
                continue;
            }
            for (stmt_id, stmt) in body.stmts() {
                let Stmt::Assign { targets, values } = stmt else {
                    continue;
                };
                if targets.len() != 1 || values.len() != 1 {
                    continue;
                }
                let (target, value) = (targets[0], values[0]);
                if !matches!(body.expr(target), Expr::Name(_)) {
                    continue;
                }
                let Some(binding) =
                    binding_of(ctx.lowered.resolution(HirId::expr(body_id, target)))
                else {
                    continue;
                };
                let Expr::Binary {
                    op: BinOp::Concat,
                    lhs,
                    rhs,
                } = *body.expr(value)
                else {
                    continue;
                };
                let is_self = |e: ExprId| {
                    matches!(body.expr(e), Expr::Name(_))
                        && binding_of(ctx.lowered.resolution(HirId::expr(body_id, e)))
                            == Some(binding)
                };
                if !is_self(lhs) && !is_self(rhs) {
                    continue;
                }
                let Some(assign_range) = ctx.node_range(HirId::stmt(body_id, stmt_id)) else {
                    continue;
                };
                // Attribute the assignment to its innermost enclosing loop.
                let Some(inner) = loops
                    .iter()
                    .filter(|lr| lr.start <= assign_range.start && assign_range.end <= lr.end)
                    .min_by_key(|lr| lr.end - lr.start)
                else {
                    continue;
                };
                // Only flag when the accumulator lives outside that loop.
                let decl = &ctx.lowered.binding(binding).range;
                let decl_start = usize::from(decl.start());
                let decl_end = usize::from(decl.end());
                let declared_inside = inner.start <= decl_start && decl_end <= inner.end;
                if declared_inside {
                    continue;
                }
                out.push(
                    LintDiagnostic::new(
                        assign_range,
                        "string built with `..` in a loop is quadratic",
                    )
                    .with_note("collect the pieces in a table and join once with `table.concat`"),
                );
            }
        }
        out
    }
}

/// The source ranges of every loop statement in `body`.
fn loop_ranges(ctx: &LintContext<'_>, body_id: BodyId, body: &Body) -> Vec<Range<usize>> {
    body.stmts()
        .filter(|(_, stmt)| {
            matches!(
                stmt,
                Stmt::While { .. }
                    | Stmt::Repeat { .. }
                    | Stmt::NumericFor { .. }
                    | Stmt::GenericFor { .. }
            )
        })
        .filter_map(|(id, _)| ctx.node_range(HirId::stmt(body_id, id)))
        .collect()
}
