//! `unused-param` (pedantic): a function parameter that is never read.

use luabox_diag::Code;
use luabox_hir::BindingKind;

use crate::context::{LintContext, to_range};
use crate::diagnostic::{Fix, LintDiagnostic};
use crate::rule::{Rule, Tier};

/// A parameter that is never read (SPEC.md §9). Pedantic (off by default):
/// unused parameters are often mandated by a callback shape. The implicit
/// `self` of a `:` method is exempt, as are `_`-prefixed names.
pub struct UnusedParam;

impl Rule for UnusedParam {
    fn id(&self) -> &'static str {
        "unused-param"
    }

    fn tier(&self) -> Tier {
        Tier::Pedantic
    }

    fn code(&self) -> Code {
        Code::new(502)
    }

    fn description(&self) -> &'static str {
        "a function parameter is never read"
    }

    fn check(&self, ctx: &LintContext<'_>) -> Vec<LintDiagnostic> {
        let mut out = Vec::new();
        for (id, binding) in ctx.lowered.bindings() {
            // `SelfParam` is the implicit method receiver — always exempt.
            if binding.kind != BindingKind::Param {
                continue;
            }
            if binding.name.is_empty() || binding.name == "self" || binding.name.starts_with('_') {
                continue;
            }
            if ctx.uses.is_read(id) {
                continue;
            }
            let range = to_range(binding.range);
            let mut diag = LintDiagnostic::new(
                range.clone(),
                format!("unused parameter `{}`", binding.name),
            )
            .with_note("prefix the name with `_` to mark it deliberately unused");
            if ctx.uses.occurrences(id) == 0 {
                diag = diag.with_fix(Fix::machine(range, format!("_{}", binding.name)));
            }
            out.push(diag);
        }
        out
    }
}
