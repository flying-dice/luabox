//! Diagnostic codes and severities — the shared vocabulary.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A validated diagnostic code of the form `LBnnnn` (exactly four digits).
///
/// The leading digit partitions the code space into blocks:
///
/// - `0xxx` — core / syntax: lexer, parser, generic frontend errors.
/// - `1xxx` — manifest / config: `luabox.toml`, editions, workspace layout.
///
/// Blocks `2xxx` and above are unassigned and reserved for later contexts
/// (types, lint, lowering, resolver, ...). Internally the code is stored as a
/// number so its rendering (`LB{:04}`) is always canonical.
///
/// Inside block `0` the hundreds are subdivided by producer — `03xx`
/// typecheck, `05xx` lint, `06xx` lowering. Only one of those subdivisions is
/// load-bearing outside the registry (consumers branch on it), and it has a
/// predicate rather than open-coded arithmetic: see [`Code::is_lint`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Code(u16);

impl Code {
    /// The largest representable code number (`LB9999`).
    pub const MAX: u16 = 9999;

    /// Construct a code from its numeric part.
    ///
    /// # Panics
    ///
    /// Panics if `number` exceeds [`Code::MAX`]; this makes it usable in
    /// `const` contexts (the registry table) with compile-time validation.
    #[must_use]
    pub const fn new(number: u16) -> Self {
        assert!(number <= Self::MAX, "diagnostic code out of range");
        Self(number)
    }

    /// The block this code belongs to (the leading digit: 0, 1, 2, ...).
    #[must_use]
    pub const fn block(self) -> u16 {
        self.0 / 1000
    }

    /// The raw numeric part (e.g. `1` for `LB0001`).
    #[must_use]
    pub const fn number(self) -> u16 {
        self.0
    }

    /// Whether this code sits in the lint band — **the** authority on that
    /// question, so nobody has to open-code `number() / 100 == 5`.
    ///
    /// # The band contract
    ///
    /// `LB0500`-`LB0599` belongs to `luabox-lint` and to nothing else:
    ///
    /// - `LB0500` is the crate's own suppression-syntax diagnostic (a
    ///   malformed `---@luabox-ignore`). It is not a rule: it has no tier and
    ///   no id, and it cannot be suppressed.
    /// - `LB0501`+ are the rule codes, one per entry in `luabox_lint::rules`.
    ///
    /// The invariant that every rule's code lands in this band is asserted in
    /// `luabox-lint`, where the registry lives; this crate cannot see the rule
    /// set, so it asserts only the half it owns — that the band is allocated
    /// densely from [`Code::LINT_BAND_START`].
    ///
    /// Consumers use it to decide provenance rather than meaning: the language
    /// server tags findings in this band with the `luabox-lint` source, which
    /// is what its quick-fix matcher keys off. The control-flow legality
    /// errors (`LB0020`-`LB0022`) travel through the same lint engine and are
    /// deliberately *not* in the band — they are not rules. Neither is the
    /// next band up: `LB06xx` is lowering's, and already allocated.
    ///
    /// Do **not** build a band test on [`Code::block`]: that is the leading
    /// digit of a four-digit code, so it is `0` for every `LB0xxx` and cannot
    /// tell a lint code from a syntax one.
    #[must_use]
    pub const fn is_lint(self) -> bool {
        self.0 >= Self::LINT_BAND_START && self.0 <= Self::LINT_BAND_END
    }

    /// Whether this code is a lint **rule** code — in the band
    /// ([`Code::is_lint`]) and not [`Code::LINT_BAND_START`] itself.
    ///
    /// The distinction is small but real, and it is why there are two
    /// predicates rather than one: `LB0500` is `luabox-lint`'s own
    /// suppression-syntax diagnostic. It is in the band (the language server
    /// tags it with the lint source, because the lint crate is where it comes
    /// from) but it is not a rule — it has no tier, no id, no
    /// `---@luabox-ignore` spelling, and no fix. Anything reasoning about
    /// *rules* wants this predicate: the registry invariant in `luabox-lint`,
    /// and with it the "only rules carry fixes" contract the editor's
    /// quick-fix matcher rests on.
    #[must_use]
    pub const fn is_lint_rule(self) -> bool {
        self.is_lint() && self.0 != Self::LINT_BAND_START
    }

    /// First code of the lint band — `LB0500`, the suppression-syntax
    /// diagnostic. Rule codes start one above it. See [`Code::is_lint`].
    pub const LINT_BAND_START: u16 = 500;
    /// Last code of the lint band — see [`Code::is_lint`]. `LB0600` and up
    /// belong to lowering.
    pub const LINT_BAND_END: u16 = 599;
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LB{:04}", self.0)
    }
}

impl fmt::Debug for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Code({self})")
    }
}

/// The error returned when a string is not a well-formed `LBnnnn` code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodeParseError;

impl fmt::Display for CodeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not a valid diagnostic code; codes look like LB0300")
    }
}

impl std::error::Error for CodeParseError {}

impl FromStr for Code {
    type Err = CodeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let digits = s.strip_prefix("LB").ok_or(CodeParseError)?;
        if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(CodeParseError);
        }
        let number = digits.parse::<u16>().map_err(|_| CodeParseError)?;
        Ok(Self(number))
    }
}

