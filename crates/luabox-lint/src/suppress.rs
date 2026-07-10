//! `---@luabox-ignore rule-id reason` suppression (SPEC.md §9).
//!
//! The reason is **mandatory**: a tag missing its rule id or reason is itself
//! a (correctness-tier) `malformed-ignore` diagnostic (`LB0500`). A well-formed
//! tag suppresses its rule on the same line or the line below; placed before
//! the first statement it suppresses the rule file-wide.

use std::collections::HashSet;
use std::ops::Range;

use luabox_syntax::lua::ast::AstNode as _;
use luabox_syntax::lua::{self, SyntaxKind};

/// The tag marker. Written `---@luabox-ignore`; we match the `@`-word so a
/// plain `--` comment carrying it is honoured too.
const MARKER: &str = "@luabox-ignore";

/// Parsed suppressions for one file.
#[derive(Debug, Default)]
pub struct Suppressions {
    /// `(rule-id, comment-line)` — suppresses that line and the one below.
    line_rules: HashSet<(String, usize)>,
    /// Rule ids suppressed for the whole file.
    file_rules: HashSet<String>,
    /// Malformed tags: `(range, message)` — each becomes an `LB0500`.
    pub malformed: Vec<(Range<usize>, String)>,
}

impl Suppressions {
    /// Scan a parsed file for suppression comments.
    #[must_use]
    pub fn collect(parse: &lua::Parse, source: &str) -> Self {
        let root = parse.syntax();
        let first_stmt = parse
            .tree()
            .block()
            .and_then(|b| b.stmts().next())
            .map_or(usize::MAX, |s| usize::from(s.syntax().text_range().start()));

        let mut out = Suppressions::default();
        for token in root
            .descendants_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .filter(|t| t.kind() == SyntaxKind::COMMENT)
        {
            let text = token.text();
            let Some(pos) = text.find(MARKER) else {
                continue;
            };
            let start = usize::from(token.text_range().start());
            let range = to_range(token.text_range());

            let rest = text[pos + MARKER.len()..]
                .trim_end_matches([']', '='])
                .trim();
            let mut words = rest.split_whitespace();
            let Some(rule_id) = words.next() else {
                out.malformed.push((
                    range,
                    "`---@luabox-ignore` needs a rule id and a reason".to_owned(),
                ));
                continue;
            };
            let reason = words.collect::<Vec<_>>().join(" ");
            if reason.is_empty() {
                out.malformed.push((
                    range,
                    format!("`---@luabox-ignore {rule_id}` is missing its mandatory reason"),
                ));
                continue;
            }

            if start < first_stmt {
                out.file_rules.insert(rule_id.to_owned());
            } else {
                out.line_rules
                    .insert((rule_id.to_owned(), line_of(source, start)));
            }
        }
        out
    }

    /// Whether `rule_id` is suppressed for a diagnostic on `line` (1-based).
    #[must_use]
    pub fn is_suppressed(&self, rule_id: &str, line: usize) -> bool {
        if self.file_rules.contains(rule_id) {
            return true;
        }
        // A comment on line L covers L (trailing) and L+1 (comment above).
        self.line_rules.contains(&(rule_id.to_owned(), line))
            || (line > 0 && self.line_rules.contains(&(rule_id.to_owned(), line - 1)))
    }
}

/// The 1-based line number of a byte offset.
#[must_use]
pub fn line_of(source: &str, offset: usize) -> usize {
    1 + source[..offset.min(source.len())]
        .bytes()
        .filter(|&b| b == b'\n')
        .count()
}

fn to_range(range: rowan::TextRange) -> Range<usize> {
    usize::from(range.start())..usize::from(range.end())
}
