//! Byte-offset ↔ LSP position conversion.
//!
//! LSP positions are `(line, character)` where `character` counts **UTF-16
//! code units** (the protocol default encoding, which this server
//! advertises implicitly by not negotiating another one). Internally
//! everything — rowan ranges, `luabox_diag::Span`s, LuaCATS spans — is UTF-8
//! byte offsets, so every boundary crossing goes through a [`LineIndex`].
//!
//! The index owns a copy of the text it was built from: conversions need the
//! line contents to count UTF-16 units, and per-request construction is cheap
//! (one pass over the file).

use lsp_types::Position;

/// A line-start table over one file's text, plus the text itself.
#[derive(Debug, Clone)]
pub struct LineIndex {
    text: String,
    /// Byte offset of the start of each line. `line_starts[0] == 0`.
    line_starts: Vec<usize>,
}

impl LineIndex {
    /// Build the index for `text`.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let mut line_starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self { text, line_starts }
    }

    /// The indexed text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Convert a byte offset to an LSP (line, UTF-16 column) position.
    /// Offsets past the end clamp to the end of the text; offsets that fall
    /// inside a multi-byte character snap back to its start.
    #[must_use]
    pub fn position(&self, offset: usize) -> Position {
        let offset = clamp_to_char_boundary(&self.text, offset);
        let line = self
            .line_starts
            .partition_point(|&start| start <= offset)
            .saturating_sub(1);
        let line_start = self.line_starts[line];
        #[expect(
            clippy::string_slice,
            reason = "offset is clamped to a char boundary above; line_start follows a `\\n` byte, so both are valid boundaries"
        )]
        let col_utf16: usize = self.text[line_start..offset]
            .chars()
            .map(char::len_utf16)
            .sum();
        Position {
            line: u32::try_from(line).unwrap_or(u32::MAX),
            character: u32::try_from(col_utf16).unwrap_or(u32::MAX),
        }
    }

    /// Convert an LSP position back to a byte offset. Positions past the end
    /// of a line clamp to the line's end (before its `\n`); lines past the
    /// end of the file clamp to the end of the text.
    #[must_use]
    pub fn offset(&self, position: Position) -> usize {
        let line = position.line as usize;
        let Some(&line_start) = self.line_starts.get(line) else {
            return self.text.len();
        };
        let line_end = self
            .line_starts
            .get(line + 1)
            .map_or(self.text.len(), |&next| next - 1);
        #[expect(
            clippy::string_slice,
            reason = "line_start and line_end come from the `\\n`-delimited line-start table, so both are char boundaries"
        )]
        let line_text = &self.text[line_start..line_end];

        let mut units_left = position.character as usize;
        for (i, ch) in line_text.char_indices() {
            if ch == '\r' && i + 1 == line_text.len() {
                // Don't land between `\r` and `\n`.
                return line_start + i;
            }
            let width = ch.len_utf16();
            if units_left < width {
                return line_start + i;
            }
            units_left -= width;
        }
        line_end
    }

    /// Convert a byte range to an LSP range.
    #[must_use]
    pub fn range(&self, range: std::ops::Range<usize>) -> lsp_types::Range {
        lsp_types::Range {
            start: self.position(range.start),
            end: self.position(range.end),
        }
    }
}

/// The nearest char boundary at or before `offset` (clamped to the length).
fn clamp_to_char_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn ascii_positions_roundtrip() {
        let index = LineIndex::new("local x = 1\nprint(x)\n");
        assert_eq!(index.position(0), pos(0, 0));
        assert_eq!(index.position(6), pos(0, 6));
        assert_eq!(index.position(12), pos(1, 0));
        assert_eq!(index.position(18), pos(1, 6));
        for offset in [0, 6, 11, 12, 18, 20] {
            assert_eq!(index.offset(index.position(offset)), offset);
        }
    }

    #[test]
    fn multibyte_chars_count_one_utf16_unit() {
        // 'é' is 2 bytes / 1 UTF-16 unit; '中' is 3 bytes / 1 UTF-16 unit.
        let text = "é中x";
        let index = LineIndex::new(text);
        assert_eq!(index.position(0), pos(0, 0));
        assert_eq!(index.position(2), pos(0, 1)); // after é
        assert_eq!(index.position(5), pos(0, 2)); // after 中
        assert_eq!(index.offset(pos(0, 1)), 2);
        assert_eq!(index.offset(pos(0, 2)), 5);
    }

    #[test]
    fn surrogate_pairs_count_two_utf16_units() {
        // '😀' is 4 bytes / 2 UTF-16 units (a surrogate pair).
        let text = "a😀b";
        let index = LineIndex::new(text);
        assert_eq!(index.position(1), pos(0, 1)); // before 😀
        assert_eq!(index.position(5), pos(0, 3)); // after 😀
        assert_eq!(index.offset(pos(0, 3)), 5);
        // A position landing *inside* the pair snaps to the char start.
        assert_eq!(index.offset(pos(0, 2)), 1);
        // A byte offset inside the char snaps back too.
        assert_eq!(index.position(3), pos(0, 1));
    }

    #[test]
    fn out_of_range_positions_clamp() {
        let index = LineIndex::new("ab\ncd");
        assert_eq!(index.offset(pos(0, 99)), 2); // end of line 0
        assert_eq!(index.offset(pos(1, 99)), 5); // end of text
        assert_eq!(index.offset(pos(9, 0)), 5); // line past the end
        assert_eq!(index.position(999), pos(1, 2));
    }

    #[test]
    fn crlf_line_endings() {
        let index = LineIndex::new("ab\r\ncd\r\n");
        assert_eq!(index.position(4), pos(1, 0));
        assert_eq!(index.offset(pos(0, 2)), 2); // at the \r
        // Clamping a too-large column never lands between \r and \n.
        assert_eq!(index.offset(pos(0, 99)), 2);
        assert_eq!(index.offset(pos(1, 0)), 4);
    }

    #[test]
    fn empty_text_and_trailing_newline() {
        let empty = LineIndex::new("");
        assert_eq!(empty.position(0), pos(0, 0));
        assert_eq!(empty.offset(pos(0, 0)), 0);
        let trailing = LineIndex::new("x\n");
        // The virtual line after a trailing newline exists and is empty.
        assert_eq!(trailing.position(2), pos(1, 0));
        assert_eq!(trailing.offset(pos(1, 0)), 2);
    }
}
