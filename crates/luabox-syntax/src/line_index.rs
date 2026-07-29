//! A line-start table for one source string: byte offset → 1-based line.
//!
//! Diagnostics, suppression comments and `---@diagnostic` directives are all
//! keyed by *line*, while everything inside the toolchain carries *byte
//! offsets*. Converting one to the other by counting newlines from byte 0
//! costs O(offset) per lookup, which is invisible on a file with one finding
//! and quadratic on a file with thousands (a 100-kLOC file with 32 k lint
//! findings spent ~30 s doing nothing else). Building the table once per file
//! and binary-searching it makes every lookup O(log lines).
//!
//! This lives in the syntax crate because it is the lowest crate every
//! line-reporting consumer (`luabox-lint`, `luabox-types`) already depends
//! on. It is deliberately about *source text*, not trees: no rowan types
//! appear in its API.

/// Line-start byte offsets for one source string.
///
/// Build it once per file, then call [`LineIndex::line_of`] per lookup.
///
/// # Examples
///
/// ```
/// use luabox_syntax::LineIndex;
///
/// let index = LineIndex::new("local x = 1\nprint(x)\n");
/// assert_eq!(index.line_of(0), 1);
/// assert_eq!(index.line_of(12), 2);
/// // Offsets past the end clamp to the last line.
/// assert_eq!(index.line_of(9_999), 3);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex {
    /// Byte offset of the start of each line. Always begins with `0`, so the
    /// vector is never empty and its length is the number of lines.
    starts: Vec<usize>,
}

impl LineIndex {
    /// Build the index for `source` in one pass.
    #[must_use]
    pub fn new(source: &str) -> Self {
        let starts = std::iter::once(0)
            .chain(
                source
                    .bytes()
                    .enumerate()
                    .filter(|&(_, b)| b == b'\n')
                    .map(|(i, _)| i + 1),
            )
            .collect();
        LineIndex { starts }
    }

    /// The 1-based line number containing `offset`.
    ///
    /// A `\n` byte belongs to the line it terminates; offsets past the end of
    /// the source clamp to the last line. `\r\n` needs no special case — only
    /// the `\n` starts a new line, so the `\r` sits on the line it ends.
    #[must_use]
    pub fn line_of(&self, offset: usize) -> usize {
        // `starts` is sorted and starts at 0, so the count of starts at or
        // before `offset` is both >= 1 and exactly the 1-based line number.
        self.starts.partition_point(|&start| start <= offset)
    }

    /// The number of lines. A source with no trailing newline and the empty
    /// source both have at least one line.
    #[must_use]
    pub fn len(&self) -> usize {
        self.starts.len()
    }

    /// Always `false` — every source has at least one line. Present because
    /// clippy (rightly) asks for it next to [`LineIndex::len`].
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.starts.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The implementation this index replaced: count the newlines before
    /// `offset`, rescanning from byte 0 every time. Kept as the oracle so
    /// the fast path is pinned to the behaviour callers already relied on.
    fn line_of_by_counting(source: &str, offset: usize) -> usize {
        1 + source
            .bytes()
            .take(offset.min(source.len()))
            .filter(|&b| b == b'\n')
            .count()
    }

    #[test]
    fn matches_the_counting_implementation_on_tricky_inputs() {
        for source in [
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
        ] {
            let index = LineIndex::new(source);
            // Every byte offset, plus a few past the end.
            for offset in 0..=source.len() + 3 {
                assert_eq!(
                    index.line_of(offset),
                    line_of_by_counting(source, offset),
                    "source {source:?} at offset {offset}"
                );
            }
        }
    }

    #[test]
    fn empty_source_is_one_line() {
        let index = LineIndex::new("");
        assert_eq!(index.len(), 1);
        assert!(!index.is_empty());
        assert_eq!(index.line_of(0), 1);
        assert_eq!(index.line_of(usize::MAX), 1);
    }

    #[test]
    fn a_newline_belongs_to_the_line_it_ends() {
        let index = LineIndex::new("ab\ncd");
        assert_eq!(index.line_of(2), 1, "the \\n byte itself");
        assert_eq!(index.line_of(3), 2, "the byte after it");
        assert_eq!(index.len(), 2);
    }

    #[test]
    fn offsets_past_the_end_clamp_to_the_last_line() {
        let index = LineIndex::new("a\nb\nc");
        assert_eq!(index.line_of(5), 3);
        assert_eq!(index.line_of(usize::MAX), 3);
        // A trailing newline opens a (virtual, empty) last line.
        assert_eq!(LineIndex::new("a\nb\n").line_of(usize::MAX), 3);
    }

    #[test]
    fn lookups_do_not_depend_on_the_offset_distance() {
        // The property the fix is about: a lookup near the end of a large
        // file must not walk the file. Asserted structurally (the table is
        // built once and binary-searched) rather than by timing.
        let source = "x\n".repeat(50_000);
        let index = LineIndex::new(&source);
        assert_eq!(index.len(), 50_001);
        assert_eq!(index.line_of(source.len() - 1), 50_000);
        assert_eq!(index.line_of(0), 1);
    }
}
