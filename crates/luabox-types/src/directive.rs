//! Minimal checker-side `---@diagnostic disable*: <rule>` handling for the
//! type diagnostics that carry a luals rule name (`undefined-field` → `LB0306`,
//! `deprecated` → `LB0308`, `discard-returns` → `LB0309`, `duplicate-doc-field`
//! → `LB0311`, `invisible` → `LB0312`).
//!
//! luabox has no general checker-side `---@diagnostic` engine yet — the only
//! other directive infrastructure lives in `luabox-lint` (`suppress.rs`), scoped
//! to the lint rule names. Rather than take a dependency on the linter (and risk
//! a crate cycle), this module re-implements just the slice the checker needs,
//! mirroring `suppress.rs`'s mapping and line semantics, keyed by rule name so
//! one scan serves every checker diagnostic that luals lets you suppress.

use std::collections::HashMap;
use std::collections::HashSet;

use luabox_diag::Code;

use crate::codes;

/// The luals rule names the checker recognises in a `---@diagnostic` directive.
/// A directive naming any other rule is ignored here (it may belong to the
/// linter, which scans independently). `duplicate-doc-alias` is absent by
/// design: like the `LB0307` class collision it is a project-assembly finding,
/// not a per-file check, so this per-file filter never sees it.
const KNOWN_RULES: &[&str] = &[
    "undefined-field",
    "deprecated",
    "discard-returns",
    "duplicate-doc-field",
    "invisible",
    "await-in-sync",
];

/// The luals rule name that maps onto a checker [`Code`], or `None` when the
/// code carries no `---@diagnostic`-suppressible name.
///
/// Takes the `Code` itself, not its rendering: this runs once per diagnostic on
/// every check, and `Code` equality is an integer compare.
pub(crate) fn rule_for_code(code: Code) -> Option<&'static str> {
    match code {
        codes::FIELD_NOT_FOUND => Some("undefined-field"),
        codes::DEPRECATED => Some("deprecated"),
        codes::DISCARD_RETURNS => Some("discard-returns"),
        codes::DUPLICATE_DOC_FIELD => Some("duplicate-doc-field"),
        codes::INVISIBLE => Some("invisible"),
        codes::AWAIT_IN_SYNC => Some("await-in-sync"),
        // LB0310 (duplicate-doc-alias) is a project-assembly finding, like the
        // LB0307 class collision — it never flows through this per-file filter,
        // so it has no entry here.
        _ => None,
    }
}

/// Per-rule suppression state: whether a bare `disable` covered the whole file
/// and which 1-based comment lines a `disable-line`/`disable-next-line` named.
#[derive(Default)]
struct RuleState {
    file_wide: bool,
    lines: HashSet<usize>,
}

/// All `---@diagnostic disable*` directives in a source, indexed by luals rule
/// name — a superset scan reused across every checker diagnostic.
#[derive(Default)]
pub(crate) struct DirectiveScan {
    rules: HashMap<&'static str, RuleState>,
}

impl DirectiveScan {
    /// Scan `source` for `---@diagnostic` comments naming any [`KNOWN_RULES`].
    pub(crate) fn scan(source: &str) -> Self {
        let mut out = Self::default();
        for (i, line) in source.lines().enumerate() {
            let comment_line = i + 1;
            let Some(rest) = line.split("@diagnostic").nth(1) else {
                continue;
            };
            // `rest` is e.g. ` disable: deprecated, foo]` — split the action
            // from the comma-separated name list.
            let Some((action, names)) = rest.trim().split_once(':') else {
                continue;
            };
            let matched: Vec<&'static str> = names
                .split(',')
                .filter_map(|n| {
                    let n = n.trim().trim_end_matches([']', '=']).trim();
                    KNOWN_RULES.iter().copied().find(|&rule| rule == n)
                })
                .collect();
            if matched.is_empty() {
                continue;
            }
            for rule in matched {
                let state = out.rules.entry(rule).or_default();
                match action.trim() {
                    "disable" => state.file_wide = true,
                    // Both line forms record the *comment* line; `suppresses`
                    // fans out to the line below, covering the trailing form
                    // (read on the comment line) and the comment-above form
                    // (read on the next line) alike — matching `suppress.rs`.
                    "disable-line" | "disable-next-line" => {
                        state.lines.insert(comment_line);
                    }
                    _ => {}
                }
            }
        }
        out
    }

    /// Whether any recognised directive was found (a cheap short-circuit).
    pub(crate) fn any(&self) -> bool {
        self.rules
            .values()
            .any(|s| s.file_wide || !s.lines.is_empty())
    }