impl Serialize for Code {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Code {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Diagnostic severity, ordered loudest-first for reporting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// A hard error: the operation cannot succeed.
    Error,
    /// A warning: suspicious but not fatal.
    Warning,
}

impl Severity {
    /// The lowercase keyword used in rustc-style human rendering.
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_codes() {
        let code: Code = "LB0001".parse().unwrap();
        assert_eq!(code.number(), 1);
        assert_eq!(code.block(), 0);
        assert_eq!(code.to_string(), "LB0001");
    }

    #[test]
    fn block_is_the_leading_digit() {
        assert_eq!("LB1001".parse::<Code>().unwrap().block(), 1);
        assert_eq!("LB2010".parse::<Code>().unwrap().block(), 2);
        assert_eq!("LB9999".parse::<Code>().unwrap().block(), 9);
    }

    #[test]
    fn round_trips_through_display() {
        for raw in ["LB0001", "LB1001", "LB2008", "LB9999"] {
            let code: Code = raw.parse().unwrap();
            assert_eq!(code.to_string(), raw);
        }
    }

    #[test]
    fn rejects_malformed_codes() {
        for bad in [
            "banana", "LB1", "LB12345", "lb0001", "LBxxxx", "0001", "LB-001", "",
        ] {
            assert!(bad.parse::<Code>().is_err(), "should reject `{bad}`");
        }
    }

    #[test]
    fn serde_round_trip_is_a_string() {
        let code: Code = "LB2001".parse().unwrap();
        let json = serde_json::to_string(&code).unwrap();
        assert_eq!(json, "\"LB2001\"");
        let back: Code = serde_json::from_str(&json).unwrap();
        assert_eq!(back, code);
    }

    #[test]
    fn new_is_the_numeric_constructor_display_agrees_with() {
        for number in [0u16, 1, 503, 1001, Code::MAX] {
            let code = Code::new(number);
            assert_eq!(code.number(), number);
            assert_eq!(code.to_string(), format!("LB{number:04}"));
            assert_eq!(code.to_string().parse::<Code>(), Ok(code));
        }
        assert_eq!(Code::new(503).block(), 0);
        assert_eq!(Code::new(1001).block(), 1);
    }

    #[test]
    fn debug_shows_the_canonical_spelling_not_the_raw_number() {
        assert_eq!(format!("{:?}", Code::new(1)), "Code(LB0001)");
        assert_eq!(format!("{:?}", Code::new(1001)), "Code(LB1001)");
    }

    #[test]
    fn the_parse_error_explains_the_expected_shape() {
        let err = "banana".parse::<Code>().unwrap_err();
        assert_eq!(
            err.to_string(),
            "not a valid diagnostic code; codes look like LB0300"
        );
        // It is a real `std::error::Error`.
        let boxed: Box<dyn std::error::Error> = Box::new(err);
        assert!(boxed.to_string().contains("LB0300"));
    }

    #[test]
    fn deserializing_a_malformed_code_is_an_error_not_a_panic() {
        let err = serde_json::from_str::<Code>("\"LB1\"").unwrap_err();
        assert!(err.to_string().contains("LB0300"), "{err}");
    }

    #[test]
    fn the_lint_band_is_exactly_500_to_599() {
        assert!(!Code::new(499).is_lint());
        assert!(Code::new(500).is_lint(), "LB0500 is the lint crate's own");
        assert!(Code::new(509).is_lint());
        assert!(Code::new(599).is_lint());
        assert!(!Code::new(600).is_lint(), "LB0601 is lowering, not lint");
        assert!(
            !Code::new(20).is_lint(),
            "control-flow legality is not lint"
        );
        assert!(!Code::new(316).is_lint(), "typecheck is not lint");
        assert!(!Code::new(1004).is_lint(), "manifest is not lint");
    }

    #[test]
    fn a_rule_code_is_in_the_band_but_is_never_lb0500() {
        assert!(!Code::new(500).is_lint_rule(), "LB0500 is not a rule");
        assert!(Code::new(501).is_lint_rule());
        assert!(Code::new(510).is_lint_rule());
        assert!(Code::new(599).is_lint_rule());
        assert!(!Code::new(499).is_lint_rule());
        assert!(!Code::new(600).is_lint_rule());
        // Every rule code is a band code; only the converse differs.
        for n in 0..=Code::MAX {
            let code = Code::new(n);
            assert!(!code.is_lint_rule() || code.is_lint(), "{code}");
        }
    }

    /// `block` is the leading digit of a four-digit code, so it cannot stand
    /// in for the band predicates — the pitfall the doc comment warns about.
    #[test]
    fn block_cannot_stand_in_for_the_lint_band() {
        assert_eq!(Code::new(1).block(), Code::new(510).block());
        assert!(!Code::new(1).is_lint());
        assert!(Code::new(510).is_lint());
    }

    #[test]
    fn severity_keywords() {
        assert_eq!(Severity::Error.keyword(), "error");
        assert_eq!(Severity::Warning.keyword(), "warning");
    }
}
