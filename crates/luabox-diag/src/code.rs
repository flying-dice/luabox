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
        f.write_str("not a valid diagnostic code; codes look like LB0421")
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
    /// An informational note, usually attached to another diagnostic.
    Note,
    /// A suggestion for how to proceed.
    Help,
}

impl Severity {
    /// The lowercase keyword used in rustc-style human rendering.
    #[must_use]
    pub const fn keyword(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Note => "note",
            Self::Help => "help",
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
    fn severity_keywords() {
        assert_eq!(Severity::Error.keyword(), "error");
        assert_eq!(Severity::Warning.keyword(), "warning");
    }
}
