//! Lossless lexer for every supported dialect.
//!
//! Invariant: the concatenation of all token texts is byte-identical to the
//! input — nothing is skipped, normalized, or merged. Whitespace and comments
//! are ordinary (trivia) tokens.
//!
//! The lexer accepts the *union* of all dialect lexical grammars wherever
//! that is unambiguous (`::`, `//`, `<<` lex the same everywhere; a 5.1
//! validator diagnoses them later with a proper span). Only two things are
//! dialect-gated at token level because they change token *boundaries*:
//! the `goto` keyword and Luau-only syntax (backtick interpolated strings,
//! compound assignment, `->`, `?`, LuaJIT number suffixes on the JIT side).
//!
//! Unterminated long strings/comments lex to end-of-input with their natural
//! kind (the validator sees the missing terminator in the text); an
//! unterminated *short* string becomes an [`SyntaxKind::ERROR`] token ending
//! at the newline, so the rest of the line still lexes normally.

use crate::{Dialect, SyntaxKind};

/// One lexed token: a kind and a byte length. Positions are implicit — the
/// tokens tile the input exactly, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: SyntaxKind,
    pub len: u32,
}

/// Lex `text` under `dialect`. Never fails: unrecognized bytes become
/// [`SyntaxKind::ERROR`] tokens and lexing continues.
pub fn lex(text: &str, dialect: Dialect) -> Vec<Token> {
    let mut lexer = Lexer {
        text,
        bytes: text.as_bytes(),
        pos: 0,
        dialect,
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
    dialect: Dialect,
    tokens: Vec<Token>,
}

impl Lexer<'_> {
    fn peek(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn push(&mut self, kind: SyntaxKind, start: usize) {
        let len = u32::try_from(self.pos - start).expect("token longer than u32::MAX bytes");
        debug_assert!(len > 0, "zero-length token");
        self.tokens.push(Token { kind, len });
    }

    fn next_token(&mut self) {
        let start = self.pos;
        let b = self.bytes[self.pos];
        match b {
            b' ' | b'\t' | b'\r' | b'\n' | 0x0B | 0x0C => {
                while matches!(
                    self.peek(0),
                    Some(b' ' | b'\t' | b'\r' | b'\n' | 0x0B | 0x0C)
                ) {
                    self.pos += 1;
                }
                self.push(SyntaxKind::WHITESPACE, start);
            }
            b'-' => self.minus(start),
            b'[' => {
                if let Some(level) = self.long_bracket_level() {
                    self.long_bracket(start, level, SyntaxKind::STRING);
                } else {
                    self.pos += 1;
                    self.push(SyntaxKind::L_BRACKET, start);
                }
            }
            b'\'' | b'"' => self.short_string(start, b),
            b'`' if self.dialect.is_luau() => self.interp_string(start),
            b'0'..=b'9' => self.number(start),
            b'.' => {
                if matches!(self.peek(1), Some(b'0'..=b'9')) {
                    self.number(start);
                } else {
                    self.dots(start);
                }
            }
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => self.ident_or_keyword(start),
            _ => self.symbol(start, b),
        }
    }

    fn minus(&mut self, start: usize) {
        match self.peek(1) {
            Some(b'-') => {
                self.pos += 2;
                if let Some(level) = self.long_bracket_level() {
                    self.long_bracket(start, level, SyntaxKind::COMMENT);
                } else {
                    while !matches!(self.peek(0), None | Some(b'\n')) {
                        self.pos += 1;
                    }
                    self.push(SyntaxKind::COMMENT, start);
                }
            }
            Some(b'>') if self.dialect.is_luau() => {
                self.pos += 2;
                self.push(SyntaxKind::THIN_ARROW, start);
            }
            Some(b'=') if self.dialect.is_luau() => {
                self.pos += 2;
                self.push(SyntaxKind::MINUS_EQ, start);
            }
            _ => {
                self.pos += 1;
                self.push(SyntaxKind::MINUS, start);
            }
        }
    }

    /// At `self.pos` sitting on `[`: if this opens a long bracket
    /// (`[[`, `[=[`, `[==[`, …) return its level without consuming.
    fn long_bracket_level(&self) -> Option<usize> {
        if self.peek(0) != Some(b'[') {
            return None;
        }
        let mut level = 0;
        while self.peek(1 + level) == Some(b'=') {
            level += 1;
        }
        (self.peek(1 + level) == Some(b'[')).then_some(level)
    }

    /// Consume `[=*[ ... ]=*]` (opening already verified). Unterminated:
    /// consumes to end of input, keeping `kind` (validator diagnoses).
    fn long_bracket(&mut self, start: usize, level: usize, kind: SyntaxKind) {
        self.pos += 2 + level; // [=*[
        loop {
            match self.peek(0) {
                None => break,
                Some(b']') => {
                    let mut eqs = 0;
                    while self.peek(1 + eqs) == Some(b'=') {
                        eqs += 1;
                    }
                    if eqs == level && self.peek(1 + eqs) == Some(b']') {
                        self.pos += 2 + level;
                        break;
                    }
                    self.pos += 1;
                }
                Some(_) => self.pos += 1,
            }
        }
        self.push(kind, start);
    }

    /// `'...'` / `"..."` with escapes. An unescaped newline or end of input
    /// before the closing quote yields an ERROR token (newline excluded).
    fn short_string(&mut self, start: usize, quote: u8) {
        self.pos += 1;
        loop {
            match self.peek(0) {
                None | Some(b'\n') => {
                    self.push(SyntaxKind::ERROR, start);
                    return;
                }
                Some(b'\\') => {
                    // `\z` skips following whitespace (newlines included);
                    // any other escape consumes exactly one char.
                    if matches!(self.peek(1), Some(b'z' | b'Z')) {
                        self.pos += 2;
                        while matches!(
                            self.peek(0),
                            Some(b' ' | b'\t' | b'\r' | b'\n' | 0x0B | 0x0C)
                        ) {
                            self.pos += 1;
                        }
                    } else if self.peek(1).is_some() {
                        self.pos += 2;
                    } else {
                        self.pos += 1;
                    }
                }
                Some(b) if b == quote => {
                    self.pos += 1;
                    self.push(SyntaxKind::STRING, start);
                    return;
                }
                Some(_) => self.pos += 1,
            }
        }
    }

    /// Luau `` `text {expr}` `` — lexed as a single token; the parser
    /// re-lexes the `{}` holes. Brace depth is tracked so `{ {a=1} }` holes
    /// don't end the string early. Unterminated → ERROR to end of line.
    fn interp_string(&mut self, start: usize) {
        self.pos += 1;
        let mut depth = 0usize;
        loop {
            match self.peek(0) {
                None | Some(b'\n') => {
                    self.push(SyntaxKind::ERROR, start);
                    return;
                }
                Some(b'\\') => self.pos += if self.peek(1).is_some() { 2 } else { 1 },
                Some(b'{') => {
                    depth += 1;
                    self.pos += 1;
                }
                Some(b'}') => {
                    depth = depth.saturating_sub(1);
                    self.pos += 1;
                }
                Some(b'`') if depth == 0 => {
                    self.pos += 1;
                    self.push(SyntaxKind::INTERP_STRING, start);
                    return;
                }
                Some(_) => self.pos += 1,
            }
        }
    }

    fn number(&mut self, start: usize) {
        let is_digit_sep = |b: u8| b == b'_' && self.dialect.is_luau();
        if self.peek(0) == Some(b'0') && matches!(self.peek(1), Some(b'x' | b'X')) {
            self.pos += 2;
            while self
                .peek(0)
                .is_some_and(|b| b.is_ascii_hexdigit() || is_digit_sep(b))
            {
                self.pos += 1;
            }
            if self.peek(0) == Some(b'.') {
                self.pos += 1;
                while self.peek(0).is_some_and(|b| b.is_ascii_hexdigit()) {
                    self.pos += 1;
                }
            }
            // Hex float binary exponent (5.2+, LuaJIT).
            if matches!(self.peek(0), Some(b'p' | b'P')) {
                self.exponent();
            }
        } else if self.peek(0) == Some(b'0')
            && matches!(self.peek(1), Some(b'b' | b'B'))
            && self.dialect.is_luau()
        {
            self.pos += 2;
            while self
                .peek(0)
                .is_some_and(|b| matches!(b, b'0' | b'1' | b'_'))
            {
                self.pos += 1;
            }
        } else {
            while self
                .peek(0)
                .is_some_and(|b| b.is_ascii_digit() || is_digit_sep(b))
            {
                self.pos += 1;
            }
            if self.peek(0) == Some(b'.') && self.peek(1) != Some(b'.') {
                // `1..2` is NUMBER DOT_DOT NUMBER, not a malformed float.
                self.pos += 1;
                while self
                    .peek(0)
                    .is_some_and(|b| b.is_ascii_digit() || is_digit_sep(b))
                {
                    self.pos += 1;
                }
            }
            if matches!(self.peek(0), Some(b'e' | b'E')) {
                self.exponent();
            }
        }
        if self.dialect == Dialect::LuaJit {
            self.luajit_suffix();
        }
        self.push(SyntaxKind::NUMBER, start);
    }

    /// `e`/`E`/`p`/`P` exponent: consumed only if digits (after an optional
    /// sign) actually follow, so `1e` lexes as NUMBER(1e)… no — as
    /// NUMBER(1) IDENT(e), matching how `print(1e)` should diagnose.
    fn exponent(&mut self) {
        let sign = usize::from(matches!(self.peek(1), Some(b'+' | b'-')));
        if self.peek(1 + sign).is_some_and(|b| b.is_ascii_digit()) {
            self.pos += 1 + sign;
            while self.peek(0).is_some_and(|b| b.is_ascii_digit()) {
                self.pos += 1;
            }
        }
    }

    /// LuaJIT `LL`/`ULL` (any case) and imaginary `i`/`I` suffixes.
    fn luajit_suffix(&mut self) {
        let lower = |i: usize| self.peek(i).map(|b| b.to_ascii_lowercase());
        if lower(0) == Some(b'u') && lower(1) == Some(b'l') && lower(2) == Some(b'l') {
            self.pos += 3;
        } else if lower(0) == Some(b'l') && lower(1) == Some(b'l') {
            self.pos += 2;
        } else if lower(0) == Some(b'i')
            && !matches!(
                self.peek(1),
                Some(b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_')
            )
        {
            self.pos += 1;
        }
    }

    fn dots(&mut self, start: usize) {
        if self.peek(1) == Some(b'.') {
            if self.peek(2) == Some(b'.') {
                self.pos += 3;
                self.push(SyntaxKind::DOT_DOT_DOT, start);
            } else if self.peek(2) == Some(b'=') && self.dialect.is_luau() {
                self.pos += 3;
                self.push(SyntaxKind::DOT_DOT_EQ, start);
            } else {
                self.pos += 2;
                self.push(SyntaxKind::DOT_DOT, start);
            }
        } else {
            self.pos += 1;
            self.push(SyntaxKind::DOT, start);
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
            "and" => SyntaxKind::AND_KW,
            "break" => SyntaxKind::BREAK_KW,
            "do" => SyntaxKind::DO_KW,
            "else" => SyntaxKind::ELSE_KW,
            "elseif" => SyntaxKind::ELSEIF_KW,
            "end" => SyntaxKind::END_KW,
            "false" => SyntaxKind::FALSE_KW,
            "for" => SyntaxKind::FOR_KW,
            "function" => SyntaxKind::FUNCTION_KW,
            "goto" if self.dialect.has_goto() => SyntaxKind::GOTO_KW,
            "if" => SyntaxKind::IF_KW,
            "in" => SyntaxKind::IN_KW,
            "local" => SyntaxKind::LOCAL_KW,
            "nil" => SyntaxKind::NIL_KW,
            "not" => SyntaxKind::NOT_KW,
            "or" => SyntaxKind::OR_KW,
            "repeat" => SyntaxKind::REPEAT_KW,
            "return" => SyntaxKind::RETURN_KW,
            "then" => SyntaxKind::THEN_KW,
            "true" => SyntaxKind::TRUE_KW,
            "until" => SyntaxKind::UNTIL_KW,
            "while" => SyntaxKind::WHILE_KW,
            _ => SyntaxKind::IDENT,
        };
        self.push(kind, start);
    }

    fn symbol(&mut self, start: usize, b: u8) {
        let luau = self.dialect.is_luau();
        let (kind, len) = match (b, self.peek(1), self.peek(2)) {
            (b'/', Some(b'/'), Some(b'=')) if luau => (SyntaxKind::SLASH_SLASH_EQ, 3),
            (b'/', Some(b'/'), _) => (SyntaxKind::SLASH_SLASH, 2),
            (b'/', Some(b'='), _) if luau => (SyntaxKind::SLASH_EQ, 2),
            (b'/', _, _) => (SyntaxKind::SLASH, 1),
            (b'<', Some(b'<'), _) => (SyntaxKind::LT_LT, 2),
            (b'<', Some(b'='), _) => (SyntaxKind::LT_EQ, 2),
            (b'<', _, _) => (SyntaxKind::LT, 1),
            (b'>', Some(b'>'), _) => (SyntaxKind::GT_GT, 2),
            (b'>', Some(b'='), _) => (SyntaxKind::GT_EQ, 2),
            (b'>', _, _) => (SyntaxKind::GT, 1),
            (b'=', Some(b'='), _) => (SyntaxKind::EQ_EQ, 2),
            (b'=', _, _) => (SyntaxKind::EQ, 1),
            (b'~', Some(b'='), _) => (SyntaxKind::TILDE_EQ, 2),
            (b'~', _, _) => (SyntaxKind::TILDE, 1),
            (b':', Some(b':'), _) => (SyntaxKind::COLON_COLON, 2),
            (b':', _, _) => (SyntaxKind::COLON, 1),
            (b'+', Some(b'='), _) if luau => (SyntaxKind::PLUS_EQ, 2),
            (b'+', _, _) => (SyntaxKind::PLUS, 1),
            (b'*', Some(b'='), _) if luau => (SyntaxKind::STAR_EQ, 2),
            (b'*', _, _) => (SyntaxKind::STAR, 1),
            (b'%', Some(b'='), _) if luau => (SyntaxKind::PERCENT_EQ, 2),
            (b'%', _, _) => (SyntaxKind::PERCENT, 1),
            (b'^', Some(b'='), _) if luau => (SyntaxKind::CARET_EQ, 2),
            (b'^', _, _) => (SyntaxKind::CARET, 1),
            (b'#', _, _) => (SyntaxKind::HASH, 1),
            (b'&', _, _) => (SyntaxKind::AMP, 1),
            (b'|', _, _) => (SyntaxKind::PIPE, 1),
            (b'(', _, _) => (SyntaxKind::L_PAREN, 1),
            (b')', _, _) => (SyntaxKind::R_PAREN, 1),
            (b'{', _, _) => (SyntaxKind::L_BRACE, 1),
            (b'}', _, _) => (SyntaxKind::R_BRACE, 1),
            (b']', _, _) => (SyntaxKind::R_BRACKET, 1),
            (b';', _, _) => (SyntaxKind::SEMICOLON, 1),
            (b',', _, _) => (SyntaxKind::COMMA, 1),
            (b'?', _, _) if luau => (SyntaxKind::QUESTION, 1),
            _ => {
                // Unrecognized byte: consume the full UTF-8 char so token
                // boundaries stay char boundaries.
                let ch_len = self.text[start..].chars().next().map_or(1, char::len_utf8);
                (SyntaxKind::ERROR, ch_len)
            }
        };
        self.pos += len;
        self.push(kind, start);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SyntaxKind::*;

    /// Lex and return (kind, text) pairs, asserting losslessness.
    fn check(text: &str, dialect: Dialect) -> Vec<(SyntaxKind, &str)> {
        let tokens = lex(text, dialect);
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

    fn kinds(text: &str, dialect: Dialect) -> Vec<SyntaxKind> {
        check(text, dialect).into_iter().map(|(k, _)| k).collect()
    }

    #[test]
    fn lossless_over_mixed_corpus() {
        let corpus = [
            "local x = 1 + 2 -- neat\n",
            "s = \"he said \\\"hi\\\"\" .. 'and \\'bye\\''",
            "t = { [1] = 2, a = 'b'; c }\n#t",
            "--[==[ long\ncomment ]==]\nreturn ...",
            "x = 0x1F.8p-2 ~= 1e10 // 3 << 2",
            "::top:: goto top",
            "weird = £ § unterminated'",
        ];
        for src in corpus {
            for d in [
                Dialect::Lua51,
                Dialect::Lua52,
                Dialect::Lua53,
                Dialect::Lua54,
                Dialect::LuaJit,
                Dialect::Luau,
            ] {
                check(src, d); // asserts tiling internally
            }
        }
    }

    #[test]
    fn goto_is_dialect_gated() {
        assert_eq!(kinds("goto", Dialect::Lua54), vec![GOTO_KW]);
        assert_eq!(kinds("goto", Dialect::LuaJit), vec![GOTO_KW]);
        assert_eq!(kinds("goto", Dialect::Lua51), vec![IDENT]);
        assert_eq!(kinds("goto", Dialect::Luau), vec![IDENT]);
    }

    #[test]
    fn keywords_and_idents() {
        assert_eq!(
            kinds("local function foo() end", Dialect::Lua51),
            vec![
                LOCAL_KW,
                WHITESPACE,
                FUNCTION_KW,
                WHITESPACE,
                IDENT,
                L_PAREN,
                R_PAREN,
                WHITESPACE,
                END_KW
            ]
        );
    }

    #[test]
    fn numbers() {
        assert_eq!(kinds("3", Dialect::Lua51), vec![NUMBER]);
        assert_eq!(kinds("3.1416e-2", Dialect::Lua51), vec![NUMBER]);
        assert_eq!(kinds("0xBEBADA", Dialect::Lua51), vec![NUMBER]);
        assert_eq!(kinds(".5", Dialect::Lua51), vec![NUMBER]);
        // hex float (5.2+/JIT lexes it; earlier dialects diagnose later)
        assert_eq!(kinds("0x1p4", Dialect::Lua52), vec![NUMBER]);
        // `1..2` is a concat of numbers, not a malformed float
        assert_eq!(kinds("1..2", Dialect::Lua51), vec![NUMBER, DOT_DOT, NUMBER]);
        // Luau: binary literal + digit separators
        assert_eq!(kinds("0b1010", Dialect::Luau), vec![NUMBER]);
        assert_eq!(kinds("1_000_000", Dialect::Luau), vec![NUMBER]);
        // ...which are two tokens outside Luau
        assert_eq!(kinds("1_000", Dialect::Lua54), vec![NUMBER, IDENT]);
        // LuaJIT suffixes
        assert_eq!(kinds("42ULL", Dialect::LuaJit), vec![NUMBER]);
        assert_eq!(kinds("12.5i", Dialect::LuaJit), vec![NUMBER]);
        assert_eq!(kinds("42LL", Dialect::Lua54), vec![NUMBER, IDENT]);
    }

    #[test]
    fn strings() {
        assert_eq!(kinds(r#""hello""#, Dialect::Lua51), vec![STRING]);
        assert_eq!(kinds(r#""a\"b""#, Dialect::Lua51), vec![STRING]);
        assert_eq!(kinds("[[multi\nline]]", Dialect::Lua51), vec![STRING]);
        assert_eq!(kinds("[==[ ]] ]==]", Dialect::Lua51), vec![STRING]);
        // \z skips the newline without terminating the string
        assert_eq!(kinds("\"a\\z\n  b\"", Dialect::Lua52), vec![STRING]);
        // unterminated short string: ERROR up to the newline, rest lexes on
        assert_eq!(
            kinds("x = 'oops\ny = 1", Dialect::Lua51),
            vec![
                IDENT, WHITESPACE, EQ, WHITESPACE, ERROR, WHITESPACE, IDENT, WHITESPACE, EQ,
                WHITESPACE, NUMBER
            ]
        );
    }

    #[test]
    fn comments() {
        assert_eq!(kinds("-- line", Dialect::Lua51), vec![COMMENT]);
        assert_eq!(
            kinds("--[[ block\n ]] x", Dialect::Lua51),
            vec![COMMENT, WHITESPACE, IDENT]
        );
        assert_eq!(kinds("--[=[ a ]] b ]=]", Dialect::Lua51), vec![COMMENT]);
        // `--[=` without the second bracket is a plain line comment
        assert_eq!(kinds("--[= not long", Dialect::Lua51), vec![COMMENT]);
    }

    #[test]
    fn operators_53() {
        assert_eq!(
            kinds("a // b & c | ~d << 2 >> 1", Dialect::Lua53),
            vec![
                IDENT,
                WHITESPACE,
                SLASH_SLASH,
                WHITESPACE,
                IDENT,
                WHITESPACE,
                AMP,
                WHITESPACE,
                IDENT,
                WHITESPACE,
                PIPE,
                WHITESPACE,
                TILDE,
                IDENT,
                WHITESPACE,
                LT_LT,
                WHITESPACE,
                NUMBER,
                WHITESPACE,
                GT_GT,
                WHITESPACE,
                NUMBER
            ]
        );
    }

    #[test]
    fn luau_syntax() {
        assert_eq!(
            kinds("x += 1", Dialect::Luau),
            vec![IDENT, WHITESPACE, PLUS_EQ, WHITESPACE, NUMBER]
        );
        assert_eq!(
            kinds("s ..= 'a'", Dialect::Luau),
            vec![IDENT, WHITESPACE, DOT_DOT_EQ, WHITESPACE, STRING]
        );
        assert_eq!(
            kinds("(x: number) -> string?", Dialect::Luau),
            vec![
                L_PAREN, IDENT, COLON, WHITESPACE, IDENT, R_PAREN, WHITESPACE, THIN_ARROW,
                WHITESPACE, IDENT, QUESTION
            ]
        );
        assert_eq!(
            kinds("`hi {name} and { {n=1} }`", Dialect::Luau),
            vec![INTERP_STRING]
        );
        // compound assignment does not exist outside Luau
        assert_eq!(
            kinds("x += 1", Dialect::Lua54),
            vec![IDENT, WHITESPACE, PLUS, EQ, WHITESPACE, NUMBER]
        );
        // backtick is an error byte outside Luau
        assert_eq!(kinds("`x`", Dialect::Lua54), vec![ERROR, IDENT, ERROR]);
    }

    #[test]
    fn labels() {
        assert_eq!(
            kinds("::top::", Dialect::Lua54),
            vec![COLON_COLON, IDENT, COLON_COLON]
        );
    }

    #[test]
    fn error_bytes_keep_char_boundaries() {
        // multi-byte char must be one ERROR token, not split bytes
        assert_eq!(kinds("£", Dialect::Lua51), vec![ERROR]);
    }
}
