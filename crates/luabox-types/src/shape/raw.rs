//! The owned, thread-safe mirror of the `.lb` typed AST.
//!
//! [`crate::shape::ShapeStore`] caches parsed shape modules across rayon
//! workers, and rowan syntax nodes are not `Send` — so the AST is converted
//! into this plain-data model immediately after parsing. Ranges are byte
//! offsets into the `.lb` source, kept for diagnostics.

use std::ops::Range;
use std::path::PathBuf;

use luabox_syntax::shape::{self, ast};

/// A `.lb` type expression, owned (SHAPES.md §3 `type`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RawTy {
    /// `Name` or `Name<Args>`; also every primitive keyword.
    Named {
        name: String,
        args: Vec<RawTy>,
        range: Range<usize>,
    },
    /// `T?`.
    Optional(Box<RawTy>),
    /// `A | B`.
    Union(Vec<RawTy>),
    /// `fn(a: A) -> R, S`.
    Fn {
        params: Vec<(String, RawTy)>,
        returns: Vec<RawTy>,
    },
    /// Unparseable — lowers to `unknown` (the parse error is reported when
    /// the `.lb` file itself is checked).
    Error,
}

/// One generic parameter with its bounds: `T: Shape + Drawable`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawGeneric {
    pub name: String,
    pub bounds: Vec<String>,
}

/// One struct field.
#[derive(Debug, Clone)]
pub(crate) struct RawField {
    pub name: String,
    pub ty: RawTy,
}

/// A `struct` declaration.
#[derive(Debug, Clone)]
pub(crate) struct RawStruct {
    pub name: String,
    pub generics: Vec<RawGeneric>,
    pub fields: Vec<RawField>,
    /// `..` open marker present — the shape is *not* sealed.
    pub open: bool,
    pub range: Range<usize>,
}

/// One trait function signature.
#[derive(Debug, Clone)]
pub(crate) struct RawTraitFn {
    pub name: String,
    pub has_self: bool,
    /// Non-`self` parameters, in order.
    pub params: Vec<(String, RawTy)>,
    pub returns: Vec<RawTy>,
    pub range: Range<usize>,
}

/// A `trait` declaration.
#[derive(Debug, Clone)]
pub(crate) struct RawTrait {
    pub name: String,
    pub supertraits: Vec<String>,
    pub fns: Vec<RawTraitFn>,
    pub range: Range<usize>,
}

/// A `type X = ...;` alias.
#[derive(Debug, Clone)]
pub(crate) struct RawAlias {
    pub name: String,
    pub generics: Vec<RawGeneric>,
    pub ty: Option<RawTy>,
}

/// An `impl Trait for Struct;` conformance assertion.
#[derive(Debug, Clone)]
pub(crate) struct RawImpl {
    pub trait_name: String,
    pub struct_name: String,
}

/// A `use path;` import.
#[derive(Debug, Clone)]
pub(crate) struct RawUse {
    pub path: String,
    pub range: Range<usize>,
}

/// One parse diagnostic, owned.
#[derive(Debug, Clone)]
pub(crate) struct RawError {
    /// `Some("LB2010")` for body rejection; `None` for plain syntax errors.
    pub code: Option<&'static str>,
    pub message: String,
    pub range: Range<usize>,
}

/// One fully parsed `.lb` shape module.
#[derive(Debug)]
pub(crate) struct RawModule {
    /// Diagnostic file name (project-relative, forward slashes).
    pub file: String,
    /// The directory the module lives in (sibling tier for its own `use`s).
    pub dir: PathBuf,
    pub uses: Vec<RawUse>,
    pub structs: Vec<RawStruct>,
    pub traits: Vec<RawTrait>,
    pub aliases: Vec<RawAlias>,
    pub impls: Vec<RawImpl>,
    pub errors: Vec<RawError>,
}

fn node_range(node: &shape::ShapeSyntaxNode) -> Range<usize> {
    let r = node.text_range();
    usize::from(r.start())..usize::from(r.end())
}

