//! `pairs-on-array` (perf): `pairs(t)` where `t`'s shape is array-only.

use luabox_diag::Code;
use luabox_hir::{Expr, HirId, Resolution, TableEntry};

use crate::context::{LintContext, binding_of};
use crate::diagnostic::{Fix, LintDiagnostic};
use crate::rule::{Rule, Tier};

/// `pairs(t)` where `t` is an array (declared `T[]`/`table<integer, V>`, or a
/// positional-only table literal) — `ipairs` iterates in order without hashing
/// (SPEC.md §9). Fixes by rewriting `pairs` to `ipairs`.
pub struct PairsOnArray;

impl Rule for PairsOnArray {
    fn id(&self) -> &'static str {
        "pairs-on-array"
    }

    fn tier(&self) -> Tier {
        Tier::Perf
    }

    fn code(&self) -> Code {
        Code::new(507)
    }

    fn description(&self) -> &'static str {
        "`pairs` over an array should be `ipairs`"
    }

    fn check(&self, ctx: &LintContext<'_>) -> Vec<LintDiagnostic> {
        let mut out = Vec::new();
        for (body_id, body) in ctx.lowered.bodies() {
            for (_, expr) in body.exprs() {
                let Expr::Call { callee, args } = expr else {
                    continue;
                };
                if !matches!(body.expr(*callee), Expr::Name(n) if n == "pairs") {
                    continue;
                }
                // Must be the real global `pairs`, not a shadowing local.
                if !matches!(
                    ctx.lowered.resolution(HirId::expr(body_id, *callee)),
                    Some(Resolution::Global(name)) if name == "pairs"
                ) {
                    continue;
                }
                if args.len() != 1 {
                    continue;
                }
                let arg = args[0];
                let is_array = match body.expr(arg) {
                    Expr::Table { entries } => {
                        !entries.is_empty()
                            && entries
                                .iter()
                                .all(|e| matches!(e, TableEntry::Positional(_)))
                    }
                    Expr::Name(_) => binding_of(ctx.lowered.resolution(HirId::expr(body_id, arg)))
                        .is_some_and(|b| ctx.facts.is_array(b)),
                    _ => false,
                };
                if !is_array {
                    continue;
                }
                let Some(callee_range) = ctx.node_range(HirId::expr(body_id, *callee)) else {
                    continue;
                };
                out.push(
                    LintDiagnostic::new(
                        callee_range.clone(),
                        "`pairs` over an array hashes keys and loses order; use `ipairs`",
                    )
                    .with_fix(Fix::machine(callee_range, "ipairs")),
                );
            }
        }
        out
    }
}
