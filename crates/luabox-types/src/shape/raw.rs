//! The owned, thread-safe mirror of the `.luab` typed AST.
//!
//! [`crate::shape::ShapeStore`] caches parsed shape modules across rayon
//! workers, and rowan syntax nodes are not `Send` — so the AST is converted
//! into this plain-data model immediately after parsing. Ranges are byte
//! offsets into the `.luab` source, kept for diagnostics.

use std::ops::Range;

use luabox_syntax::shape::{self, ast};

/// A `.luab` type expression, owned (SHAPES-V2.md `type_expr`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RawTy {
    /// `Name`, `pkg.mod.Name`, or `Name<Args>`; also every primitive keyword.
    /// `name` is the reference exactly as written (dotted when qualified).
    Named {
        name: String,
        args: Vec<RawTy>,
        range: Range<usize>,
    },
    /// `{ fields..., methods... }`. A member-level `?` (`label?: string`)
    /// is folded into an [`RawTy::Optional`] field type — Lua nil semantics
    /// make "absent" and "nil" the same fact.
    Object {
        fields: Vec<RawField>,
        methods: Vec<RawMethod>,
        range: Range<usize>,
    },
    /// `T?`.
    Optional(Box<RawTy>),
    /// `A | B`.
    Union(Vec<RawTy>),
    /// `A & B`.
    Intersection(Vec<RawTy>),
    /// `(a: A) => R`.
    Fn {
        params: Vec<(String, RawTy)>,
        returns: Vec<RawTy>,
    },
    /// Unparseable — lowers to `unknown` (the parse error is reported when
    /// the `.luab` file itself is checked).
    Error,
}

/// One object field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawField {
    pub name: String,
    pub ty: RawTy,
}

/// One object method signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawMethod {
    pub name: String,
    pub has_self: bool,
    /// Non-`self` parameters, in order.
    pub params: Vec<(String, RawTy)>,
    /// Empty when the method returns nothing; a parenthesised comma list in
    /// return position is a multi-return.
    pub returns: Vec<RawTy>,
    pub range: Range<usize>,
}