/// Parse `.lb` source into the owned module model.
pub(crate) fn parse_module(source: &str, file: String, dir: PathBuf) -> RawModule {
    let parse = shape::parse(source);
    let mut module = RawModule {
        file,
        dir,
        uses: Vec::new(),
        structs: Vec::new(),
        traits: Vec::new(),
        aliases: Vec::new(),
        impls: Vec::new(),
        errors: parse
            .errors()
            .iter()
            .map(|e| RawError {
                code: e.code,
                message: e.message.clone(),
                range: usize::from(e.range.start())..usize::from(e.range.end()),
            })
            .collect(),
    };
    let Some(root) = ast::AstNode::cast(parse.syntax()) else {
        return module;
    };
    let root: ast::ShapeFile = root;
    for item in root.items() {
        match item {
            ast::Item::Struct(s) => module.structs.push(convert_struct(&s)),
            ast::Item::Trait(t) => module.traits.push(convert_trait(&t)),
            ast::Item::Impl(i) => {
                if let (Some(trait_name), Some(struct_name)) = (i.trait_name(), i.struct_name()) {
                    module.impls.push(RawImpl {
                        trait_name,
                        struct_name,
                    });
                }
            }
            ast::Item::Alias(a) => {
                if let Some(name) = a.name() {
                    module.aliases.push(RawAlias {
                        name,
                        generics: convert_generics(a.generic_params()),
                        ty: a.ty().as_ref().map(convert_ty),
                    });
                }
            }
            ast::Item::Use(u) => {
                let path = u.path();
                if !path.is_empty() {
                    module.uses.push(RawUse {
                        range: node_range(ast::AstNode::syntax(&u)),
                        path,
                    });
                }
            }
        }
    }
    module
}

fn convert_generics(params: Option<ast::GenericParams>) -> Vec<RawGeneric> {
    params
        .map(|list| {
            list.params()
                .filter_map(|p| {
                    p.name().map(|name| RawGeneric {
                        name,
                        bounds: p.bounds(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn convert_struct(def: &ast::StructDef) -> RawStruct {
    RawStruct {
        name: def.name().unwrap_or_default(),
        generics: convert_generics(def.generic_params()),
        fields: def
            .fields()
            .filter_map(|f| {
                let name = f.name()?;
                let ty = f.ty().as_ref().map_or(RawTy::Error, convert_ty);
                Some(RawField { name, ty })
            })
            .collect(),
        open: def.is_open(),
        range: node_range(ast::AstNode::syntax(def)),
    }
}

fn convert_trait(def: &ast::TraitDef) -> RawTrait {
    RawTrait {
        name: def.name().unwrap_or_default(),
        supertraits: def.supertraits(),
        fns: def
            .fns()
            .filter_map(|f| {
                let name = f.name()?;
                let (has_self, params) = convert_params(f.params());
                Some(RawTraitFn {
                    name,
                    has_self,
                    params,
                    returns: f.returns().iter().map(convert_ty).collect(),
                    range: node_range(ast::AstNode::syntax(&f)),
                })
            })
            .collect(),
        range: node_range(ast::AstNode::syntax(def)),
    }
}

/// Convert a parameter list: whether the first parameter is `self`, plus the
/// remaining named parameters.
fn convert_params(list: Option<ast::ParamList>) -> (bool, Vec<(String, RawTy)>) {
    let mut has_self = false;
    let mut params = Vec::new();
    if let Some(list) = list {
        for param in list.params() {
            if param.is_self() {
                has_self = true;
            } else if let Some(name) = param.name() {
                let ty = param.ty().as_ref().map_or(RawTy::Error, convert_ty);
                params.push((name, ty));
            }
        }
    }
    (has_self, params)
}

fn convert_ty(ty: &ast::TypeRef) -> RawTy {
    match ty {
        ast::TypeRef::Named(named) => RawTy::Named {
            name: named.name().unwrap_or_default(),
            args: named
                .args()
                .map(|a| a.args().map(|t| convert_ty(&t)).collect())
                .unwrap_or_default(),
            range: node_range(ast::AstNode::syntax(ty)),
        },
        ast::TypeRef::Optional(opt) => RawTy::Optional(Box::new(
            opt.inner().as_ref().map_or(RawTy::Error, convert_ty),
        )),
        ast::TypeRef::Union(union) => {
            RawTy::Union(union.members().map(|m| convert_ty(&m)).collect())
        }
        ast::TypeRef::Fn(func) => {
            let (_, params) = convert_params(func.params());
            RawTy::Fn {
                params,
                returns: func.returns().iter().map(convert_ty).collect(),
            }
        }
        ast::TypeRef::Paren(paren) => paren.inner().as_ref().map_or(RawTy::Error, convert_ty),
    }
}

/// Parse the raw generic-argument text of a `---@struct Name<...>` binding
/// tag with the real shape type grammar, via a synthetic alias declaration.
/// Returns `None` when the text does not parse cleanly.
pub(crate) fn parse_type_args(args: &str) -> Option<Vec<RawTy>> {
    let synthetic = format!("type __A = __B<{args}>;");
    let parse = shape::parse(&synthetic);
    if !parse.errors().is_empty() {
        return None;
    }
    let root: ast::ShapeFile = ast::AstNode::cast(parse.syntax())?;
    for item in root.items() {
        if let ast::Item::Alias(alias) = item
            && let Some(ast::TypeRef::Named(named)) = alias.ty()
        {
            return Some(
                named
                    .args()
                    .map(|a| a.args().map(|t| convert_ty(&t)).collect())
                    .unwrap_or_default(),
            );
        }
    }
    None
}
