//! Typed AST layer over the lossless `.lb` green tree (SHAPES.md §3).
//!
//! Thin wrappers around [`ShapeSyntaxNode`]: each accessor re-derives its value
//! from the tree on demand, so the AST stays a *view* — never a second source
//! of truth. Trivia (doc comments) is left in the tree for the formatter and
//! doc harvester to read directly.

use super::{ShapeSyntaxKind, ShapeSyntaxNode, ShapeSyntaxToken};
use ShapeSyntaxKind::{
    DOT, FIELD, FN_TYPE, FOR_KW, GENERIC_ARGS, GENERIC_PARAM, GENERIC_PARAMS, IDENT, IMPL_DEF,
    OPEN_MARKER, OPTIONAL_TYPE, PARAM, PARAM_LIST, PAREN_TYPE, RET_TYPE, SELF_KW, STRUCT_DEF,
    SUPERTRAITS, TRAIT_DEF, TRAIT_FN, TYPE_ALIAS, TYPE_REF, UNION_TYPE, USE_DECL,
};

/// A typed handle onto a syntax node of a specific kind.
pub trait AstNode: Sized {
    /// Wrap `node` if it is of the expected kind.
    fn cast(node: ShapeSyntaxNode) -> Option<Self>;
    /// The underlying syntax node.
    fn syntax(&self) -> &ShapeSyntaxNode;
}

// --- small helpers ------------------------------------------------------

fn child_node<T: AstNode>(node: &ShapeSyntaxNode) -> Option<T> {
    node.children().find_map(T::cast)
}

fn child_nodes<T: AstNode + 'static>(node: &ShapeSyntaxNode) -> impl Iterator<Item = T> + '_ {
    node.children().filter_map(T::cast)
}

fn first_token(node: &ShapeSyntaxNode, kind: ShapeSyntaxKind) -> Option<ShapeSyntaxToken> {
    node.children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .find(|t| t.kind() == kind)
}

fn first_ident(node: &ShapeSyntaxNode) -> Option<String> {
    first_token(node, IDENT).map(|t| t.text().to_string())
}

macro_rules! ast_node {
    ($name:ident, $kind:expr) => {
        #[doc = concat!("Typed view of a `", stringify!($name), "` node.")]
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $name(ShapeSyntaxNode);

        impl AstNode for $name {
            fn cast(node: ShapeSyntaxNode) -> Option<Self> {
                (node.kind() == $kind).then_some(Self(node))
            }
            fn syntax(&self) -> &ShapeSyntaxNode {
                &self.0
            }
        }
    };
}

// --- file & items -------------------------------------------------------

ast_node!(ShapeFile, ShapeSyntaxKind::SHAPE_FILE);

impl ShapeFile {
    /// The declarations in the file, in source order.
    pub fn items(&self) -> impl Iterator<Item = Item> + '_ {
        self.0.children().filter_map(Item::cast)
    }
}

/// Any top-level `.lb` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// A `struct` declaration.
    Struct(StructDef),
    /// A `trait` declaration.
    Trait(TraitDef),
    /// An `impl Trait for Struct;` conformance assertion.
    Impl(ImplDef),
    /// A `type X = ...;` alias.
    Alias(TypeAlias),
    /// A `use path;` import.
    Use(UseDecl),
}

impl AstNode for Item {
    fn cast(node: ShapeSyntaxNode) -> Option<Self> {
        match node.kind() {
            STRUCT_DEF => Some(Self::Struct(StructDef(node))),
            TRAIT_DEF => Some(Self::Trait(TraitDef(node))),
            IMPL_DEF => Some(Self::Impl(ImplDef(node))),
            TYPE_ALIAS => Some(Self::Alias(TypeAlias(node))),
            USE_DECL => Some(Self::Use(UseDecl(node))),
            _ => None,
        }
    }

    fn syntax(&self) -> &ShapeSyntaxNode {
        match self {
            Self::Struct(n) => n.syntax(),
            Self::Trait(n) => n.syntax(),
            Self::Impl(n) => n.syntax(),
            Self::Alias(n) => n.syntax(),
            Self::Use(n) => n.syntax(),
        }
    }
}

// --- struct -------------------------------------------------------------

ast_node!(StructDef, STRUCT_DEF);

impl StructDef {
    /// The struct name.
    pub fn name(&self) -> Option<String> {
        first_ident(&self.0)
    }

    /// The generic parameter list, if any.
    pub fn generic_params(&self) -> Option<GenericParams> {
        child_node(&self.0)
    }

