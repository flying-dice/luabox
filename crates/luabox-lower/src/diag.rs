//! Lowering diagnostics (SPEC.md §2.1, §14).
//!
//! Codes are the bare numeric part of `LBnnnn` (`601` for `LB0601`) — this
//! crate, like `luabox-syntax`, stays off `luabox-diag` (acyclic dep graph,
//! SPEC.md §16). A `u16` is exactly what `luabox_diag::Code::new` takes, so a
//! frontend converts by construction rather than re-parsing a string this
//! crate had just formatted, and no "cannot happen" arm is left to abort in.

use rowan::TextRange;

/// Reported by [`crate::lower`] when the input does not parse (the caller
/// is expected to have run `luabox check` first, so this is a safety net).
pub(crate) const PARSE_ERROR: u16 = 1;
/// Irreducible `goto`: no reducible loop/skip shape fits (SPEC.md §2.1).
pub(crate) const IRREDUCIBLE_GOTO: u16 = 601;
/// Reassignment of a `<const>` (or `<close>`) local, caught at compile time.
pub(crate) const CONST_REASSIGNED: u16 = 602;
/// `<close>` lowering fidelity: warn tier for the coroutine-error-path
/// delta (suppressible via `---@luabox-allow lossy-lowering`), error tier
/// for scope tails the `pcall` rewrite cannot wrap.
pub(crate) const CLOSE_FIDELITY: u16 = 603;
/// `_ENV` use outside the lowerable idioms.
pub(crate) const ENV_NOT_LOWERABLE: u16 = 604;
/// LuaJIT extension with no polyfill (`ffi`, unknown `bit.*` members,
/// 64-bit/imaginary number literals).
pub(crate) const JIT_NOT_LOWERABLE: u16 = 605;
/// Integer/float divergence heuristics (5.3+ integers onto double-only
/// targets), warn tier.
pub(crate) const INT_FLOAT_DIVERGENCE: u16 = 606;

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
    /// The numeric part of the `LBnnnn` code (registered in `luabox-diag`).
    pub code: u16,
    pub severity: Severity,
    pub message: String,
    pub range: TextRange,
}

impl LowerDiagnostic {
    pub(crate) fn error(code: u16, message: String, range: TextRange) -> Self {
        LowerDiagnostic {
            code,
            severity: Severity::Error,
            message,
            range,
        }
    }

    pub(crate) fn warning(code: u16, message: String, range: TextRange) -> Self {
        LowerDiagnostic {
            code,
            severity: Severity::Warning,
            message,
            range,
        }
    }
}
