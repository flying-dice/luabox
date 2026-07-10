//! Token/node vocabulary for the `.luab` shape grammar — deliberately its own
//! kind space, disjoint from [`crate::lua::SyntaxKind`] (SHAPES.md §9).

syntax_kinds! {
    /// Every token and node kind in the `.luab` grammar (SHAPES.md §3).
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
        /// One `T: Bound + Bound2` (or bare `T`) inside [`Self::GENERIC_PARAMS`].
        GENERIC_PARAM,
        GENERIC_ARGS,
        /// A named/generic type reference: `IDENT generic_args?`
        /// (`number`, `Vec<T>`, `HashMap<K, V>`).
        TYPE_REF,
        /// A nil-union postfix type: `<inner> "?"`.
        OPTIONAL_TYPE,
        /// A union type: `<inner> ("|" <inner>)+`.
        UNION_TYPE,
        /// A function type: `"fn" "(" params? ")" ("->" ret)?`.
        FN_TYPE,
        /// A parenthesised type: `"(" type ")"`.
        PAREN_TYPE,
        /// A return clause: `"->" type ("," type)*` (multi-return).
        RET_TYPE,
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

/// The rowan language tag for `.luab` shape files.
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

/// A parsed `.luab` syntax node in the [`ShapeLanguage`] tree.
pub type ShapeSyntaxNode = rowan::SyntaxNode<ShapeLanguage>;
/// A leaf token in the [`ShapeLanguage`] tree.
pub type ShapeSyntaxToken = rowan::SyntaxToken<ShapeLanguage>;
/// A node-or-token element in the [`ShapeLanguage`] tree.
pub type ShapeSyntaxElement = rowan::SyntaxElement<ShapeLanguage>;
