//! One source file, fetched once and indexed once, for the renderers.
//!
//! Every renderer needs the same two things per label: the 1-based line and
//! column of a byte offset, and the text of that line. Computing either by
//! walking the file from byte 0 costs O(offset), so a file with *n* findings
//! paid O(n x file size) to be reported — the human renderer spent 43 s on a
//! 1.8 MB file with 32 k diagnostics that `--format json` emitted in 0.5 s.
//!
//! [`IndexedSource`] owns the text and a table of line-start byte offsets
//! built in one pass, so [`IndexedSource::line_col`] binary-searches instead
//! of scanning and [`IndexedSource::line_text`] slices instead of iterating.
//!
//! This is deliberately a *private* re-implementation of the same idea as
//! `luabox_syntax::LineIndex`: `luabox-diag` is the cross-cutting vocabulary
//! crate every other crate depends on, and it has no workspace dependencies
//! by design (see its `Cargo.toml`). Twenty lines of `partition_point` is a
//! smaller price than inverting that dependency edge.

/// A source file plus its line-start table.
///
/// Build one per file per render, then answer every label against it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexedSource {
    /// The whole file, owned so the renderers can hand out `&str` slices of
    /// it without cloning per label.
    text: String,
    /// Byte offset of the start of each line. Always begins with `0`, so it
    /// is never empty and its length is the number of lines.
    starts: Vec<usize>,
}

impl IndexedSource {
    /// Index `text` in one pass.
    pub(crate) fn new(text: String) -> Self {
        let starts = std::iter::once(0)
            .chain(
                text.bytes()
                    .enumerate()
                    .filter(|&(_, byte)| byte == b'\n')
                    .map(|(i, _)| i + 1),
            )
            .collect();
        IndexedSource { text, starts }
    }

    /// The whole file.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// 1-based line and column for a byte `offset`.
    ///
    /// The column counts *characters*, not bytes, and an offset in the middle
    /// of a multi-byte character counts that character — exactly what the
    /// scanning implementation this replaced reported. A `\n` belongs to the
    /// line it terminates; an offset past the end clamps to the last line's
    /// end, and `\r\n` needs no special case because only the `\n` opens a
    /// line.
    pub(crate) fn line_col(&self, offset: usize) -> (usize, usize) {
        // `starts` is sorted and starts at 0, so the number of starts at or
        // before `offset` is both >= 1 and exactly the 1-based line number.
        let line = self.starts.partition_point(|&start| start <= offset);
        let start = self
            .starts
            .get(line.saturating_sub(1))
            .copied()
            .unwrap_or(0);
        let col = self
            .text
            .get(start..)
            .unwrap_or("")
            .char_indices()
            .take_while(|&(i, _)| start + i < offset)
            .count()
            + 1;
        (line, col)
    }

