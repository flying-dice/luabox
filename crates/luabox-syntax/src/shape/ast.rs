//! Typed AST layer over the lossless `.luab` green tree (SHAPES-V2.md).
//!
//! Thin wrappers around [`ShapeSyntaxNode`]: each accessor re-derives its value
//! from the tree on demand, so the AST stays a *view* — never a second source
//! of truth. Trivia (doc comments) is left in the tree for the formatter and
//! doc harvester to read directly.

use super::{ShapeSyntaxKind, ShapeSyntaxNode, ShapeSyntaxToken};
use ShapeSyntaxKind::{
    COLON, DOT, EXPORT_KW, FIELD, FN_TYPE, GENERIC_ARGS, GENERIC_PARAM, GENERIC_PARAMS, IDENT,
    INTERSECTION_TYPE, METHOD, OBJECT_TYPE, OPTIONAL_TYPE, PARAM, PARAM_LIST, PAREN_TYPE, QUESTION,
    SELF_KW, TYPE_DEF, TYPE_REF, UNION_TYPE,
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

// --- file & the single item form ----------------------------------------

ast_node!(ShapeFile, ShapeSyntaxKind::SHAPE_FILE);

impl ShapeFile {
    /// The type declarations in the file, in source order.
    pub fn items(&self) -> impl Iterator<Item = TypeDef> + '_ {
        child_nodes(&self.0)
    }
}

ast_node!(TypeDef, TYPE_DEF);

impl TypeDef {
    /// Whether the declaration carries the `export` modifier.
    pub fn is_export(&self) -> bool {
        first_token(&self.0, EXPORT_KW).is_some()
    }

    /// The declared name.
    pub fn name(&self) -> Option<String> {
        first_ident(&self.0)
    }

    /// The generic parameter list, if any.
    pub fn generic_params(&self) -> Option<GenericParams> {
        child_node(&self.0)
    }

    /// The right-hand type expression.
    pub fn ty(&self) -> Option<TypeRef> {
        child_node(&self.0)
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
}

ast_node!(GenericArgs, GENERIC_ARGS);

impl GenericArgs {
    /// The type arguments, in order.
    pub fn args(&self) -> impl Iterator<Item = TypeRef> + '_ {
        child_nodes(&self.0)
    }
}

// --- object types & members ---------------------------------------------

ast_node!(ObjectType, OBJECT_TYPE);

impl ObjectType {
    /// The members, in source order.
    pub fn members(&self) -> impl Iterator<Item = Member> + '_ {
        self.0.children().filter_map(Member::cast)
    }
}

/// One member of an object type: a data field or a method signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Member {
    /// `name?: type`
    Field(FieldMember),
    /// `name(self?, params...): ret?`
    Method(MethodMember),
}

impl AstNode for Member {
    fn cast(node: ShapeSyntaxNode) -> Option<Self> {
        match node.kind() {
            FIELD => Some(Self::Field(FieldMember(node))),
            METHOD => Some(Self::Method(MethodMember(node))),
            _ => None,
        }
    }

    fn syntax(&self) -> &ShapeSyntaxNode {
        match self {
            Self::Field(n) => n.syntax(),
            Self::Method(n) => n.syntax(),
        }
    }
}

ast_node!(FieldMember, FIELD);

impl FieldMember {
    /// The field name.
    pub fn name(&self) -> Option<String> {
        first_ident(&self.0)
    }

    /// Whether the field is declared optional (`name?: T`).
    pub fn optional(&self) -> bool {
        first_token(&self.0, QUESTION).is_some()
    }

    /// The field type.
    pub fn ty(&self) -> Option<TypeRef> {
        child_node(&self.0)
    }
}

ast_node!(MethodMember, METHOD);

impl MethodMember {
    /// The method name.
    pub fn name(&self) -> Option<String> {
        first_ident(&self.0)
    }

    /// The parameter list.
    pub fn params(&self) -> Option<ParamList> {
        child_node(&self.0)
    }