    /// The declared fields, in order.
    pub fn fields(&self) -> impl Iterator<Item = Field> + '_ {
        child_nodes(&self.0)
    }

    /// Whether the struct carries the `..` open marker.
    pub fn is_open(&self) -> bool {
        self.0.children().any(|c| c.kind() == OPEN_MARKER)
    }
}

ast_node!(Field, FIELD);

impl Field {
    /// The field name.
    pub fn name(&self) -> Option<String> {
        first_ident(&self.0)
    }

    /// The field type.
    pub fn ty(&self) -> Option<TypeRef> {
        child_node(&self.0)
    }

    /// Whether the field type is nil-optional (`T?`).
    pub fn optional(&self) -> bool {
        matches!(self.ty(), Some(TypeRef::Optional(_)))
    }
}

// --- trait --------------------------------------------------------------

ast_node!(TraitDef, TRAIT_DEF);

impl TraitDef {
    /// The trait name.
    pub fn name(&self) -> Option<String> {
        first_ident(&self.0)
    }

    /// The generic parameter list, if any.
    pub fn generic_params(&self) -> Option<GenericParams> {
        child_node(&self.0)
    }

    /// The supertrait names from `: A + B`, in order.
    pub fn supertraits(&self) -> Vec<String> {
        self.0
            .children()
            .find(|c| c.kind() == SUPERTRAITS)
            .map(|s| {
                s.children_with_tokens()
                    .filter_map(rowan::NodeOrToken::into_token)
                    .filter(|t| t.kind() == IDENT)
                    .map(|t| t.text().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The trait function signatures, in order.
    pub fn fns(&self) -> impl Iterator<Item = TraitFn> + '_ {
        child_nodes(&self.0)
    }
}

ast_node!(TraitFn, TRAIT_FN);

impl TraitFn {
    /// The function name.
    pub fn name(&self) -> Option<String> {
        first_ident(&self.0)
    }

    /// The parameter list.
    pub fn params(&self) -> Option<ParamList> {
        child_node(&self.0)
    }

    /// Whether the first parameter is `self`.
    pub fn has_self(&self) -> bool {
        self.params()
            .and_then(|p| p.params().next())
            .is_some_and(|p| p.is_self())
    }

    /// The return types (possibly multiple; empty when the fn returns nothing).
    pub fn returns(&self) -> Vec<TypeRef> {
        self.0
            .children()
            .find(|c| c.kind() == RET_TYPE)
            .map(|r| r.children().filter_map(TypeRef::cast).collect())
            .unwrap_or_default()
    }
}

ast_node!(ParamList, PARAM_LIST);

impl ParamList {
    /// The parameters, in order.
    pub fn params(&self) -> impl Iterator<Item = Param> + '_ {
        child_nodes(&self.0)
    }
}

ast_node!(Param, PARAM);

impl Param {
    /// Whether this parameter is the `self` receiver.
    pub fn is_self(&self) -> bool {
        first_token(&self.0, SELF_KW).is_some()
    }

    /// The parameter name (`None` for `self`).
    pub fn name(&self) -> Option<String> {
        if self.is_self() {
            None
        } else {
            first_ident(&self.0)
        }
    }

    /// The parameter type (`None` for `self`).
    pub fn ty(&self) -> Option<TypeRef> {
        child_node(&self.0)
    }
}

// --- impl / alias / use -------------------------------------------------

ast_node!(ImplDef, IMPL_DEF);

impl ImplDef {
    /// The trait being asserted (the name before `for`).
    pub fn trait_name(&self) -> Option<String> {
        first_ident(&self.0)
    }

    /// The struct the trait is asserted for (the name after `for`).
    pub fn struct_name(&self) -> Option<String> {
        let mut seen_for = false;
        for t in self
            .0
            .children_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
        {
            if seen_for && t.kind() == IDENT {
                return Some(t.text().to_string());
            }
            if t.kind() == FOR_KW {
                seen_for = true;
            }
        }
        None
    }

    /// The generic parameter list on the trait reference, if any.
    pub fn generic_params(&self) -> Option<GenericParams> {
        child_node(&self.0)
    }
}

ast_node!(TypeAlias, TYPE_ALIAS);

impl TypeAlias {
    /// The alias name.
    pub fn name(&self) -> Option<String> {
        first_ident(&self.0)
    }

    /// The generic parameter list, if any.
    pub fn generic_params(&self) -> Option<GenericParams> {
        child_node(&self.0)
    }

    /// The aliased type (right of `=`).
    pub fn ty(&self) -> Option<TypeRef> {
        child_node(&self.0)
    }
}

ast_node!(UseDecl, USE_DECL);

impl UseDecl {
    /// The dotted import path (e.g. `pkg.geometry.core`).
    pub fn path(&self) -> String {
        self.0
            .children_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .filter(|t| matches!(t.kind(), IDENT | DOT))
            .map(|t| t.text().to_string())
            .collect()
    }
}

// --- generics -----------------------------------------------------------

ast_node!(GenericParams, GENERIC_PARAMS);

impl GenericParams {
    /// The declared parameters, in order.
    pub fn params(&self) -> impl Iterator<Item = GenericParam> + '_ {
        child_nodes(&self.0)
    }
}

ast_node!(GenericParam, GENERIC_PARAM);

impl GenericParam {
    /// The parameter name.
    pub fn name(&self) -> Option<String> {
        first_ident(&self.0)
    }