    /// Whether a diagnostic for luals `rule` on 1-based `line` is suppressed. A
    /// line directive covers its own line (trailing form) and the line below
    /// (comment-above form), matching `suppress.rs`.
    pub(crate) fn suppresses(&self, rule: &str, line: usize) -> bool {
        let Some(state) = self.rules.get(rule) else {
            return false;
        };
        state.file_wide
            || state.lines.contains(&line)
            || (line > 0 && state.lines.contains(&(line - 1)))
    }
}

/// The 1-based line number of a byte offset (mirrors `suppress.rs::line_of`).
pub(crate) fn line_of(source: &str, offset: usize) -> usize {
    1 + source
        .bytes()
        .take(offset.min(source.len()))
        .filter(|&b| b == b'\n')
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disable_next_line_covers_the_line_below() {
        let sup = DirectiveScan::scan(
            "---@diagnostic disable-next-line: undefined-field\nlocal x = p.nope\n",
        );
        assert!(sup.suppresses("undefined-field", 2));
        assert!(!sup.suppresses("undefined-field", 3));
    }

    #[test]
    fn trailing_disable_line_covers_its_own_line() {
        let sup =
            DirectiveScan::scan("local x = p.nope ---@diagnostic disable-line: undefined-field\n");
        assert!(sup.suppresses("undefined-field", 1));
    }

    #[test]
    fn bare_disable_is_file_wide() {
        let sup = DirectiveScan::scan("---@diagnostic disable: undefined-field\n");
        assert!(sup.suppresses("undefined-field", 1));
        assert!(sup.suppresses("undefined-field", 999));
    }

    #[test]
    fn an_unrelated_rule_name_is_ignored() {
        let sup = DirectiveScan::scan("---@diagnostic disable: undefined-global\n");
        assert!(!sup.any());
    }

    #[test]
    fn rules_are_kept_separate() {
        let sup = DirectiveScan::scan("---@diagnostic disable: deprecated\n");
        assert!(sup.suppresses("deprecated", 1));
        assert!(sup.suppresses("deprecated", 42));
        assert!(!sup.suppresses("discard-returns", 1));
        assert!(!sup.suppresses("undefined-field", 1));
    }

    #[test]
    fn multiple_rules_on_one_line() {
        let sup = DirectiveScan::scan("---@diagnostic disable: deprecated, discard-returns\n");
        assert!(sup.suppresses("deprecated", 5));
        assert!(sup.suppresses("discard-returns", 5));
    }

    #[test]
    fn code_to_rule_mapping() {
        assert_eq!(
            rule_for_code(codes::FIELD_NOT_FOUND),
            Some("undefined-field")
        );
        assert_eq!(rule_for_code(codes::DEPRECATED), Some("deprecated"));
        assert_eq!(
            rule_for_code(codes::DISCARD_RETURNS),
            Some("discard-returns")
        );
        assert_eq!(
            rule_for_code(codes::DUPLICATE_DOC_FIELD),
            Some("duplicate-doc-field")
        );
        assert_eq!(rule_for_code(codes::INVISIBLE), Some("invisible"));
        assert_eq!(rule_for_code(codes::AWAIT_IN_SYNC), Some("await-in-sync"));
        assert_eq!(rule_for_code(codes::TYPE_MISMATCH), None);
    }

    /// The `LBnnnn` spellings the mapping is defined in terms of — a guard that
    /// the `codes` constants keep naming the codes luals' rule names attach to.
    #[test]
    fn the_mapped_codes_are_the_documented_ones() {
        for (code, rule) in [
            (codes::FIELD_NOT_FOUND, "undefined-field"),
            (codes::DEPRECATED, "deprecated"),
            (codes::DISCARD_RETURNS, "discard-returns"),
            (codes::DUPLICATE_DOC_FIELD, "duplicate-doc-field"),
            (codes::INVISIBLE, "invisible"),
            (codes::AWAIT_IN_SYNC, "await-in-sync"),
        ] {
            assert_eq!(rule_for_code(code), Some(rule), "{code}");
        }
        assert_eq!(codes::FIELD_NOT_FOUND.to_string(), "LB0306");
        assert_eq!(codes::DEPRECATED.to_string(), "LB0308");
        assert_eq!(codes::DISCARD_RETURNS.to_string(), "LB0309");
        assert_eq!(codes::DUPLICATE_DOC_FIELD.to_string(), "LB0311");
        assert_eq!(codes::INVISIBLE.to_string(), "LB0312");
        assert_eq!(codes::AWAIT_IN_SYNC.to_string(), "LB0316");
    }
}
