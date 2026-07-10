//! Lowering diagnostics (SPEC.md §2.1, §14).
//!
//! Codes are plain `LBnnnn` strings — this crate, like `luabox-syntax`,
//! stays off `luabox-diag` (acyclic dep graph, SPEC.md §16); the CLI maps
//! these onto registered diagnostics with explain pages.

use rowan::TextRange;

/// Reported by [`crate::lower`] when the input does not parse (the caller
/// is expected to have run `luabox check` first, so this is a safety net).
pub(crate) const PARSE_ERROR: &str = "LB0001";
/// Irreducible `goto`: no reducible loop/skip shape fits (SPEC.md §2.1).
pub(crate) const IRREDUCIBLE_GOTO: &str = "LB0601";
/// Reassignment of a `<const>` (or `<close>`) local, caught at compile time.
pub(crate) const CONST_REASSIGNED: &str = "LB0602";
/// `<close>` lowering fidelity: warn tier for the coroutine-error-path
/// delta (suppressible via `---@luabox-allow lossy-lowering`), error tier
/// for scope tails the `pcall` rewrite cannot wrap.
pub(crate) const CLOSE_FIDELITY: &str = "LB0603";
/// `_ENV` use outside the lowerable idioms.
pub(crate) const ENV_NOT_LOWERABLE: &str = "LB0604";
/// LuaJIT extension with no polyfill (`ffi`, unknown `bit.*` members,
/// 64-bit/imaginary number literals).
pub(crate) const JIT_NOT_LOWERABLE: &str = "LB0605";
/// Integer/float divergence heuristics (5.3+ integers onto double-only
/// targets), warn tier.
pub(crate) const INT_FLOAT_DIVERGENCE: &str = "LB0606";

/// Diagnostic severity: errors abort the lowering of a file; warnings ride
/// along on [`crate::Lowered::warnings`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// One lowering diagnostic, anchored to a byte range of the *input* text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LowerDiagnostic {
    /// `LBnnnn` code string (registered in `luabox-diag`).
    pub code: &'static str,
    pub severity: Severity,
    pub message: String,
    pub range: TextRange,
}

impl LowerDiagnostic {
    pub(crate) fn error(code: &'static str, message: String, range: TextRange) -> Self {
        LowerDiagnostic {
            code,
            severity: Severity::Error,
            message,
            range,
        }
    }

    pub(crate) fn warning(code: &'static str, message: String, range: TextRange) -> Self {
        LowerDiagnostic {
            code,
            severity: Severity::Warning,
            message,
            range,
        }
    }
}
