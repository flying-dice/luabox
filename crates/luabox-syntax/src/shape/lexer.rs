//! Lossless lexer for `.luab` shape files (SHAPES-V2.md).
//!
//! Same invariant as the Lua lexer: tokens tile the input byte-for-byte.
//! The `.luab` surface is small — no string or numeric literals exist in the
//! grammar; anything outside it becomes an [`ShapeSyntaxKind::ERROR`] token
//! and lexing continues.

use super::ShapeSyntaxKind;

/// One lexed token: a kind and a byte length. Positions are implicit — the
/// tokens tile the input exactly, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShapeToken {
    pub kind: ShapeSyntaxKind,
    pub len: u32,
}

/// Lex `.luab` source. Never fails.
pub fn lex(text: &str) -> Vec<ShapeToken> {
    let mut lexer = Lexer {
        text,
        bytes: text.as_bytes(),
        pos: 0,
        tokens: Vec::new(),
    };
    while lexer.pos < lexer.bytes.len() {
        lexer.next_token();
    }
    lexer.tokens
}

struct Lexer<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    tokens: Vec<ShapeToken>,
}

impl Lexer<'_> {
    fn peek(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn push(&mut self, kind: ShapeSyntaxKind, start: usize) {
        let len = u32::try_from(self.pos - start).expect("token longer than u32::MAX bytes");
        debug_assert!(len > 0, "zero-length token");
        self.tokens.push(ShapeToken { kind, len });
    }

    fn next_token(&mut self) {
        let start = self.pos;
        let b = self.bytes[self.pos];
        match b {
            b' ' | b'\t' | b'\r' | b'\n' => {
                while matches!(self.peek(0), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                    self.pos += 1;
                }
                self.push(ShapeSyntaxKind::WHITESPACE, start);
            }
            b'-' if self.peek(1) == Some(b'-') => self.dash_comment(start),
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => self.ident_or_keyword(start),
            b'=' if self.peek(1) == Some(b'>') => {
                self.pos += 2;
                self.push(ShapeSyntaxKind::FAT_ARROW, start);
            }
            _ => self.symbol(start, b),
        }
    }

    /// Lua-convention comments (SHAPES-V2.md): `--` line comment, `---` doc
    /// comment (`----…` demotes back to plain, as LuaCATS does), or a
    /// `--[[ ]]` / `--[=[ ]=]` long-bracket block comment. A lone `-` is an
    /// ERROR — the grammar has no minus.
    fn dash_comment(&mut self, start: usize) {
        // `--[=*[` opens a long-bracket block comment.
        if self.peek(2) == Some(b'[') {
            let mut level = 0usize;
            while self.peek(3 + level) == Some(b'=') {
                level += 1;
            }
            if self.peek(3 + level) == Some(b'[') {
                self.pos += 4 + level;
                let kind = if self.skip_to_long_close(level) {
                    ShapeSyntaxKind::COMMENT
                } else {
                    ShapeSyntaxKind::ERROR // unterminated: ERROR to EOF
                };
                self.push(kind, start);
                return;
            }
        }
        let doc = self.peek(2) == Some(b'-') && self.peek(3) != Some(b'-');
        while !matches!(self.peek(0), None | Some(b'\n')) {
            self.pos += 1;
        }
        let kind = if doc {
            ShapeSyntaxKind::DOC_COMMENT
        } else {
            ShapeSyntaxKind::COMMENT
        };
        self.push(kind, start);
    }

    /// Advance past the matching `]=*]` of a level-`level` long bracket.
    /// Returns `false` (cursor at EOF) when unterminated.
    fn skip_to_long_close(&mut self, level: usize) -> bool {
        while self.pos < self.bytes.len() {
            if self.bytes[self.pos] == b']' {
                let mut eq = 0usize;
                while self.peek(1 + eq) == Some(b'=') {
                    eq += 1;
                }
                if eq == level && self.peek(1 + eq) == Some(b']') {
                    self.pos += 2 + eq;
                    return true;
                }
            }
            self.pos += 1;
        }
        false
    }

    fn ident_or_keyword(&mut self, start: usize) {
        while matches!(
            self.peek(0),
            Some(b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_')
        ) {
            self.pos += 1;
        }
        let kind = match &self.text[start..self.pos] {
            "type" => ShapeSyntaxKind::TYPE_KW,
            "export" => ShapeSyntaxKind::EXPORT_KW,
            "self" => ShapeSyntaxKind::SELF_KW,
            _ => ShapeSyntaxKind::IDENT,
        };
        self.push(kind, start);
    }

    fn symbol(&mut self, start: usize, b: u8) {
        let kind = match b {
            b'{' => ShapeSyntaxKind::L_BRACE,
            b'}' => ShapeSyntaxKind::R_BRACE,
            b'(' => ShapeSyntaxKind::L_PAREN,
            b')' => ShapeSyntaxKind::R_PAREN,
            b'<' => ShapeSyntaxKind::L_ANGLE,
            b'>' => ShapeSyntaxKind::R_ANGLE,
            b':' => ShapeSyntaxKind::COLON,
            b',' => ShapeSyntaxKind::COMMA,
            b'?' => ShapeSyntaxKind::QUESTION,
            b'|' => ShapeSyntaxKind::PIPE,
            b'&' => ShapeSyntaxKind::AMP,
            b'=' => ShapeSyntaxKind::EQ,
            b'.' => ShapeSyntaxKind::DOT,
            _ => {
                // Consume the full UTF-8 char so token boundaries stay char
                // boundaries.
                let ch_len = self.text[start..].chars().next().map_or(1, char::len_utf8);
                self.pos += ch_len;
                self.push(ShapeSyntaxKind::ERROR, start);
                return;
            }
        };
        self.pos += 1;
        self.push(kind, start);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ShapeSyntaxKind::*;

    fn check(text: &str) -> Vec<(ShapeSyntaxKind, &str)> {
        let tokens = lex(text);
        let mut out = Vec::new();
        let mut pos = 0;
        for t in tokens {
            let end = pos + t.len as usize;
            out.push((t.kind, &text[pos..end]));
            pos = end;
        }
        assert_eq!(pos, text.len(), "tokens must tile the input exactly");
        out
    }

    fn kinds(text: &str) -> Vec<ShapeSyntaxKind> {
        check(text).into_iter().map(|(k, _)| k).collect()
    }

    fn nontrivia(text: &str) -> Vec<ShapeSyntaxKind> {
        kinds(text).into_iter().filter(|k| !k.is_trivia()).collect()
    }

    #[test]
    fn spec_example_lexes_clean() {
        let src = r"
--- 2D geometry primitives.
type Point = { x: number, y: number, label?: string }
type Circle = { radius: number }
type Pair<T> = { first: T, second: T }

export type Shape = {
    area(self): number,
    perimeter(self): number,
}

export type Drawable = Shape & {
    draw(self, surface: Surface),
}
";
        let toks = check(src);
        assert!(
            !toks.iter().any(|(k, _)| *k == ERROR),
            "spec example must lex without errors: {toks:?}"
        );
    }

    #[test]
    fn type_def_tokens() {
        assert_eq!(
            nontrivia("type Point = { x: number, }"),
            vec![
                TYPE_KW, IDENT, EQ, L_BRACE, IDENT, COLON, IDENT, COMMA, R_BRACE
            ]
        );
    }

    #[test]
    fn export_and_optional_field() {
        assert_eq!(
            nontrivia("export type Bag = { n?: number }"),
            vec![
                EXPORT_KW, TYPE_KW, IDENT, EQ, L_BRACE, IDENT, QUESTION, COLON, IDENT, R_BRACE
            ]
        );
    }

    #[test]
    fn method_member() {
        assert_eq!(
            nontrivia("area(self): number"),
            vec![IDENT, L_PAREN, SELF_KW, R_PAREN, COLON, IDENT]
        );
    }

    #[test]
    fn intersection_and_union() {
        assert_eq!(
            nontrivia("Shape & { draw(self): string }"),
            vec![
                IDENT, AMP, L_BRACE, IDENT, L_PAREN, SELF_KW, R_PAREN, COLON, IDENT, R_BRACE
            ]
        );
        assert_eq!(nontrivia("number | string"), vec![IDENT, PIPE, IDENT]);
    }

    #[test]
    fn qualified_reference() {
        assert_eq!(
            nontrivia("export type Canvas = love.graphics.Canvas"),
            vec![EXPORT_KW, TYPE_KW, IDENT, EQ, IDENT, DOT, IDENT, DOT, IDENT]
        );
    }

    #[test]
    fn fn_type_fat_arrow() {
        assert_eq!(
            nontrivia("type F = (x: number) => string"),
            vec![
                TYPE_KW, IDENT, EQ, L_PAREN, IDENT, COLON, IDENT, R_PAREN, FAT_ARROW, IDENT
            ]
        );
        // `=` followed by non-`>` is plain EQ
        assert_eq!(nontrivia("= >"), vec![EQ, R_ANGLE]);
    }

    #[test]
    fn comments() {
        // Lua conventions (SHAPES-V2.md): `--` line, `---` doc, `--[[ ]]`
        // long-bracket blocks.
        assert_eq!(kinds("-- plain"), vec![COMMENT]);
        assert_eq!(kinds("--- doc"), vec![DOC_COMMENT]);
        // ---- is a plain comment again, as in LuaCATS
        assert_eq!(kinds("---- rule"), vec![COMMENT]);
        assert_eq!(kinds("--[[ a ]] b"), vec![COMMENT, WHITESPACE, IDENT]);
        // levelled long brackets contain unlevelled closers
        assert_eq!(kinds("--[=[ a ]] b ]=]"), vec![COMMENT]);
        // unterminated block comment is an ERROR to EOF
        assert_eq!(kinds("--[[ open"), vec![ERROR]);
        assert_eq!(kinds("--[=[ open ]]"), vec![ERROR]);
        // `--[` without a matching bracket is a plain line comment
        assert_eq!(kinds("--[ not a block"), vec![COMMENT]);
    }

    #[test]
    fn keywords_are_exact() {
        // `selfish` is an ident, not SELF_KW + ish; retired v1 keywords are
        // plain idents now.
        assert_eq!(
            nontrivia("selfish struct trait impl fn use for"),
            vec![IDENT, IDENT, IDENT, IDENT, IDENT, IDENT, IDENT]
        );
    }

    #[test]
    fn error_bytes() {
        // no numbers or strings in the grammar
        assert_eq!(nontrivia("x: 5"), vec![IDENT, COLON, ERROR]);
        assert_eq!(kinds("£"), vec![ERROR]);
        // lone slash, lone minus, and the retired thin arrow are errors
        assert_eq!(nontrivia("a / b"), vec![IDENT, ERROR, IDENT]);
        assert_eq!(nontrivia("a - b"), vec![IDENT, ERROR, IDENT]);
        assert_eq!(nontrivia("-> x"), vec![ERROR, R_ANGLE, IDENT]);
        // Rust-style v1 comments no longer lex as comments
        assert_eq!(nontrivia("// x"), vec![ERROR, ERROR, IDENT]);
    }
}
