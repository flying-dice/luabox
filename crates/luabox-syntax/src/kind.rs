//! The token/node vocabulary shared by the lexer, parser, and rowan trees.

/// Defines [`SyntaxKind`] plus a safe `u16` round-trip (no `unsafe`, no
/// hand-maintained match — the macro generates both directions).
macro_rules! syntax_kinds {
    ($($(#[$attr:meta])* $name:ident,)*) => {
        /// Every token and node kind in the unified all-dialect grammar.
        ///
        /// Token kinds are produced by the lexer; node kinds by the parser.
        /// The numbering is not stable across versions — never persist it.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(u16)]
        #[allow(non_camel_case_types)]
        pub enum SyntaxKind {
            $($(#[$attr])* $name,)*
        }

        impl SyntaxKind {
            fn from_u16(raw: u16) -> Option<Self> {
                $(if raw == Self::$name as u16 {
                    return Some(Self::$name);
                })*
                None
            }
        }
    };
}

syntax_kinds! {
    // === Trivia ===
    WHITESPACE,
    /// `--` line comment or `--[[ ... ]]` long comment (any bracket level).
    COMMENT,

    // === Literals & names ===
    IDENT,
    /// All numeric forms across dialects: decimal, hex, hex float (5.2+/JIT),
    /// binary (Luau), digit separators (Luau), LL/ULL/i suffixes (LuaJIT).
    NUMBER,
    /// `'...'`, `"..."`, or `[[...]]` long string (any bracket level).
    STRING,
    /// Luau backtick interpolated string, lexed as one token; the parser
    /// re-lexes the `{expr}` holes.
    INTERP_STRING,

    // === Keywords (all dialects) ===
    AND_KW, BREAK_KW, DO_KW, ELSE_KW, ELSEIF_KW, END_KW, FALSE_KW, FOR_KW,
    FUNCTION_KW, IF_KW, IN_KW, LOCAL_KW, NIL_KW, NOT_KW, OR_KW, REPEAT_KW,
    RETURN_KW, THEN_KW, TRUE_KW, UNTIL_KW, WHILE_KW,
    /// Keyword only where `Dialect::has_goto`; identifier elsewhere.
    GOTO_KW,

    // === Symbols ===
    PLUS, MINUS, STAR, SLASH, PERCENT, CARET, HASH,
    AMP, TILDE, PIPE, LT_LT, GT_GT, SLASH_SLASH,
    EQ, EQ_EQ, TILDE_EQ, LT_EQ, GT_EQ, LT, GT,
    L_PAREN, R_PAREN, L_BRACE, R_BRACE, L_BRACKET, R_BRACKET,
    SEMICOLON, COLON, COLON_COLON, COMMA, DOT, DOT_DOT, DOT_DOT_DOT,

    // === Luau-only symbols ===
    PLUS_EQ, MINUS_EQ, STAR_EQ, SLASH_EQ, SLASH_SLASH_EQ, PERCENT_EQ,
    CARET_EQ, DOT_DOT_EQ, THIN_ARROW, QUESTION,

    /// Byte(s) no rule matched, or an unterminated string/comment tail.
    /// Lossless trees keep these; diagnostics point at them.
    ERROR,

    // === Nodes (parser output; grows as the grammar lands) ===
    SOURCE_FILE,
    ERROR_NODE,
}

impl SyntaxKind {
    /// Trivia is preserved in the tree but skipped by the parser proper.
    pub fn is_trivia(self) -> bool {
        matches!(self, SyntaxKind::WHITESPACE | SyntaxKind::COMMENT)
    }
}

/// The rowan language tag for all Lua dialects (one tree type; dialect is a
/// parse-time parameter, not a tree type).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LuaLanguage {}

impl rowan::Language for LuaLanguage {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> SyntaxKind {
        SyntaxKind::from_u16(raw.0).expect("invalid SyntaxKind raw value")
    }

    fn kind_to_raw(kind: SyntaxKind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind as u16)
    }
}
