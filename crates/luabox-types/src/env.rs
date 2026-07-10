//! The per-file type environment: every declaration the annotations make.
//!
//! Built in two passes over [`luacats::harvest`] output: first collect the
//! *names* of classes/aliases/enums (so forward references resolve), then
//! lower every annotation body against them. Cross-file `require`
//! resolution is P1 — the environment is strictly per file for now.

use std::collections::{BTreeMap, HashMap, HashSet};

use luabox_syntax::lua::ast::{AstNode, Expr, LocalStmt, Stmt};
use luabox_syntax::lua::{self, SyntaxKind, SyntaxNode};
use luabox_syntax::luacats::{self, FieldKey, ParamTag, ReturnTag, Tag, TypeExprKind};

use crate::lower::{Declared, Lowerer};
use crate::ty::{FieldTy, FunctionTy, ParamTy, TableTy, Ty};

/// A statement's byte range, used to key annotations to their target.
pub(crate) type Target = (usize, usize);

/// A declared `---@class`: parents plus *own* members (inherited members
/// are merged on demand by [`TypeEnv::class_shape`]).
#[derive(Debug, Default)]
pub(crate) struct ClassDef {
    pub parents: Vec<String>,
    pub fields: BTreeMap<String, FieldTy>,
    pub indexers: Vec<(Ty, Ty)>,
}

/// A declared `---@enum`: member name → value type, plus the union of all
/// member values (what the enum *type* accepts).
#[derive(Debug)]
pub(crate) struct EnumDef {
    pub members: BTreeMap<String, Ty>,
    pub value_union: Ty,
}

/// Everything the annotations of one file declare.
#[derive(Debug, Default)]
pub struct TypeEnv {
    classes: BTreeMap<String, ClassDef>,
    enums: BTreeMap<String, EnumDef>,
    /// Annotated functions by (dotted) name — `f`, `M.helper`.
    functions: BTreeMap<String, FunctionTy>,
    /// `---@type` annotations keyed by their target `local` statement.
    typed_locals: HashMap<Target, Vec<Ty>>,
    /// Function signatures keyed by their target statement (for return
    /// checking inside the body).
    fn_sigs: HashMap<Target, FunctionTy>,
    /// References to undeclared type names (LB0305): `(name, span)`.
    pub(crate) unknown_names: Vec<(String, luacats::Span)>,
}

impl TypeEnv {
    /// Build the environment for one parsed file.
    #[must_use]
    pub fn build(parse: &lua::Parse) -> TypeEnv {
        let items = luacats::harvest(parse);
        let mut decl = Declared::default();
        for item in &items {
            for tag in &item.block.tags {
                match tag {
                    Tag::Class(c) if !c.name.is_empty() => {
                        decl.classes.insert(c.name.clone());
                    }
                    Tag::Alias(a) if !a.name.is_empty() => {
                        decl.aliases.insert(a.name.clone(), a.clone());
                    }
                    Tag::Enum(e) if !e.name.is_empty() => {
                        decl.enums.insert(e.name.clone());
                    }
                    _ => {}
                }
            }
        }

        let mut env = TypeEnv::default();
        let mut lowerer = Lowerer::new(&decl);
        let root = parse.syntax();
        for item in &items {
            lowerer.generics = item
                .block
                .tags
                .iter()
                .filter_map(|tag| match tag {
                    Tag::Generic(g) => Some(g.params.iter().map(|p| p.name.clone())),
                    _ => None,
                })
                .flatten()
                .collect();
            env.absorb_block(item, &mut lowerer, &root);
        }
        env.unknown_names = std::mem::take(&mut lowerer.unknown_names);
        env
    }

