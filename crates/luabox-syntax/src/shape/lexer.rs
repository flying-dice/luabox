//! Lossless lexer for `.lb` shape files (SHAPES.md §2–3).
//!
//! Same invariant as the Lua lexer: tokens tile the input byte-for-byte.
//! The `.lb` surface is small — no string or numeric literals exist in the
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

/// Lex `.lb` source. Never fails.
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
            b'/' => self.slash(start),
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => self.ident_or_keyword(start),
            b'-' if self.peek(1) == Some(b'>') => {
                self.pos += 2;
                self.push(ShapeSyntaxKind::ARROW, start);
            }
            b'.' => {
                if self.peek(1) == Some(b'.') {
                    self.pos += 2;
                    self.push(ShapeSyntaxKind::DOT_DOT, start);
                } else {
                    self.pos += 1;
                    self.push(ShapeSyntaxKind::DOT, start);
                }
            }
            _ => self.symbol(start, b),
        }
    }

    /// `//` line comment, `///` doc comment (`////…` demotes back to plain,
    /// as in Rust), or `/* */` block comment with nesting. A lone `/` is an
    /// ERROR — the grammar has no division.
    fn slash(&mut self, start: usize) {
        match self.peek(1) {
            Some(b'/') => {
                let doc = self.peek(2) == Some(b'/') && self.peek(3) != Some(b'/');
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
            Some(b'*') => {
                self.pos += 2;
                let mut depth = 1usize;
                while depth > 0 {
                    match (self.peek(0), self.peek(1)) {
                        (None, _) => break, // unterminated: ERROR to EOF
                        (Some(b'/'), Some(b'*')) => {
                            depth += 1;
                            self.pos += 2;
                        }
                        (Some(b'*'), Some(b'/')) => {
                            depth -= 1;
                            self.pos += 2;
                        }
                        _ => self.pos += 1,
                    }
                }
                let kind = if depth == 0 {
                    ShapeSyntaxKind::COMMENT
                } else {
                    ShapeSyntaxKind::ERROR
                };
                self.push(kind, start);
            }
            _ => {
                self.pos += 1;
                self.push(ShapeSyntaxKind::ERROR, start);
            }
        }
    }

    fn ident_or_keyword(&mut self, start: usize) {
        while matches!(
            self.peek(0),
            Some(b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_')
        ) {
            self.pos += 1;
        }
        let kind = match &self.text[start..self.pos] {
            "struct" => ShapeSyntaxKind::STRUCT_KW,
            "trait" => ShapeSyntaxKind::TRAIT_KW,
            "impl" => ShapeSyntaxKind::IMPL_KW,
            "for" => ShapeSyntaxKind::FOR_KW,
            "fn" => ShapeSyntaxKind::FN_KW,
            "self" => ShapeSyntaxKind::SELF_KW,
            "type" => ShapeSyntaxKind::TYPE_KW,
            "use" => ShapeSyntaxKind::USE_KW,
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
            b';' => ShapeSyntaxKind::SEMICOLON,
            b',' => ShapeSyntaxKind::COMMA,
            b'?' => ShapeSyntaxKind::QUESTION,
            b'|' => ShapeSyntaxKind::PIPE,
            b'+' => ShapeSyntaxKind::PLUS,
            b'=' => ShapeSyntaxKind::EQ,
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
/// 2D geometry primitives.
struct Point { x: number, y: number, label: string? }

trait Shape {
    fn area(self) -> number;
    fn perimeter(self) -> number;
}

trait Drawable: Shape {
    fn draw(self, surface: Surface);
}

struct Circle { radius: number }
impl Shape for Circle;

struct Pair<T> { first: T, second: T }
";
        let toks = check(src);
        assert!(
            !toks.iter().any(|(k, _)| *k == ERROR),
            "spec example must lex without errors: {toks:?}"
        );
    }

    #[test]
    fn struct_tokens() {
        assert_eq!(
            nontrivia("struct Point { x: number, }"),
            vec![
                STRUCT_KW, IDENT, L_BRACE, IDENT, COLON, IDENT, COMMA, R_BRACE
            ]
        );
    }

    #[test]
    fn open_marker_and_optional() {
        assert_eq!(
            nontrivia("struct Bag { n: number? .. }"),
            vec![
                STRUCT_KW, IDENT, L_BRACE, IDENT, COLON, IDENT, QUESTION, DOT_DOT, R_BRACE
            ]
        );
    }

    #[test]
    fn trait_fn_signature() {
        assert_eq!(
            nontrivia("fn area(self) -> number;"),
            vec![
                FN_KW, IDENT, L_PAREN, SELF_KW, R_PAREN, ARROW, IDENT, SEMICOLON
            ]
        );
    }

    #[test]
    fn supertraits_and_impl() {
        assert_eq!(
            nontrivia("trait Drawable: Shape + Sized {}"),
            vec![TRAIT_KW, IDENT, COLON, IDENT, PLUS, IDENT, L_BRACE, R_BRACE]
        );
        assert_eq!(
            nontrivia("impl Shape for Circle;"),
            vec![IMPL_KW, IDENT, FOR_KW, IDENT, SEMICOLON]
        );
    }

    #[test]
    fn alias_and_use() {
        assert_eq!(
            nontrivia("type Points = Vec<Point>;"),
            vec![
                TYPE_KW, IDENT, EQ, IDENT, L_ANGLE, IDENT, R_ANGLE, SEMICOLON
            ]
        );
        assert_eq!(nontrivia("use geometry;"), vec![USE_KW, IDENT, SEMICOLON]);
    }

    #[test]
    fn comments() {
        assert_eq!(kinds("// plain"), vec![COMMENT]);
        assert_eq!(kinds("/// doc"), vec![DOC_COMMENT]);
        // //// is a plain comment again, as in Rust
        assert_eq!(kinds("//// rule"), vec![COMMENT]);
        assert_eq!(kinds("/* a /* nested */ b */"), vec![COMMENT]);
        // unterminated block comment is an ERROR to EOF
        assert_eq!(kinds("/* open"), vec![ERROR]);
    }

    #[test]
    fn keywords_are_exact() {
        // `selfish` is an ident, not SELF_KW + ish
        assert_eq!(nontrivia("selfish structs"), vec![IDENT, IDENT]);
    }

    #[test]
    fn error_bytes() {
        // no numbers or strings in the grammar
        assert_eq!(nontrivia("x: 5"), vec![IDENT, COLON, ERROR]);
        assert_eq!(kinds("£"), vec![ERROR]);
        // lone slash and lone minus are errors
        assert_eq!(nontrivia("a / b"), vec![IDENT, ERROR, IDENT]);
        assert_eq!(nontrivia("a - b"), vec![IDENT, ERROR, IDENT]);
    }
}
