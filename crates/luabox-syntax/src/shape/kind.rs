//! Token/node vocabulary for the `.lb` shape grammar — deliberately its own
//! kind space, disjoint from [`crate::lua::SyntaxKind`] (SHAPES.md §9).

syntax_kinds! {
    /// Every token and node kind in the `.lb` grammar (SHAPES.md §3).
    ///
    /// The numbering is not stable across versions — never persist it.
    ShapeSyntaxKind {
        // === Trivia ===
        WHITESPACE,
        /// `//` line comment or `/* ... */` block comment (nesting allowed).
        COMMENT,
        /// `///` doc comment — trivia in the tree, but harvested for hover
        /// and `luabox doc` (SHAPES.md §2).
        DOC_COMMENT,

        // === Names ===
        IDENT,

        // === Keywords ===
        STRUCT_KW, TRAIT_KW, IMPL_KW, FOR_KW, FN_KW, SELF_KW, TYPE_KW, USE_KW,

        // === Symbols ===
        L_BRACE, R_BRACE, L_PAREN, R_PAREN, L_ANGLE, R_ANGLE,
        COLON, SEMICOLON, COMMA, QUESTION, PIPE, PLUS, EQ,
        /// `->` return-type arrow.
        ARROW,
        DOT,
        /// `..` open-shape marker.
        DOT_DOT,

        /// Byte(s) no rule matched, or an unterminated block-comment tail.
        ERROR,

        // === Nodes (parser output; grows as the grammar lands) ===
        SHAPE_FILE,
        STRUCT_DEF,
        TRAIT_DEF,
        IMPL_DEF,
        TYPE_ALIAS,
        USE_DECL,
        FIELD,
        /// The `..` open marker inside a struct body.
        OPEN_MARKER,
        SUPERTRAITS,
        TRAIT_FN,
        PARAM_LIST,
        PARAM,
        GENERIC_PARAMS,
        GENERIC_ARGS,
        TYPE_REF,
        ERROR_NODE,
    }
}

impl ShapeSyntaxKind {
    /// Trivia is preserved in the tree but skipped by the parser proper.
    /// Doc comments are trivia structurally; doc harvesting reads them off
    /// the tokens preceding an item.
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            ShapeSyntaxKind::WHITESPACE | ShapeSyntaxKind::COMMENT | ShapeSyntaxKind::DOC_COMMENT
        )
    }
}

/// The rowan language tag for `.lb` shape files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ShapeLanguage {}

impl rowan::Language for ShapeLanguage {
    type Kind = ShapeSyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> ShapeSyntaxKind {
        ShapeSyntaxKind::from_u16(raw.0).expect("invalid ShapeSyntaxKind raw value")
    }

    fn kind_to_raw(kind: ShapeSyntaxKind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind as u16)
    }
}
