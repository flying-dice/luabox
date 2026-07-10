//! The [`Rule`] trait and lint [`Tier`]s (SPEC.md §9).

use luabox_diag::Code;

use crate::context::LintContext;
use crate::diagnostic::LintDiagnostic;

/// A lint tier (SPEC.md §9), ordered least-to-most opt-in. The tier fixes the
/// default level (see [`crate::config::tier_default`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tier {
    /// Almost certainly a bug — `deny` by default.
    Correctness,
    /// Very likely wrong — `warn`.
    Suspicious,
    /// Correct but slow — `warn`.
    Perf,
    /// Correct and fast but non-idiomatic — `warn`.
    Style,
    /// Opinionated — off unless enabled.
    Pedantic,
}

impl Tier {
    /// The tier's `[lint]` keyword.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Tier::Correctness => "correctness",
            Tier::Suspicious => "suspicious",
            Tier::Perf => "perf",
            Tier::Style => "style",
            Tier::Pedantic => "pedantic",
        }
    }

    /// Parse a tier keyword.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Tier> {
        Some(match raw {
            "correctness" => Tier::Correctness,
            "suspicious" => Tier::Suspicious,
            "perf" => Tier::Perf,
            "style" => Tier::Style,
            "pedantic" => Tier::Pedantic,
            _ => return None,
        })
    }
}

/// One lint rule: a self-contained analysis over the shared parse/HIR/type
/// machinery (SPEC.md §9 — no regex lints).
///
/// Implementations are stateless zero-sized types; [`crate::rules`] holds the
/// registry. The `id` is kebab-case and doubles as the human rule name; the
/// `code` is the stable `LB05xx` registry code.
pub trait Rule: Sync {
    /// The kebab-case rule id (also the human name).
    fn id(&self) -> &'static str;

    /// The rule's tier.
    fn tier(&self) -> Tier;

    /// The `LB05xx` diagnostic code.
    fn code(&self) -> Code;

    /// A one-line description.
    fn description(&self) -> &'static str;

    /// Run the rule over one file, returning its findings.
    fn check(&self, ctx: &LintContext<'_>) -> Vec<LintDiagnostic>;
}
