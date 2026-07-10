//! `explicit-nil-compare-truthiness` (style): `x ~= nil` / `x == nil` used as a
//! whole `if` condition where plain truthiness is provably equivalent.

use luabox_diag::Code;
use luabox_hir::{BinOp, Expr, HirId, Literal, Stmt};

use crate::context::{LintContext, binding_of};
use crate::diagnostic::{Fix, LintDiagnostic};
use crate::rule::{Rule, Tier};

/// Rewrites a nil comparison used as an `if`/`elseif` condition into plain
/// truthiness, but only when type information proves the value can never be
/// `false` (SPEC.md §9). Unknown types are skipped, so the rewrite is always
/// behaviour-preserving. Fixes both directions.
pub struct NilCompare;

impl Rule for NilCompare {
    fn id(&self) -> &'static str {
        "explicit-nil-compare-truthiness"
    }

    fn tier(&self) -> Tier {
        Tier::Style
    }

    fn code(&self) -> Code {
        Code::new(505)
    }

    fn description(&self) -> &'static str {
        "a nil comparison is equivalent to plain truthiness here"
    }

    fn check(&self, ctx: &LintContext<'_>) -> Vec<LintDiagnostic> {
        let mut out = Vec::new();
        for (body_id, body) in ctx.lowered.bodies() {
            for (_, stmt) in body.stmts() {
                let Stmt::If { branches, .. } = stmt else {
                    continue;
                };
                for branch in branches {
                    if let Some(diag) = check_cond(ctx, body_id, branch.cond) {
                        out.push(diag);
                    }
                }
            }
        }
        out
    }
}

fn check_cond(
    ctx: &LintContext<'_>,
    body_id: luabox_hir::BodyId,
    cond: luabox_hir::ExprId,
) -> Option<LintDiagnostic> {
    let body = ctx.lowered.body(body_id);
    let Expr::Binary { op, lhs, rhs } = *body.expr(cond) else {
        return None;
    };
    let is_eq = match op {
        BinOp::Eq => true,
        BinOp::Ne => false,
        _ => return None,
    };
    let lhs_nil = matches!(body.expr(lhs), Expr::Literal(Literal::Nil));
    let rhs_nil = matches!(body.expr(rhs), Expr::Literal(Literal::Nil));
    let var = match (lhs_nil, rhs_nil) {
        (true, false) => rhs,
        (false, true) => lhs,
        _ => return None,
    };
    if !matches!(body.expr(var), Expr::Name(_)) {
        return None;
    }
    let binding = binding_of(ctx.lowered.resolution(HirId::expr(body_id, var)))?;
    if !ctx.facts.excludes_false(binding) {
        return None;
    }

    let var_range = ctx.node_range(HirId::expr(body_id, var))?;
    let cond_range = ctx.node_range(HirId::expr(body_id, cond))?;
    let var_text = ctx.text(&var_range).to_owned();
    let (replacement, message) = if is_eq {
        (
            format!("not {var_text}"),
            format!("`{var_text} == nil` is equivalent to `not {var_text}` here"),
        )
    } else {
        (
            var_text.clone(),
            format!("`{var_text} ~= nil` is equivalent to `{var_text}` here"),
        )
    };
    Some(
        LintDiagnostic::new(cond_range.clone(), message)
            .with_fix(Fix::machine(cond_range, replacement)),
    )
}
