//! Token/node vocabulary for the `.luab` shape grammar — deliberately its own
//! kind space, disjoint from [`crate::lua::SyntaxKind`] (SHAPES-V2.md).

syntax_kinds! {
    /// Every token and node kind in the `.luab` grammar (SHAPES-V2.md).
    ///
    /// The numbering is not stable across versions — never persist it.
    ShapeSyntaxKind {
        // === Trivia ===
        WHITESPACE,
        /// `//` line comment or `/* ... */` block comment (nesting allowed).
        COMMENT,
        /// `///` doc comment — trivia in the tree, but harvested for hover
        /// and `luabox doc`.
        DOC_COMMENT,

        // === Names ===
        IDENT,

        // === Keywords ===
        TYPE_KW, EXPORT_KW, SELF_KW,

        // === Symbols ===
        L_BRACE, R_BRACE, L_PAREN, R_PAREN, L_ANGLE, R_ANGLE,
        COLON, COMMA, QUESTION, PIPE, AMP, EQ, DOT,
        /// `=>` function-type arrow.
        FAT_ARROW,

        /// Byte(s) no rule matched, or an unterminated block-comment tail.
        ERROR,

        // === Nodes ===
        SHAPE_FILE,
        /// `export? type Name<T, ...> = <type-expr>` — the only item form.
        TYPE_DEF,
        GENERIC_PARAMS,
        /// One bare `T` inside [`Self::GENERIC_PARAMS`] (no bounds in v2 —
        /// generics are checked structurally at instantiation).
        GENERIC_PARAM,
        GENERIC_ARGS,
        /// A (possibly dotted) named type reference with optional args:
        /// `number`, `geometry.Point`, `Pair<T>`, `love.graphics.Canvas`.
        TYPE_REF,
        /// An object type: `{ member, ... }`.
        OBJECT_TYPE,
        /// A data member inside an object type: `name?: type`.
        FIELD,
        /// A method member inside an object type:
        /// `name(self?, params...) (":" ret)?`. A `self` first parameter
        /// marks the member as receiver-taking (`:` definition side).
        METHOD,
        PARAM_LIST,
        PARAM,
        /// A function type: `"(" params? ")" "=>" ret`.
        FN_TYPE,
        /// A nil-union postfix type: `<inner> "?"`.
        OPTIONAL_TYPE,
        /// A union type: `<inner> ("|" <inner>)+`.
        UNION_TYPE,
        /// An intersection type: `<inner> ("&" <inner>)+`.
        INTERSECTION_TYPE,
        /// A parenthesised type: `"(" type ")"`. With commas it is a
        /// multi-return list, legal only in return position: `(a, b)`.
        PAREN_TYPE,
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
