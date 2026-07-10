//! `global-write` (suspicious): assignment to an unresolved global.

use luabox_diag::Code;
use luabox_hir::{Expr, HirId, Resolution, Stmt};

use crate::context::LintContext;
use crate::diagnostic::LintDiagnostic;
use crate::rule::{Rule, Tier};

/// An assignment whose bare-name target resolves to a global — the classic
/// "forgot `local`" footgun (SPEC.md §9). Field writes (`t.x = v`) are not
/// flagged; the allow-list comes from `[lint] globals`.
pub struct GlobalWrite;

impl Rule for GlobalWrite {
    fn id(&self) -> &'static str {
        "global-write"
    }

    fn tier(&self) -> Tier {
        Tier::Suspicious
    }

    fn code(&self) -> Code {
        Code::new(504)
    }

    fn description(&self) -> &'static str {
        "assignment targets a global (missing `local`?)"
    }

    fn check(&self, ctx: &LintContext<'_>) -> Vec<LintDiagnostic> {
        let mut out = Vec::new();
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
                    let Some(Resolution::Global(name)) = ctx.lowered.resolution(hir) else {
                        continue;
                    };
                    if ctx.config.is_allowed_global(name) {
                        continue;
                    }
                    let Some(range) = ctx.node_range(hir) else {
                        continue;
                    };
                    out.push(
                        LintDiagnostic::new(range, format!("assignment to global `{name}`"))
                            .with_note(format!(
                                "add `local`, or allow it with `[lint] globals = [\"{name}\"]`"
                            )),
                    );
                }
            }
        }
        out
    }
}