    /// Whether the first parameter is `self` (receiver-taking: implemented
    /// and called with `:`).
    pub fn has_self(&self) -> bool {
        self.params()
            .and_then(|p| p.params().next())
            .is_some_and(|p| p.is_self())
    }

    /// The declared return type, if any. A parenthesised comma list is a
    /// multi-return (see [`ParenType::inners`]).
    pub fn ret(&self) -> Option<TypeRef> {
        // The return type is the TypeRef that is a *direct* child of the
        // METHOD node (parameter types live inside the PARAM_LIST child).
        child_node(&self.0)
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

    /// Whether the parameter is declared optional (`name?: T`).
    pub fn optional(&self) -> bool {
        // The `?` on a parameter sits before the `:`, directly in the PARAM.
        self.0
            .children_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .take_while(|t| t.kind() != COLON)
            .any(|t| t.kind() == QUESTION)
    }

    /// The parameter type (`None` for `self`).
    pub fn ty(&self) -> Option<TypeRef> {
        child_node(&self.0)
    }
}

// --- type expressions -----------------------------------------------------

/// A type expression (SHAPES-V2.md grammar `type_expr`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeRef {
    /// A named reference, possibly qualified and generic:
    /// `number`, `geometry.Point`, `Pair<T>`.
    Named(NamedType),
    /// An object type: `{ x: number, area(self): number }`.
    Object(ObjectType),
    /// A nil-optional type: `T?`.
    Optional(OptionalType),
    /// A union type: `A | B`.
    Union(UnionType),
    /// An intersection type: `A & B`.
    Intersection(IntersectionType),
    /// A function type: `(a: A) => R`.
    Fn(FnType),
    /// A parenthesised type: `(T)`, or a multi-return list `(A, B)`.
    Paren(ParenType),
}

impl AstNode for TypeRef {
    fn cast(node: ShapeSyntaxNode) -> Option<Self> {
        match node.kind() {
            TYPE_REF => Some(Self::Named(NamedType(node))),
            OBJECT_TYPE => Some(Self::Object(ObjectType(node))),
            OPTIONAL_TYPE => Some(Self::Optional(OptionalType(node))),
            UNION_TYPE => Some(Self::Union(UnionType(node))),
            INTERSECTION_TYPE => Some(Self::Intersection(IntersectionType(node))),
            FN_TYPE => Some(Self::Fn(FnType(node))),
            PAREN_TYPE => Some(Self::Paren(ParenType(node))),
            _ => None,
        }
    }

    fn syntax(&self) -> &ShapeSyntaxNode {
        match self {
            Self::Named(n) => n.syntax(),
            Self::Object(n) => n.syntax(),
            Self::Optional(n) => n.syntax(),
            Self::Union(n) => n.syntax(),
            Self::Intersection(n) => n.syntax(),
            Self::Fn(n) => n.syntax(),
            Self::Paren(n) => n.syntax(),
        }
    }
}

ast_node!(NamedType, TYPE_REF);

impl NamedType {
    /// The full (possibly dotted) reference path: `Point`,
    /// `love.graphics.Canvas`. Generic-argument idents live in the
    /// [`GenericArgs`] child node and are not included.
    pub fn path(&self) -> String {
        self.0
            .children_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .filter(|t| matches!(t.kind(), IDENT | DOT))
            .map(|t| t.text().to_string())
            .collect()
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

ast_node!(IntersectionType, INTERSECTION_TYPE);

impl IntersectionType {
    /// The intersection members, in order.
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

    /// The return type (right of `=>`).
    pub fn ret(&self) -> Option<TypeRef> {
        child_node(&self.0)
    }
}

ast_node!(ParenType, PAREN_TYPE);

impl ParenType {
    /// The parenthesised inner types. One element for a plain group; several
    /// for a multi-return list (legal only in return position).
    pub fn inners(&self) -> impl Iterator<Item = TypeRef> + '_ {
        child_nodes(&self.0)
    }

    /// The single inner type, when this is a plain group.
    pub fn inner(&self) -> Option<TypeRef> {
        child_node(&self.0)
    }
}
