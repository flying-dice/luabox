//! The token/node vocabulary shared by the Lua lexer, parser, and rowan trees.

syntax_kinds! {
    /// Every token and node kind in the unified all-dialect Lua grammar.
    ///
    /// Token kinds are produced by the lexer; node kinds by the parser.
    /// The numbering is not stable across versions — never persist it.
    SyntaxKind {
        // === Trivia ===
        WHITESPACE,
        /// `--` line comment or `--[[ ... ]]` long comment (any bracket level).
        COMMENT,
        /// A UTF-8 byte-order mark at byte 0. Reference Lua skips it from 5.2
        /// on (`skipBOM`), as does LuaJIT; 5.1 rejects the file. Lexed as
        /// trivia in every dialect so the tree stays lossless — the parser
        /// turns it back into an error where the dialect demands one.
        BOM,
        /// A `#`-led first line (`#!/usr/bin/env lua`) at byte 0, up to but
        /// not including its newline — exactly how a `--` line comment is
        /// tokenized. Reference Lua has skipped it since 5.0 (`skipcomment`),
        /// in every version, so it is trivia in every dialect.
        SHEBANG,

        // === Literals & names ===
        IDENT,
        /// All numeric forms across dialects: decimal, hex, hex float
        /// (5.2+/JIT), `LL`/`ULL`/`i` suffixes (LuaJIT).
        NUMBER,
        /// `'...'`, `"..."`, or `[[...]]` long string (any bracket level).
        STRING,

        // === Keywords (all dialects) ===
        AND_KW, BREAK_KW, DO_KW, ELSE_KW, ELSEIF_KW, END_KW, FALSE_KW, FOR_KW,
        FUNCTION_KW, IF_KW, IN_KW, LOCAL_KW, NIL_KW, NOT_KW, OR_KW, REPEAT_KW,
        RETURN_KW, THEN_KW, TRUE_KW, UNTIL_KW, WHILE_KW,
        /// Keyword only where `Dialect::has_goto`; identifier in 5.1.
        GOTO_KW,

        // === Symbols ===
        PLUS, MINUS, STAR, SLASH, PERCENT, CARET, HASH,
        AMP, TILDE, PIPE, LT_LT, GT_GT, SLASH_SLASH,
        EQ, EQ_EQ, TILDE_EQ, LT_EQ, GT_EQ, LT, GT,
        L_PAREN, R_PAREN, L_BRACE, R_BRACE, L_BRACKET, R_BRACKET,
        SEMICOLON, COLON, COLON_COLON, COMMA, DOT, DOT_DOT, DOT_DOT_DOT,

        /// Byte(s) no rule matched, or an unterminated short-string tail.
        /// Lossless trees keep these; diagnostics point at them.
        ERROR,

        // === Nodes (parser output; grows as the grammar lands) ===
        SOURCE_FILE,
        /// Tokens the parser could not fit into the grammar; recovery keeps
        /// them here so the tree stays lossless.
        ERROR_NODE,
        BLOCK,

        // --- Statements ---
        /// `local a <const>, b = 1, 2` (attribs are 5.4; union grammar).
        LOCAL_STMT,
        /// One declared name with its optional attribute: `a <const>`.
        LOCAL_NAME,
        /// `<const>` / `<close>` (5.4 attribs; validated per dialect later).
        NAME_ATTRIB,
        /// `a, b.c[k] = 1, 2` — two `EXPR_LIST` children around `=`.
        ASSIGN_STMT,
        /// An expression in statement position; only calls are legal, but
        /// recovery wraps any expression here (with a `ParseError`).
        CALL_STMT,
        DO_STMT,
        WHILE_STMT,
        REPEAT_STMT,
        IF_STMT,
        ELSEIF_CLAUSE,
        ELSE_CLAUSE,
        /// `for i = a, b [, c] do ... end`
        NUMERIC_FOR_STMT,
        /// `for a, b in exprs do ... end`
        GENERIC_FOR_STMT,
        /// `function a.b.c:d(...) ... end`
        FUNCTION_DECL_STMT,
        /// The dotted/method path of a `FUNCTION_DECL_STMT`.
        FUNCTION_NAME,
        LOCAL_FUNCTION_STMT,
        RETURN_STMT,
        BREAK_STMT,
        /// `goto label` (5.2+/LuaJIT; in 5.1 `goto` lexes as IDENT).
        GOTO_STMT,
        /// `::label::`
        LABEL_STMT,

        // --- Expressions ---
        NAME_EXPR,
        /// `nil`, `true`, `false`, NUMBER, STRING.
        LITERAL_EXPR,
        /// `...`
        VARARG_EXPR,
        PAREN_EXPR,
        /// Unary: `not x`, `#x`, `-x`, `~x`.
        PREFIX_EXPR,
        BIN_EXPR,
        /// `function (...) ... end`
        FUNCTION_EXPR,
        /// Table constructor `{ ... }`.
        TABLE_EXPR,
        /// `f(args)`, `f "s"`, `f {t}`.
        CALL_EXPR,
        /// `o:m(args)`.
        METHOD_CALL_EXPR,
        /// `t[k]`
        INDEX_EXPR,
        /// `t.name`
        FIELD_EXPR,

        // --- Support ---
        PARAM_LIST,
        /// One parameter: a name or `...`.
        PARAM,
        /// Call arguments: `( EXPR_LIST )`, a STRING, or a `TABLE_EXPR`.
        ARG_LIST,
        EXPR_LIST,
        /// `[k] = v`
        TABLE_KEY_FIELD,
        /// `name = v`
        TABLE_NAME_FIELD,
        /// Positional `v`.
        TABLE_ITEM_FIELD,
    }
}