/// An `export? type Name<T, ...> = <type-expr>` declaration — the only item
/// form in v2.
#[derive(Debug, Clone)]
pub(crate) struct RawTypeDef {
    pub name: String,
    /// `export` modifier present: part of the package's published surface.
    pub export: bool,
    /// Generic parameter names (v2 generics carry no bounds).
    pub generics: Vec<String>,
    pub ty: Option<RawTy>,
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

/// One fully parsed `.luab` shape module.
#[derive(Debug)]
pub(crate) struct RawModule {
    /// Diagnostic file name (project-relative, forward slashes).
    pub file: String,
    /// The module's dot-separated namespace, derived from its path under
    /// `[types] shape-paths` (`shapes/love/graphics.luab` → `love.graphics`).
    /// Set by the store at load time; empty until then.
    pub namespace: String,
    pub types: Vec<RawTypeDef>,
    pub errors: Vec<RawError>,
}

fn node_range(node: &shape::ShapeSyntaxNode) -> Range<usize> {
    let r = node.text_range();
    usize::from(r.start())..usize::from(r.end())
}

/// Parse `.luab` source into the owned module model. `namespace` is the
/// path-derived dotted module namespace.
pub(crate) fn parse_module(source: &str, file: String, namespace: String) -> RawModule {
    let parse = shape::parse(source);
    let mut module = RawModule {
        file,
        namespace,
        types: Vec::new(),
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
    let mut errors = Vec::new();
    for item in root.items() {
        let Some(name) = item.name() else { continue };
        module.types.push(RawTypeDef {
            name,
            export: item.is_export(),
            generics: item
                .generic_params()
                .map(|g| g.params().filter_map(|p| p.name()).collect())
                .unwrap_or_default(),
            ty: item.ty().as_ref().map(|t| convert_ty(t, &mut errors)),
            range: node_range(ast::AstNode::syntax(&item)),
        });
    }
    module.errors.append(&mut errors);
    module
}

/// Convert a parameter list: whether the first parameter is `self`, plus the
/// remaining named parameters. An optional parameter (`x?: T`) folds into an
/// [`RawTy::Optional`] parameter type.
fn convert_params(
    list: Option<ast::ParamList>,
    errors: &mut Vec<RawError>,
) -> (bool, Vec<(String, RawTy)>) {
    let mut has_self = false;
    let mut params = Vec::new();
    if let Some(list) = list {
        for param in list.params() {
            if param.is_self() {
                has_self = true;
            } else if let Some(name) = param.name() {
                let mut ty = param
                    .ty()
                    .as_ref()
                    .map_or(RawTy::Error, |t| convert_ty(t, errors));
                if param.optional() && !matches!(ty, RawTy::Optional(_)) {
                    ty = RawTy::Optional(Box::new(ty));
                }
                params.push((name, ty));
            }
        }
    }
    (has_self, params)
}

/// A return position: nothing, one type, or a parenthesised multi-return.
fn convert_returns(ret: Option<&ast::TypeRef>, errors: &mut Vec<RawError>) -> Vec<RawTy> {
    match ret {
        None => Vec::new(),
        Some(ast::TypeRef::Paren(p)) => {
            let inners: Vec<ast::TypeRef> = p.inners().collect();
            if inners.is_empty() {
                vec![RawTy::Error]
            } else {
                inners.iter().map(|t| convert_ty(t, errors)).collect()
            }
        }
        Some(other) => vec![convert_ty(other, errors)],
    }
}

fn convert_ty(ty: &ast::TypeRef, errors: &mut Vec<RawError>) -> RawTy {
    match ty {
        ast::TypeRef::Named(named) => RawTy::Named {
            name: named.path(),
            args: named
                .args()
                .map(|a| a.args().map(|t| convert_ty(&t, errors)).collect())
                .unwrap_or_default(),
            range: node_range(ast::AstNode::syntax(ty)),
        },
        ast::TypeRef::Object(obj) => {
            let mut fields = Vec::new();
            let mut methods = Vec::new();
            for member in obj.members() {
                match member {
                    ast::Member::Field(f) => {
                        let Some(name) = f.name() else { continue };
                        let mut fty = f
                            .ty()
                            .as_ref()
                            .map_or(RawTy::Error, |t| convert_ty(t, errors));
                        if f.optional() && !matches!(fty, RawTy::Optional(_)) {
                            fty = RawTy::Optional(Box::new(fty));
                        }
                        fields.push(RawField { name, ty: fty });
                    }
                    ast::Member::Method(m) => {
                        let Some(name) = m.name() else { continue };
                        let (has_self, params) = convert_params(m.params(), errors);
                        methods.push(RawMethod {
                            name,
                            has_self,
                            params,
                            returns: convert_returns(m.ret().as_ref(), errors),
                            range: node_range(ast::AstNode::syntax(&m)),
                        });
                    }
                }
            }
            RawTy::Object {
                fields,
                methods,
                range: node_range(ast::AstNode::syntax(ty)),
            }
        }
        ast::TypeRef::Optional(opt) => RawTy::Optional(Box::new(
            opt.inner()
                .as_ref()
                .map_or(RawTy::Error, |t| convert_ty(t, errors)),
        )),
        ast::TypeRef::Union(union) => {
            RawTy::Union(union.members().map(|m| convert_ty(&m, errors)).collect())
        }
        ast::TypeRef::Intersection(inter) => {
            RawTy::Intersection(inter.members().map(|m| convert_ty(&m, errors)).collect())
        }
        ast::TypeRef::Fn(func) => {
            let (_, params) = convert_params(func.params(), errors);
            RawTy::Fn {
                params,
                returns: convert_returns(func.ret().as_ref(), errors),
            }
        }
        ast::TypeRef::Paren(paren) => {
            let inners: Vec<ast::TypeRef> = paren.inners().collect();
            match inners.as_slice() {
                [] => RawTy::Error,
                [single] => convert_ty(single, errors),
                _ => {
                    errors.push(RawError {
                        code: Some("LB2007"),
                        message: "a multi-return list `(A, B)` is only legal in return position"
                            .to_string(),
                        range: node_range(ast::AstNode::syntax(ty)),
                    });
                    RawTy::Error
                }
            }
        }
    }
}