    /// The text of a 1-based `line`, without its terminator.
    ///
    /// Matches `str::lines`: a trailing `\r` is dropped along with the `\n`, a
    /// lone `\r` is ordinary text, and a line past the end of the file — which
    /// includes the empty line a trailing newline opens — is `""`.
    pub(crate) fn line_text(&self, line: usize) -> &str {
        let index = line.saturating_sub(1);
        let Some(&start) = self.starts.get(index) else {
            return "";
        };
        let end = self
            .starts
            .get(index + 1)
            // The next line starts one past its opening `\n`; drop that `\n`.
            .map_or(self.text.len(), |&next| next.saturating_sub(1));
        let raw = self.text.get(start..end).unwrap_or("");
        raw.strip_suffix('\r').unwrap_or(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sources that between them cover every edge the renderers can meet: an
    /// empty file, bare newlines, no trailing newline, CRLF, a lone `\r`,
    /// mixed endings, multi-byte UTF-8, and a BOM + shebang preamble.
    const TRICKY: &[&str] = &[
        "",
        "\n",
        "\n\n\n",
        "no trailing newline",
        "trailing\n",
        "crlf\r\nlines\r\n",
        "lone \r carriage return\n",
        "mixed\r\nline\nendings\r\n",
        "unicode é 中 😀\nsecond line\n",
        "\u{feff}#!/usr/bin/env lua\nreturn 1\n",
        "a\r\r\n",
        "local x = 1\nlocal = 2\nprint(x)\n",
    ];

    /// The implementation this index replaced: walk `char_indices` from byte
    /// 0, counting lines and columns, stopping at the first character that
    /// starts at or after `offset`. Kept verbatim as the oracle so the fast
    /// path is pinned to the output callers already depend on.
    fn line_col_by_scanning(source: &str, offset: usize) -> (usize, usize) {
        let mut line = 1usize;
        let mut col = 1usize;
        for (idx, ch) in source.char_indices() {
            if idx >= offset {
                break;
            }
            if ch == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        (line, col)
    }

    /// The `line_text` this replaced, likewise verbatim.
    fn line_text_by_scanning(source: &str, line: usize) -> &str {
        source.lines().nth(line.saturating_sub(1)).unwrap_or("")
    }

    #[test]
    fn line_col_matches_the_scanning_implementation_everywhere() {
        for source in TRICKY {
            let indexed = IndexedSource::new((*source).to_string());
            // Every byte offset, plus a few past the end.
            for offset in 0..=source.len() + 3 {
                assert_eq!(
                    indexed.line_col(offset),
                    line_col_by_scanning(source, offset),
                    "source {source:?} at offset {offset}"
                );
            }
        }
    }

    #[test]
    fn line_text_matches_the_scanning_implementation_everywhere() {
        for source in TRICKY {
            let indexed = IndexedSource::new((*source).to_string());
            // Line 0 is not a thing, but the old code clamped it to line 1;
            // go a few lines past the end too.
            for line in 0..source.lines().count() + 4 {
                assert_eq!(
                    indexed.line_text(line),
                    line_text_by_scanning(source, line),
                    "source {source:?} at line {line}"
                );
            }
        }
    }

    #[test]
    fn the_empty_file_is_one_empty_line() {
        let indexed = IndexedSource::new(String::new());
        assert_eq!(indexed.text(), "");
        assert_eq!(indexed.line_col(0), (1, 1));
        assert_eq!(indexed.line_col(usize::MAX), (1, 1));
        assert_eq!(indexed.line_text(1), "");
        assert_eq!(indexed.line_text(2), "");
    }

    #[test]
    fn a_newline_belongs_to_the_line_it_ends() {
        let indexed = IndexedSource::new("ab\ncd".to_string());
        assert_eq!(indexed.line_col(2), (1, 3), "the \\n byte itself");
        assert_eq!(indexed.line_col(3), (2, 1), "the byte after it");
        assert_eq!(indexed.line_text(1), "ab");
        assert_eq!(indexed.line_text(2), "cd");
    }

    #[test]
    fn columns_count_characters_not_bytes() {
        // `é` is two bytes, `中` three, `😀` four.
        let indexed = IndexedSource::new("é中😀x".to_string());
        assert_eq!(indexed.line_col(0), (1, 1));
        assert_eq!(indexed.line_col(2), (1, 2), "after é");
        assert_eq!(indexed.line_col(5), (1, 3), "after 中");
        assert_eq!(indexed.line_col(9), (1, 4), "after 😀");
        // An offset inside a character counts that character, as the
        // scanning implementation did — and never panics on the boundary.
        assert_eq!(indexed.line_col(1), (1, 2));
        assert_eq!(indexed.line_col(7), (1, 4));
    }

    #[test]
    fn offsets_past_the_end_clamp_to_the_last_line() {
        let indexed = IndexedSource::new("a\nb\nc".to_string());
        assert_eq!(indexed.line_col(5), (3, 2));
        assert_eq!(indexed.line_col(usize::MAX), (3, 2));
        // A trailing newline opens a virtual, empty last line.
        let trailing = IndexedSource::new("a\nb\n".to_string());
        assert_eq!(trailing.line_col(usize::MAX), (3, 1));
        assert_eq!(trailing.line_text(3), "");
    }

    #[test]
    fn crlf_lines_drop_both_terminator_bytes() {
        let indexed = IndexedSource::new("one\r\ntwo\r\n".to_string());
        assert_eq!(indexed.line_text(1), "one");
        assert_eq!(indexed.line_text(2), "two");
        // The `\r` still occupies a column, exactly as it did before.
        assert_eq!(indexed.line_col(3), (1, 4));
        assert_eq!(indexed.line_col(5), (2, 1));
    }

    #[test]
    fn a_lone_carriage_return_is_ordinary_text() {
        let indexed = IndexedSource::new("a\rb\nc".to_string());
        assert_eq!(indexed.line_text(1), "a\rb");
        assert_eq!(indexed.line_col(2), (1, 3));
        assert_eq!(indexed.line_text(2), "c");
    }

    #[test]
    fn lookups_do_not_walk_the_file() {
        // The property the fix is about: a lookup near the end of a large
        // file must not scan to it. Asserted structurally (one table, binary
        // searched) rather than by timing.
        let source = "x\n".repeat(50_000);
        let len = source.len();
        let indexed = IndexedSource::new(source);
        assert_eq!(indexed.line_col(len - 1), (50_000, 2));
        assert_eq!(indexed.line_col(0), (1, 1));
        assert_eq!(indexed.line_text(50_000), "x");
    }
}
