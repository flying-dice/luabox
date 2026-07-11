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
];

/// The luals rule name that maps onto a checker `LBnnnn` code, or `None` when
/// the code carries no `---@diagnostic`-suppressible name.
pub(crate) fn rule_for_code(code: &str) -> Option<&'static str> {
    match code {
        "LB0306" => Some("undefined-field"),
        "LB0308" => Some("deprecated"),
        "LB0309" => Some("discard-returns"),
        "LB0311" => Some("duplicate-doc-field"),
        "LB0312" => Some("invisible"),
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
    1 + source[..offset.min(source.len())]
        .bytes()
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
        assert_eq!(rule_for_code("LB0306"), Some("undefined-field"));
        assert_eq!(rule_for_code("LB0308"), Some("deprecated"));
        assert_eq!(rule_for_code("LB0309"), Some("discard-returns"));
        assert_eq!(rule_for_code("LB0311"), Some("duplicate-doc-field"));
        assert_eq!(rule_for_code("LB0312"), Some("invisible"));
        assert_eq!(rule_for_code("LB0300"), None);
    }
}