impl SyntaxKind {
    /// Trivia is preserved in the tree but skipped by the parser proper.
    ///
    /// The file prefix reference Lua skips before the first token — a UTF-8
    /// BOM and a `#!` line — is trivia to the *parser*, but unlike whitespace
    /// it is content the formatter must reproduce byte-exact; see
    /// [`SyntaxKind::is_file_prefix`].
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            SyntaxKind::WHITESPACE | SyntaxKind::COMMENT | SyntaxKind::BOM | SyntaxKind::SHEBANG
        )
    }

    /// The trivia that may only appear at the very start of a file: a UTF-8
    /// byte-order mark and a `#!` line. It carries no layout freedom — the
    /// formatter reproduces it verbatim rather than regenerating it.
    pub fn is_file_prefix(self) -> bool {
        matches!(self, SyntaxKind::BOM | SyntaxKind::SHEBANG)
    }
}

/// The rowan language tag for all Lua dialects (one tree type; dialect is a
/// parse-time parameter, not a tree type).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LuaLanguage {}

impl rowan::Language for LuaLanguage {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> SyntaxKind {
        #[expect(
            clippy::expect_used,
            reason = "rowan only ever passes back raw values produced by kind_to_raw (kind as u16), so from_u16 always round-trips"
        )]
        SyntaxKind::from_u16(raw.0).expect("invalid SyntaxKind raw value")
    }

    fn kind_to_raw(kind: SyntaxKind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rowan::Language;

    #[test]
    fn raw_kinds_round_trip_through_rowan() {
        // The whole tree relies on this: every kind rowan stores must come
        // back as the same kind.
        let mut raw = 0u16;
        while let Some(kind) = SyntaxKind::from_u16(raw) {
            assert_eq!(
                LuaLanguage::kind_from_raw(LuaLanguage::kind_to_raw(kind)),
                kind
            );
            assert_eq!(kind as u16, raw);
            raw += 1;
        }
        // The scan must have covered the whole enum, not stopped at zero.
        assert!(raw > 50, "only {raw} kinds round-tripped");
    }

    #[test]
    fn out_of_range_raw_values_are_rejected() {
        // `from_u16` is the guard behind `kind_from_raw`'s expect: values
        // outside the enum must be `None`, never a transmuted kind.
        assert_eq!(SyntaxKind::from_u16(u16::MAX), None);
        assert_eq!(
            SyntaxKind::from_u16(SyntaxKind::TABLE_ITEM_FIELD as u16 + 1),
            None
        );
    }

    #[test]
    fn whitespace_comments_and_the_file_prefix_are_trivia() {
        for kind in [
            SyntaxKind::WHITESPACE,
            SyntaxKind::COMMENT,
            SyntaxKind::BOM,
            SyntaxKind::SHEBANG,
        ] {
            assert!(kind.is_trivia(), "{kind:?}");
        }
        for kind in [
            SyntaxKind::IDENT,
            SyntaxKind::ERROR,
            SyntaxKind::SEMICOLON,
            SyntaxKind::SOURCE_FILE,
        ] {
            assert!(!kind.is_trivia(), "{kind:?}");
        }
    }

    #[test]
    fn only_the_bom_and_shebang_are_file_prefix_trivia() {
        assert!(SyntaxKind::BOM.is_file_prefix());
        assert!(SyntaxKind::SHEBANG.is_file_prefix());
        for kind in [
            SyntaxKind::WHITESPACE,
            SyntaxKind::COMMENT,
            SyntaxKind::IDENT,
            SyntaxKind::SOURCE_FILE,
        ] {
            assert!(!kind.is_file_prefix(), "{kind:?}");
        }
    }
}
