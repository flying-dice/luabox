//! `unused-local` (style): a `local` binding that is never read.

use luabox_diag::Code;
use luabox_hir::BindingKind;

use crate::context::{LintContext, to_range};
use crate::diagnostic::{Fix, LintDiagnostic};
use crate::rule::{Rule, Tier};

/// A `local` (or `local function`) that is declared but never read (SPEC.md
/// §9). Skips `_`-prefixed names; fixes by renaming to `_name` when the
/// binding has no other references (a single, safe edit).
///
/// Silent for the whole file when it is a `---@meta` definition file
/// (SPEC.md §3, ticket #76): a defs file's locals are often structural
/// scaffolding (e.g. building up a class table before exporting it) rather
/// than leftovers.
pub struct UnusedLocal;

impl Rule for UnusedLocal {
    fn id(&self) -> &'static str {
        "unused-local"
    }

    fn tier(&self) -> Tier {
        Tier::Style
    }

    fn code(&self) -> Code {
        Code::new(501)
    }

    fn description(&self) -> &'static str {
        "a local binding is never read"
    }

    fn check(&self, ctx: &LintContext<'_>) -> Vec<LintDiagnostic> {
        if ctx.facts.is_meta() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (id, binding) in ctx.lowered.bindings() {
            let is_local = matches!(
                binding.kind,
                BindingKind::Local | BindingKind::LocalFunction
            );
            if !is_local || binding.name.is_empty() || binding.name.starts_with('_') {
                continue;
            }
            if ctx.uses.is_read(id) {
                continue;
            }
            let what = if binding.kind == BindingKind::LocalFunction {
                "local function"
            } else {
                "local"
            };
            let range = to_range(binding.range);
            let mut diag =
                LintDiagnostic::new(range.clone(), format!("unused {what} `{}`", binding.name))
                    .with_note("prefix the name with `_` to mark it deliberately unused");
            // Only offer an autofix when nothing references the binding at
            // all: renaming the declaration alone is then guaranteed safe.
            if ctx.uses.occurrences(id) == 0 {
                diag = diag.with_fix(Fix::machine(range, format!("_{}", binding.name)));
            }
            out.push(diag);
        }
        out
    }
}
