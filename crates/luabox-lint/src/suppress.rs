//! `---@luabox-ignore rule-id reason` suppression (SPEC.md §9), plus a
//! minimal mapping from the LuaCATS `---@diagnostic disable[-*]: <names>`
//! directive (ticket #103) onto the same suppression tables.
//!
//! The `---@luabox-ignore` reason is **mandatory**: a tag missing its rule id
//! or reason is itself a (correctness-tier) `malformed-ignore` diagnostic
//! (`LB0500`). A well-formed tag suppresses its rule on the same line or the
//! line below; placed before the first statement it suppresses the rule
//! file-wide.
//!
//! `---@diagnostic` is luals' own suppression vocabulary — parsed already
//! (`luabox_syntax::luacats::Tag::Diagnostic`) but, until now, a no-op: no
//! rule of ours shared a name with luals' vocabulary. `undefined-global` is
//! the first one that does (SPEC.md's launch-gate item #103), so this module
//! wires *only that one name* onto its own suppression tables — a full
//! `disable`/`enable`/multi-rule-list engine is out of scope until a second
//! rule needs it (see [`MAPPED_DIAGNOSTIC_RULES`]).

use std::collections::HashSet;
use std::ops::Range;

use luabox_syntax::lua::ast::AstNode as _;
use luabox_syntax::lua::{self, SyntaxKind};

/// The tag marker. Written `---@luabox-ignore`; we match the `@`-word so a
/// plain `--` comment carrying it is honoured too.
const MARKER: &str = "@luabox-ignore";

/// The LuaCATS diagnostic-directive marker: `---@diagnostic <action>: <names>`.
const DIAGNOSTIC_MARKER: &str = "@diagnostic";

/// Rule ids this crate recognises inside a `---@diagnostic` name list.
/// luals' vocabulary is much larger (`undefined-field`, `missing-return`,
/// ...); only names that are *also* one of our own rule ids get wired up, so
/// this list grows one entry at a time as our rules gain the same name —
/// scoped deliberately to `undefined-global` for now (ticket #103).
const MAPPED_DIAGNOSTIC_RULES: &[&str] = &["undefined-global"];

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
            let start = usize::from(token.text_range().start());
            if let Some(pos) = text.find(DIAGNOSTIC_MARKER) {
                out.collect_diagnostic_directive(text, pos, line_of(source, start));
            }
            let Some(pos) = text.find(MARKER) else {
                continue;
            };
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

    /// Parse one `---@diagnostic <action>: <name>[, <name>...]` comment
    /// (`text`, with the marker found at byte `marker_pos` inside it,
    /// tagged as if it started on `comment_line`) and, for every listed name
    /// this crate maps (see [`MAPPED_DIAGNOSTIC_RULES`]), fold it into the
    /// same `file_rules`/`line_rules` tables `---@luabox-ignore` populates.
    ///
    /// luals' own grammar is richer than what's implemented here: `enable`
    /// is not honoured (there is no "re-enable" bookkeeping — a bare
    /// `disable` is treated as suppressing the rule for the *whole file*,
    /// regardless of where the comment sits, which is a superset of luals'
    /// "from here to EOF" semantics but matches every real-world placement
    /// this project's fixtures use), and only `disable` / `disable-next-line`
    /// / `disable-line` are recognised. Widen this once a second rule needs
    /// the richer form.
    fn collect_diagnostic_directive(&mut self, text: &str, marker_pos: usize, comment_line: usize) {
        let rest = text[marker_pos + DIAGNOSTIC_MARKER.len()..]
            .trim_end_matches([']', '='])
            .trim();
        let Some((action, names)) = rest.split_once(':') else {
            return;
        };
        let action = action.trim();
        let mapped = names
            .split(',')
            .map(str::trim)
            .any(|name| MAPPED_DIAGNOSTIC_RULES.contains(&name));
        if !mapped {
            return;
        }
        match action {
            "disable" => {
                self.file_rules.insert("undefined-global".to_owned());
            }
            "disable-line" | "disable-next-line" => {
                self.line_rules
                    .insert(("undefined-global".to_owned(), comment_line));
            }
            _ => {}
        }
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
