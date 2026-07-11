//! Minimal `---@diagnostic disable: undefined-field` handling for the
//! checker's undefined-field read rule (`LB0306`, #90).
//!
//! luabox has no general checker-side `---@diagnostic` engine yet — the only
//! directive infrastructure lives in `luabox-lint` (`suppress.rs`), scoped to
//! `undefined-global`. Rather than take a dependency on the linter (and risk a
//! crate cycle), this module re-implements just the slice needed to suppress
//! `undefined-field`, mirroring `suppress.rs`'s mapping and line semantics.
//!
//! TODO(#90): promote to a shared checker-side directive layer once a second
//! type diagnostic needs `---@diagnostic` suppression.

use std::collections::HashSet;

/// The luals rule name that maps onto `LB0306`.
const UNDEFINED_FIELD: &str = "undefined-field";

/// Lines a `---@diagnostic disable*: undefined-field` directive suppresses.
#[derive(Default)]
pub(crate) struct UndefinedFieldSuppression {
    /// A bare `disable` suppresses the whole file (matches `suppress.rs`: a
    /// superset of luals' "from here to EOF" that fits every real placement).
    file_wide: bool,
    /// 1-based lines a `disable-line` / `disable-next-line` covers.
    lines: HashSet<usize>,
}

impl UndefinedFieldSuppression {
    /// Scan `source` for `---@diagnostic` comments naming `undefined-field`.
    pub(crate) fn scan(source: &str) -> Self {
        let mut out = Self::default();
        for (i, line) in source.lines().enumerate() {
            let comment_line = i + 1;
            let Some(rest) = line.split("@diagnostic").nth(1) else {
                continue;
            };
            // `rest` is e.g. ` disable: undefined-field, foo]` — split the
            // action from the comma-separated name list.
            let Some((action, names)) = rest.trim().split_once(':') else {
                continue;
            };
            let names_undefined_field = names
                .split(',')
                .map(|n| n.trim().trim_end_matches([']', '=']).trim())
                .any(|n| n == UNDEFINED_FIELD);
            if !names_undefined_field {
                continue;
            }
            match action.trim() {
                "disable" => out.file_wide = true,
                // Both line forms record the *comment* line; `suppresses`
                // fans out to the line below, covering the trailing form (read
                // on the comment line) and the comment-above form (read on the
                // next line) alike — matching `suppress.rs`.
                "disable-line" | "disable-next-line" => {
                    out.lines.insert(comment_line);
                }
                _ => {}
            }
        }
        out
    }

    /// Whether any directive was found (a cheap short-circuit for callers).
    pub(crate) fn any(&self) -> bool {
        self.file_wide || !self.lines.is_empty()
    }

    /// Whether a diagnostic on 1-based `line` is suppressed. A line directive
    /// covers its own line (trailing form) and the line below (comment-above
    /// form), matching `suppress.rs`.
    pub(crate) fn suppresses(&self, line: usize) -> bool {
        self.file_wide
            || self.lines.contains(&line)
            || (line > 0 && self.lines.contains(&(line - 1)))
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
        let sup = UndefinedFieldSuppression::scan(
            "---@diagnostic disable-next-line: undefined-field\nlocal x = p.nope\n",
        );
        assert!(sup.suppresses(2));
        assert!(!sup.suppresses(3));
    }

    #[test]
    fn trailing_disable_line_covers_its_own_line() {
        let sup = UndefinedFieldSuppression::scan(
            "local x = p.nope ---@diagnostic disable-line: undefined-field\n",
        );
        assert!(sup.suppresses(1));
    }

    #[test]
    fn bare_disable_is_file_wide() {
        let sup = UndefinedFieldSuppression::scan("---@diagnostic disable: undefined-field\n");
        assert!(sup.suppresses(1));
        assert!(sup.suppresses(999));
    }

    #[test]
    fn an_unrelated_rule_name_is_ignored() {
        let sup = UndefinedFieldSuppression::scan("---@diagnostic disable: undefined-global\n");
        assert!(!sup.any());
    }
}