    /// The declared bounds from `: A + B` (empty if unbounded).
    pub fn bounds(&self) -> Vec<String> {
        // Every IDENT after the first is a bound.
        self.0
            .children_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .filter(|t| t.kind() == IDENT)
            .skip(1)
            .map(|t| t.text().to_string())
            .collect()
    }
}

ast_node!(GenericArgs, GENERIC_ARGS);

impl GenericArgs {
    /// The type arguments, in order.
    pub fn args(&self) -> impl Iterator<Item = TypeRef> + '_ {
        child_nodes(&self.0)
    }
}

// --- types --------------------------------------------------------------

/// A type expression (SHAPES.md §3 `type`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeRef {
    /// A named/generic type: `number`, `Vec<T>`, `HashMap<K, V>`.
    Named(NamedType),
    /// A nil-optional type: `T?`.
    Optional(OptionalType),
    /// A union type: `A | B`.
    Union(UnionType),
    /// A function type: `fn(a: A) -> R`.
    Fn(FnType),
    /// A parenthesised type: `(T)`.
    Paren(ParenType),
}

impl AstNode for TypeRef {
    fn cast(node: ShapeSyntaxNode) -> Option<Self> {
        match node.kind() {
            TYPE_REF => Some(Self::Named(NamedType(node))),
            OPTIONAL_TYPE => Some(Self::Optional(OptionalType(node))),
            UNION_TYPE => Some(Self::Union(UnionType(node))),
            FN_TYPE => Some(Self::Fn(FnType(node))),
            PAREN_TYPE => Some(Self::Paren(ParenType(node))),
            _ => None,
        }
    }

    fn syntax(&self) -> &ShapeSyntaxNode {
        match self {
            Self::Named(n) => n.syntax(),
            Self::Optional(n) => n.syntax(),
            Self::Union(n) => n.syntax(),
            Self::Fn(n) => n.syntax(),
            Self::Paren(n) => n.syntax(),
        }
    }
}

ast_node!(NamedType, TYPE_REF);

impl NamedType {
    /// The type constructor name.
    pub fn name(&self) -> Option<String> {
        first_ident(&self.0)
    }

    /// The generic arguments, if any.
    pub fn args(&self) -> Option<GenericArgs> {
        child_node(&self.0)
    }
}

ast_node!(OptionalType, OPTIONAL_TYPE);

impl OptionalType {
    /// The wrapped inner type.
    pub fn inner(&self) -> Option<TypeRef> {
        child_node(&self.0)
    }
}

ast_node!(UnionType, UNION_TYPE);

impl UnionType {
    /// The union members, in order.
    pub fn members(&self) -> impl Iterator<Item = TypeRef> + '_ {
        child_nodes(&self.0)
    }
}

ast_node!(FnType, FN_TYPE);

impl FnType {
    /// The parameter list.
    pub fn params(&self) -> Option<ParamList> {
        child_node(&self.0)
    }

    /// The return types (empty when none).
    pub fn returns(&self) -> Vec<TypeRef> {
        self.0
            .children()
            .find(|c| c.kind() == RET_TYPE)
            .map(|r| r.children().filter_map(TypeRef::cast).collect())
            .unwrap_or_default()
    }
}

ast_node!(ParenType, PAREN_TYPE);

impl ParenType {
    /// The parenthesised inner type.
    pub fn inner(&self) -> Option<TypeRef> {
        child_node(&self.0)
    }
}