    /// Process one annotation block: class/field members, function
    /// signatures, `---@type` locals, and enums.
    fn absorb_block(
        &mut self,
        item: &luacats::AnnotatedItem,
        lowerer: &mut Lowerer<'_>,
        root: &SyntaxNode,
    ) {
        let mut current_class: Option<String> = None;
        let mut params: Vec<&ParamTag> = Vec::new();
        let mut returns: Vec<&ReturnTag> = Vec::new();
        let mut types: Option<Vec<Ty>> = None;

        for tag in &item.block.tags {
            match tag {
                Tag::Class(c) if !c.name.is_empty() => {
                    let parents = c
                        .parents
                        .iter()
                        .filter_map(|p| match &p.kind {
                            TypeExprKind::Named { name, .. } => Some(name.clone()),
                            _ => None,
                        })
                        .collect();
                    self.classes.insert(
                        c.name.clone(),
                        ClassDef {
                            parents,
                            ..ClassDef::default()
                        },
                    );
                    current_class = Some(c.name.clone());
                }
                Tag::Field(f) => {
                    let Some(class) = current_class
                        .as_ref()
                        .and_then(|name| self.classes.get_mut(name))
                    else {
                        continue; // a stray @field outside a @class block
                    };
                    let ty = lowerer.lower(&f.ty);
                    match &f.key {
                        FieldKey::Name(name) => {
                            class.fields.insert(
                                name.clone(),
                                FieldTy {
                                    ty,
                                    optional: f.optional,
                                },
                            );
                        }
                        FieldKey::Indexer(key) => {
                            let key = lowerer.lower(key);
                            class.indexers.push((key, ty));
                        }
                    }
                }
                Tag::Param(p) => params.push(p),
                Tag::Return(r) => returns.push(r),
                Tag::Type(t) => {
                    types = Some(t.types.iter().map(|ty| lowerer.lower(ty)).collect());
                }
                Tag::Enum(e) if !e.name.is_empty() => {
                    let def = enum_def(e, item.target, root);
                    self.enums.insert(e.name.clone(), def);
                }
                _ => {}
            }
        }

        let target = item.target.map(|span| (span.start, span.end));
        if (!params.is_empty() || !returns.is_empty())
            && let Some(target) = target
        {
            self.attach_function(&params, &returns, target, lowerer, root);
        }
        if let (Some(types), Some(target)) = (types, target) {
            self.typed_locals.insert(target, types);
        }
    }

    /// Build a [`FunctionTy`] from `@param`/`@return` tags, reconcile it
    /// with the target function's AST parameter list, and register it under
    /// the function's name.
    fn attach_function(
        &mut self,
        params: &[&ParamTag],
        returns: &[&ReturnTag],
        target: Target,
        lowerer: &mut Lowerer<'_>,
        root: &SyntaxNode,
    ) {
        let mut func = FunctionTy::default();
        for param in params {
            let ty = lowerer.lower(&param.ty);
            if param.vararg {
                func.varargs = Some(ty);
            } else {
                func.params.push(ParamTy {
                    name: param.name.clone(),
                    ty,
                    optional: param.optional,
                });
            }
        }
        func.has_return_annotation = !returns.is_empty();
        for tag in returns {
            for it in &tag.items {
                if it.vararg {
                    func.returns_vararg = true;
                }
                func.returns.push(lowerer.lower(&it.ty));
            }
        }

        let Some(stmt) = stmt_at(root, target) else {
            return;
        };
        let (name, param_list) = match &stmt {
            Stmt::LocalFunction(f) => (f.name().map(|t| t.text().to_string()), f.param_list()),
            Stmt::FunctionDecl(f) => {
                let name = f.name().and_then(|n| {
                    // Methods (`function M:m()`) have an implicit `self`
                    // parameter — TODO(P1): resolve method calls; skipped
                    // from the callable map for now.
                    if n.is_method() {
                        None
                    } else {
                        let joined: Vec<String> =
                            n.segments().map(|s| s.text().to_string()).collect();
                        Some(joined.join("."))
                    }
                });
                (name, f.param_list())
            }
            Stmt::Local(l) => {
                let value = l.values().and_then(|v| v.exprs().next());
                let Some(Expr::Function(f)) = value else {
                    return;
                };
                (
                    l.names()
                        .next()
                        .and_then(|n| n.name())
                        .map(|t| t.text().to_string()),
                    f.param_list(),
                )
            }
            _ => return,
        };

        // Reconcile with the real parameter list: unannotated trailing
        // parameters become optional `unknown` (permissive — partial
        // annotation must not manufacture arity errors), and an
        // unannotated `...` still lifts the arity ceiling.
        if let Some(list) = param_list {
            let mut ast_names: Vec<String> = Vec::new();
            let mut ast_vararg = false;
            for p in list.params() {
                if p.is_vararg() {
                    ast_vararg = true;
                } else if let Some(name) = p.name() {
                    ast_names.push(name.text().to_string());
                }
            }
            for name in ast_names.iter().skip(func.params.len()) {
                func.params.push(ParamTy {
                    name: name.clone(),
                    ty: Ty::Unknown,
                    optional: true,
                });
            }
            if ast_vararg && func.varargs.is_none() {
                func.varargs = Some(Ty::Unknown);
            }
        }

        if let Some(name) = name {
            self.functions.insert(name, func.clone());
        }
        self.fn_sigs.insert(target, func);
    }

    // --- lookups -----------------------------------------------------

    /// The merged structural shape of a class: parents first (depth-first),
    /// own members overriding, with a cycle guard.
    pub(crate) fn class_shape(&self, name: &str) -> Option<TableTy> {
        if !self.classes.contains_key(name) {
            return None;
        }
        let mut shape = TableTy::default();
        let mut seen = HashSet::new();
        self.collect_class(name, &mut shape, &mut seen);
        Some(shape)
    }

