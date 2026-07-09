//! Manifest validation errors (SPEC.md §5, §14: span-rich diagnostics).
//!
//! `luabox-resolve` doesn't depend on `luabox-diag` (Distribution owns the
//! package-graph API, not diagnostic rendering); this is a minimal,
//! self-contained error type the frontend can later lift into an `LB0xxx`
//! diagnostic.

use std::fmt::{self, Write as _};
use std::ops::Range;

/// A single manifest validation or parse failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError {
    pub message: String,
    /// Byte range into the original source text, when available.
    ///
    /// `toml_edit` attaches spans to most parsed items; some synthetic
    /// errors (e.g. "missing required table") have no natural span and
    /// carry `None`.
    pub span: Option<Range<usize>>,
}

impl ManifestError {
    pub(super) fn new(message: impl Into<String>, span: Option<Range<usize>>) -> Self {
        Self {
            message: message.into(),
            span,
        }
    }

    /// An unknown key/table error with a cargo-style "did you mean" nudge
    /// and the full valid set, e.g.:
    /// `unknown [package] key "editon", did you mean "edition"? (valid: name, version, edition, ...)`
    pub(super) fn unknown_key(
        what: &str,
        key: &str,
        valid: &[&str],
        span: Option<Range<usize>>,
    ) -> Self {
        let mut message = format!("unknown {what} `{key}`");
        if let Some(suggestion) = suggest(key, valid) {
            let _ = write!(message, ", did you mean `{suggestion}`?");
        }
        let _ = write!(message, " (valid: {})", valid.join(", "));
        Self::new(message, span)
    }
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ManifestError {}

/// Closest candidate within edit-distance 2, if any (cargo's typo-guard
/// threshold). Ties broken by declaration order in `candidates`.
pub(super) fn suggest<'a>(key: &str, candidates: &[&'a str]) -> Option<&'a str> {
    candidates
        .iter()
        .map(|candidate| (*candidate, levenshtein(key, candidate)))
        .filter(|(_, distance)| *distance <= 2)
        .min_by_key(|(_, distance)| *distance)
        .map(|(candidate, _)| candidate)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut prev_diag = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            let new_val = (row[j + 1] + 1).min(row[j] + 1).min(prev_diag + cost);
            prev_diag = row[j + 1];
            row[j + 1] = new_val;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levenshtein_basics() {
        assert_eq!(levenshtein("edition", "edition"), 0);
        assert_eq!(levenshtein("editon", "edition"), 1);
        assert_eq!(levenshtein("", "abc"), 3);
    }

    #[test]
    fn suggest_finds_close_typo() {
        let valid = ["name", "version", "edition", "description", "license"];
        assert_eq!(suggest("editon", &valid), Some("edition"));
        assert_eq!(suggest("versio", &valid), Some("version"));
        assert_eq!(suggest("totally-unrelated-key", &valid), None);
    }
}
