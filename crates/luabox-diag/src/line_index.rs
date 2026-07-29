//! One source file, fetched once and indexed once, for the renderers.
//!
//! Every renderer needs the same two things per label: the 1-based line and
//! column of a byte offset, and the text of that line. Computing either by
//! walking the file from byte 0 costs O(offset), so a file with *n* findings
//! paid O(n x file size) to be reported — the human renderer spent 43 s on a
//! 1.8 MB file with 32 k diagnostics that `--format json` emitted in 0.5 s.
//!
//! [`IndexedSource`] owns the text and two tables built in one pass — the
//! byte offset of every line start, and the byte offset of every UTF-8
//! *continuation* byte — so [`IndexedSource::line_col`] binary-searches
//! instead of scanning and [`IndexedSource::line_text`] slices instead of
//! iterating.
//!
//! The continuation table is what makes the *column* cheap. Finding the line
//! was already a `partition_point`, but the column was then counted by
//! walking `char_indices` from the line start — O(column) per label, which on
//! a single-line file (minified source, generated code) is O(file) per label
//! and quadratic all over again: `check` took 71 s to render 10 k
//! diagnostics on a 377 kB one-line file that `--format json` emitted in
//! 0.5 s. A column counts *characters*, and a character is exactly one
//! non-continuation byte, so the count is
//! `(offset - line_start) - continuation_bytes_in(line_start..offset)` —
//! both terms `partition_point`s. The table costs nothing on ASCII input,
//! where it is empty.
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
    /// Byte offset of every UTF-8 continuation byte (`0b10xx_xxxx`), i.e.
    /// every byte that does *not* start a character. Empty for ASCII, which
    /// is the overwhelmingly common case; see the module docs for why this
    /// is the whole column story.
    continuations: Vec<usize>,
}

impl IndexedSource {
    /// Index `text` in one pass.
    pub(crate) fn new(text: String) -> Self {
        let mut starts = vec![0];
        let mut continuations = Vec::new();
        for (i, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(i + 1);
            } else if byte & 0b1100_0000 == 0b1000_0000 {
                continuations.push(i);
            }
        }
        IndexedSource {
            text,
            starts,
            continuations,
        }
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
    ///
    /// Both halves are binary searches, so the cost is O(log file) whatever
    /// the offset — never O(offset), which is what made a one-line file
    /// quadratic to report.
    pub(crate) fn line_col(&self, offset: usize) -> (usize, usize) {
        // Clamping first is what the scanning implementation did implicitly
        // by running out of characters.
        let offset = offset.min(self.text.len());
        // `starts` is sorted and starts at 0, so the number of starts at or
        // before `offset` is both >= 1 and exactly the 1-based line number.
        let line = self.starts.partition_point(|&start| start <= offset);
        let start = self
            .starts
            .get(line.saturating_sub(1))
            .copied()
            .unwrap_or(0);
        // Characters in `start..offset` = bytes in it, minus the ones that
        // only continue a character. An offset landing *inside* a character
        // therefore still counts it, matching the scan this replaced.
        let bytes = offset.saturating_sub(start);
        let continued = self.continuations.partition_point(|&i| i < offset)
            - self.continuations.partition_point(|&i| i < start);
        (line, bytes - continued + 1)
    }

    /// The text of a 1-based `line`, without its terminator.
    ///
    /// Matches `str::lines`: a trailing `\r` is dropped along with the `\n`, a
    /// lone `\r` is ordinary text, and a line past the end of the file — which
    /// includes the empty line a trailing newline opens — is `""`.
    pub(crate) fn line_text(&self, line: usize) -> &str {
        let range = self.line_range(line);
        let raw = self.text.get(range).unwrap_or("");
        raw.strip_suffix('\r').unwrap_or(raw)
    }

    /// Byte range of a 1-based `line`, terminator included — the exact range
    /// [`IndexedSource::line_text`] slices before it strips a trailing `\r`.
    ///
    /// The human renderer needs the *start* to place a label's byte offset
    /// within its line without re-deriving it from the column, which is the
    /// only way to window a long line in O(window) rather than O(line).
    /// An out-of-range line gives an empty range at the end of the file.
    pub(crate) fn line_range(&self, line: usize) -> std::ops::Range<usize> {
        let index = line.saturating_sub(1);
        let Some(&start) = self.starts.get(index) else {
            return self.text.len()..self.text.len();
        };
        let end = self
            .starts
            .get(index + 1)
            // The next line starts one past its opening `\n`; drop that `\n`.
            .map_or(self.text.len(), |&next| next.saturating_sub(1));
        start..end
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
    fn line_range_agrees_with_line_text_everywhere() {
        for source in TRICKY {
            let indexed = IndexedSource::new((*source).to_string());
            for line in 0..source.lines().count() + 4 {
                let range = indexed.line_range(line);
                let raw = source.get(range.clone()).unwrap_or("");
                assert_eq!(
                    raw.strip_suffix('\r').unwrap_or(raw),
                    indexed.line_text(line),
                    "source {source:?} at line {line}"
                );
                assert!(range.start <= range.end, "source {source:?} line {line}");
            }
        }
    }

    #[test]
    fn line_range_starts_where_the_line_starts() {
        let indexed = IndexedSource::new("ab\ncd\n".to_string());
        assert_eq!(indexed.line_range(1), 0..2);
        assert_eq!(indexed.line_range(2), 3..5);
        // Past the end: an empty range at the end of the file, never a
        // backwards one.
        assert_eq!(indexed.line_range(99), 6..6);
    }

    #[test]
    fn columns_on_one_long_line_do_not_walk_it() {
        // The property F3 is about: the column of an offset near the end of
        // a 400 kB *single* line must not be counted from the line start.
        // Asserted structurally (two `partition_point`s, no scan) plus the
        // values themselves, which the oracle test above pins in general.
        let source = "x".repeat(400_000);
        let len = source.len();
        let indexed = IndexedSource::new(source);
        assert!(
            indexed.continuations.is_empty(),
            "an ASCII file must not pay for the continuation table"
        );
        assert_eq!(indexed.line_col(len - 1), (1, len));
        assert_eq!(indexed.line_col(0), (1, 1));
        // Multi-byte, so the table is actually exercised at scale.
        let wide = "中".repeat(100_000);
        let indexed = IndexedSource::new(wide);
        assert_eq!(indexed.continuations.len(), 200_000);
        assert_eq!(indexed.line_col(299_997), (1, 100_000));
        assert_eq!(indexed.line_col(usize::MAX), (1, 100_001));
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