    fn collect_class(&self, name: &str, shape: &mut TableTy, seen: &mut HashSet<String>) {
        if !seen.insert(name.to_string()) {
            return;
        }
        let Some(def) = self.classes.get(name) else {
            return;
        };
        for parent in &def.parents {
            self.collect_class(parent, shape, seen);
        }
        for (field, ty) in &def.fields {
            shape.fields.insert(field.clone(), ty.clone());
        }
        shape.indexers.extend(def.indexers.iter().cloned());
    }

    pub(crate) fn enum_member(&self, enum_name: &str, member: &str) -> Option<&Ty> {
        self.enums.get(enum_name)?.members.get(member)
    }

    /// Resolve a [`Ty::Named`] reference to its structural type: a class
    /// becomes its table shape, an enum the union of its member values.
    pub(crate) fn resolve_named(&self, name: &str) -> Option<Ty> {
        if let Some(shape) = self.class_shape(name) {
            return Some(Ty::Table(Box::new(shape)));
        }
        self.enums.get(name).map(|e| e.value_union.clone())
    }

    pub(crate) fn function(&self, name: &str) -> Option<&FunctionTy> {
        self.functions.get(name)
    }

    pub(crate) fn typed_local(&self, target: Target) -> Option<&[Ty]> {
        self.typed_locals.get(&target).map(Vec::as_slice)
    }

    pub(crate) fn fn_sig(&self, target: Target) -> Option<&FunctionTy> {
        self.fn_sigs.get(&target)
    }
}

/// The innermost statement whose range is exactly `target`.
fn stmt_at(root: &SyntaxNode, target: Target) -> Option<Stmt> {
    root.descendants()
        .filter(|node| {
            let range = node.text_range();
            (usize::from(range.start()), usize::from(range.end())) == target
        })
        .find_map(Stmt::cast)
}

/// Build an [`EnumDef`] from the table constructor the `---@enum` annotates.
fn enum_def(tag: &luacats::EnumTag, target: Option<luacats::Span>, root: &SyntaxNode) -> EnumDef {
    let mut members = BTreeMap::new();
    let table = target
        .and_then(|span| stmt_at(root, (span.start, span.end)))
        .and_then(|stmt| match stmt {
            Stmt::Local(local) => enum_table(&local),
            _ => None,
        });
    if let Some(table) = table {
        for field in table.fields() {
            let lua::ast::TableField::Name(named) = field else {
                continue;
            };
            let Some(name) = named.name() else {
                continue;
            };
            let value = if tag.key {
                // `---@enum (key)`: the enum's values are its *keys*.
                Some(Ty::StringLit(name.text().to_string()))
            } else {
                named.value().as_ref().and_then(literal_ty)
            };
            members.insert(name.text().to_string(), value.unwrap_or(Ty::Unknown));
        }
    }
    let value_union = Ty::union(members.values().cloned().collect());
    EnumDef {
        members,
        value_union,
    }
}

fn enum_table(local: &LocalStmt) -> Option<lua::ast::TableExpr> {
    match local.values()?.exprs().next()? {
        Expr::Table(table) => Some(table),
        _ => None,
    }
}

/// The literal type of a literal expression, if it is one.
pub(crate) fn literal_ty(expr: &Expr) -> Option<Ty> {
    let Expr::Literal(lit) = expr else {
        return None;
    };
    let token = lit.token()?;
    Some(match token.kind() {
        SyntaxKind::NIL_KW => Ty::Nil,
        SyntaxKind::TRUE_KW => Ty::BoolLit(true),
        SyntaxKind::FALSE_KW => Ty::BoolLit(false),
        SyntaxKind::NUMBER => Ty::NumberLit(token.text().to_string()),
        SyntaxKind::STRING => Ty::StringLit(unquote_lua(token.text())),
        _ => return None,
    })
}

/// Strip the delimiters from a Lua string literal (quotes or long
/// brackets). Escape sequences are kept verbatim (MVP: literal-type
/// comparison is textual).
pub(crate) fn unquote_lua(raw: &str) -> String {
    let bytes = raw.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[bytes.len() - 1] == bytes[0]
    {
        return raw[1..raw.len() - 1].to_string();
    }
    if let Some(rest) = raw.strip_prefix('[') {
        let level = rest.bytes().take_while(|&b| b == b'=').count();
        let open = level + 2;
        let close = format!("]{}]", "=".repeat(level));
        if raw.len() >= open + close.len() && raw.ends_with(close.as_str()) {
            return raw[open..raw.len() - close.len()].to_string();
        }
    }
    raw.to_string()
}
