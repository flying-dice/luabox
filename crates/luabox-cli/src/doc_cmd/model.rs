//! The documentation model (SPEC.md §13): a renderer-independent view of
//! everything `luabox doc` documents, extracted from two producers:
//!
//! - **LuaCATS annotations** in `.lua` files (`luabox_syntax::luacats`):
//!   classes (+fields), functions (`@param`/`@return`), aliases, enums,
//!   plain doc lines, `@deprecated`, `---@impl` conformance assertions.
//! - **`.luab` shape declarations** (`luabox_syntax::shape`): structs
//!   (fields, sealed/open), traits (fns, supertraits), impls, type
//!   aliases — each with its `///` doc comments.
//!
//! Types are captured as *rendered strings* (the same source-ish rendering
//! the LSP hover uses); cross-linking happens later in the renderer via one
//! global name table, so the model stays plain data.

use std::collections::{BTreeMap, HashMap, HashSet};

use luabox_syntax::lua::ast::{self, AstNode};
use luabox_syntax::lua::{self, SyntaxKind, SyntaxNode};
use luabox_syntax::shape::ast::{
    AstNode as ShapeAstNode, Item, ShapeFile, TraitFn, TypeRef as ShapeTypeRef,
};
use luabox_syntax::shape::{self, ShapeSyntaxKind};
use luabox_syntax::{Dialect, luacats};

/// The whole documented surface of one project.
#[derive(Debug, Default)]
pub struct DocModel {
    /// The package name (manifest `[package] name`, else the directory).
    pub package: String,
    /// One entry per project `.lua` file, in walk order.
    pub modules: Vec<Module>,
    /// One entry per `.luab` shape module, in walk order.
    pub shape_modules: Vec<ShapeModule>,
    /// Every trait-conformance assertion (`.luab` `impl` items plus Lua
    /// `---@impl` tags), project-wide.
    pub impls: Vec<ImplDoc>,
}

/// One documented `.lua` file.
#[derive(Debug)]
pub struct Module {
    /// Dotted module name derived from the path (`src/a/b.lua` → `a.b`).
    pub name: String,
    /// Module-level doc text (markdown).
    pub docs: String,
    /// Module-level functions (methods live on their class instead).
    pub functions: Vec<FunctionDoc>,
    pub classes: Vec<ClassDoc>,
    pub aliases: Vec<AliasDoc>,
    pub enums: Vec<EnumDoc>,
}

/// A documented function or method.
#[derive(Debug)]
pub struct FunctionDoc {
    /// Display name: `f`, `M.helper`, `Class:method`.
    pub name: String,
    /// Parameters in AST order, enriched from `@param` tags.
    pub params: Vec<ParamDoc>,
    /// Return values from `@return` tags.
    pub returns: Vec<ReturnDoc>,
    /// Doc text (markdown).
    pub docs: String,
    /// Whether the block carries `@deprecated`.
    pub deprecated: bool,
}

impl FunctionDoc {
    /// Plain-text signature, rendered like the LSP hover:
    /// `function name(a: number, b?: string): number`.
    pub fn signature(&self) -> String {
        let params: Vec<String> = self
            .params
            .iter()
            .map(|p| match &p.ty {
                Some(ty) => format!("{}: {ty}", p.name),
                None => p.name.clone(),
            })
            .collect();
        let mut sig = format!("function {}({})", self.name, params.join(", "));
        if !self.returns.is_empty() {
            let rets: Vec<&str> = self.returns.iter().map(|r| r.ty.as_str()).collect();
            sig.push_str(": ");
            sig.push_str(&rets.join(", "));
        }
        sig
    }
}

/// One function parameter.
#[derive(Debug)]
pub struct ParamDoc {
    pub name: String,
    /// Rendered type from `@param`, when annotated.
    pub ty: Option<String>,
    pub optional: bool,
    pub desc: Option<String>,
}

/// One function return value.
#[derive(Debug)]
pub struct ReturnDoc {
    pub ty: String,
    pub name: Option<String>,
    pub desc: Option<String>,
}

/// A `---@class` declaration.
#[derive(Debug)]
pub struct ClassDoc {
    pub name: String,
    pub exact: bool,
    /// Parent class names (named parents only).
    pub parents: Vec<String>,
    /// The class's own `@field`s, in declaration order.
    pub fields: Vec<FieldDoc>,
    /// Functions declared as `Class:m` / `Class.m` in the same module.
    pub methods: Vec<FunctionDoc>,
    pub docs: String,
}

/// One `@field` of a class.
#[derive(Debug)]
pub struct FieldDoc {
    /// Field name, or `[K]` for an indexer.
    pub name: String,
    pub ty: String,
    pub optional: bool,
    /// `private` / `protected` / `package`, when scoped.
    pub scope: Option<String>,
    pub desc: Option<String>,
}

/// A `---@alias` declaration.
#[derive(Debug)]
pub struct AliasDoc {
    pub name: String,
    /// The aliased type (single-line form).
    pub ty: Option<String>,
    /// `---|` members (multiline literal-union form): `(type, desc)`.
    pub members: Vec<(String, Option<String>)>,
    pub docs: String,
}

/// A `---@enum` declaration.
#[derive(Debug)]
pub struct EnumDoc {
    pub name: String,
    /// Whether it is a `(key)` enum.
    pub key: bool,
    pub docs: String,
}

/// One trait-conformance assertion: `impl Trait for Struct;` or
/// `---@impl Trait for Struct`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplDoc {
    pub trait_name: String,
    pub struct_name: String,
}

/// One documented `.luab` shape module.
#[derive(Debug)]
pub struct ShapeModule {
    /// The module name (file stem — SHAPES.md §6 resolution key).
    pub name: String,
    pub structs: Vec<StructDoc>,
    pub traits: Vec<TraitDoc>,
    pub aliases: Vec<ShapeAliasDoc>,
    /// `impl` items declared in this module.
    pub impls: Vec<ImplDoc>,
}

/// A `.luab` `struct` declaration.
#[derive(Debug)]
pub struct StructDoc {
    pub name: String,
    /// Generic parameters as display strings (`T`, `T: Bound + Other`).
    pub generics: Vec<String>,
    pub fields: Vec<ShapeFieldDoc>,
    /// `true` unless the struct carries the `..` open marker (SHAPES.md §3).
    pub sealed: bool,
    pub docs: String,
}

/// One field of a `.luab` struct. Nil-optionality is part of the rendered
/// type (`number?`), not a separate flag.
#[derive(Debug)]
pub struct ShapeFieldDoc {
    pub name: String,
    pub ty: String,
    pub docs: String,
}

/// A `.luab` `trait` declaration.
#[derive(Debug)]
pub struct TraitDoc {
    pub name: String,
    pub generics: Vec<String>,
    pub supertraits: Vec<String>,
    pub fns: Vec<TraitFnDoc>,
    pub docs: String,
}

/// One required function of a `.luab` trait.
#[derive(Debug)]
pub struct TraitFnDoc {
    pub name: String,
    /// Rendered signature: `fn area(self) -> number`.
    pub sig: String,
    pub docs: String,
}

/// A `.luab` `type` alias.
#[derive(Debug)]
pub struct ShapeAliasDoc {
    pub name: String,
    pub generics: Vec<String>,
    pub ty: String,
    pub docs: String,
}

// === Module naming ========================================================

/// Derive the dotted module name from a root-relative path:
/// `src/geometry/circle.lua` → `geometry.circle` (the conventional `src/`
/// prefix is stripped; a trailing `.init` collapses onto its directory).
pub fn module_name(rel: &str) -> String {
    let path = rel.strip_prefix("src/").unwrap_or(rel);
    let path = path
        .strip_suffix(".lua")
        .or_else(|| path.strip_suffix(".luab"))
        .unwrap_or(path);
    let dotted = path.replace('/', ".");
    match dotted.strip_suffix(".init") {
        Some(parent) => parent.to_string(),
        None => dotted,
    }
}

// === Lua extraction =======================================================

/// Extract the documentation model of one `.lua` file.
pub fn lua_module(name: &str, source: &str, dialect: Dialect) -> (Module, Vec<ImplDoc>) {
    let parse = lua::parse(source, dialect);
    let root = parse.syntax();
    let items = luacats::harvest(&parse);

    let mut classes: Vec<ClassDoc> = Vec::new();
    let mut aliases: Vec<AliasDoc> = Vec::new();
    let mut enums: Vec<EnumDoc> = Vec::new();
    let mut impls: Vec<ImplDoc> = Vec::new();
    // Local binding name → class name (`---@class geometry.Circle` bound to
    // `local Circle = {}` makes `Circle:m` a method of `geometry.Circle`).
    let mut bindings: HashMap<String, String> = HashMap::new();

    for item in &items {
        let docs = docs_of(&item.block, source);
        for tag in &item.block.tags {
            match tag {
                luacats::Tag::Class(class) => {
                    if let Some(binding) = item.target.and_then(|t| binding_name(&root, t))
                        && binding != class.name
                    {
                        bindings.insert(binding, class.name.clone());
                    }
                    classes.push(class_doc(class, &item.block, docs.clone()));
                }
                luacats::Tag::Alias(alias) => aliases.push(AliasDoc {
                    name: alias.name.clone(),
                    ty: alias.ty.as_ref().map(render_type),
                    members: alias
                        .members
                        .iter()
                        .map(|m| (render_type(&m.ty), m.desc.clone()))
                        .collect(),
                    docs: docs.clone(),
                }),
                luacats::Tag::Enum(en) => enums.push(EnumDoc {
                    name: en.name.clone(),
                    key: en.key,
                    docs: docs.clone(),
                }),
                luacats::Tag::Impl(imp) => impls.push(ImplDoc {
                    trait_name: imp.trait_name.clone(),
                    struct_name: imp.struct_name.clone(),
                }),
                _ => {}
            }
        }
    }

    // Functions, then attach `Class:m` / `Class.m` declarations as methods.
    let mut functions = Vec::new();
    for func in extract_functions(&root, &items, source) {
        match method_class(&func.name, &bindings, &classes) {
            Some(index) => classes[index].methods.push(func),
            None => functions.push(func),
        }
    }

    let module = Module {
        name: name.to_string(),
        docs: module_docs(&items, &root, source),
        functions,
        classes,
        aliases,
        enums,
    };
    (module, impls)
}

fn class_doc(tag: &luacats::ClassTag, block: &luacats::AnnotationBlock, docs: String) -> ClassDoc {
    let fields = block
        .tags
        .iter()
        .filter_map(|t| match t {
            luacats::Tag::Field(field) => Some(field_doc(field)),
            _ => None,
        })
        .collect();
    ClassDoc {
        name: tag.name.clone(),
        exact: tag.exact,
        parents: tag.parents.iter().filter_map(named_of).collect(),
        fields,
        methods: Vec::new(),
        docs,
    }
}

fn field_doc(field: &luacats::FieldTag) -> FieldDoc {
    let name = match &field.key {
        luacats::FieldKey::Name(name) => name.clone(),
        luacats::FieldKey::Indexer(key) => format!("[{}]", render_type(key)),
    };
    let scope = match field.scope {
        Some(luacats::FieldScope::Private) => Some("private".to_string()),
        Some(luacats::FieldScope::Protected) => Some("protected".to_string()),
        Some(luacats::FieldScope::Package) => Some("package".to_string()),
        Some(luacats::FieldScope::Public) | None => None,
    };
    FieldDoc {
        name,
        ty: render_type(&field.ty),
        optional: field.optional,
        scope,
        desc: field.desc.clone(),
    }
}

/// The class (index into `classes`) a function named `C:m` / `C.m` belongs
/// to, resolving the receiver through the local-binding table.
fn method_class(
    name: &str,
    bindings: &HashMap<String, String>,
    classes: &[ClassDoc],
) -> Option<usize> {
    let head = name
        .split_once(':')
        .map_or_else(|| name.rsplit_once('.').map(|(h, _)| h), |(h, _)| Some(h))?;
    let class_name = bindings.get(head).map_or(head, String::as_str);
    classes.iter().position(|c| c.name == class_name)
}

/// The first name a class-annotated statement binds (`local Circle = {}`,
/// `Circle = {}`), used to attach `Circle:m` methods to the class.
fn binding_name(root: &SyntaxNode, span: luacats::Span) -> Option<String> {
    root.descendants()
        .filter(|n| {
            let r = n.text_range();
            usize::from(r.start()) == span.start && usize::from(r.end()) == span.end
        })
        .find_map(|node| match node.kind() {
            SyntaxKind::LOCAL_STMT => ast::LocalStmt::cast(node)?
                .names()
                .next()?
                .name()
                .map(|t| t.text().to_string()),
            SyntaxKind::ASSIGN_STMT => {
                let assign = ast::AssignStmt::cast(node)?;
                match assign.targets()?.exprs().next()? {
                    ast::Expr::Name(name) => name.name().map(|t| t.text().to_string()),
                    _ => None,
                }
            }
            _ => None,
        })
}

/// Module-level docs: the first harvested block that is pure prose (doc
/// lines with no declaration-shaped tags) and is not a function's doc block.
fn module_docs(items: &[luacats::AnnotatedItem], root: &SyntaxNode, source: &str) -> String {
    let Some(item) = items.first() else {
        return String::new();
    };
    let prose_only = item.block.tags.iter().all(|t| {
        matches!(
            t,
            luacats::Tag::Meta(_) | luacats::Tag::See(_) | luacats::Tag::Version(_)
        )
    });
    if !prose_only || item.block.docs.is_empty() {
        return String::new();
    }
    // A block attached to a function declaration documents that function.
    if let Some(span) = item.target
        && targets_function(root, span)
    {
        return String::new();
    }
    docs_of(&item.block, source)
}

/// Whether the statement at `span` declares (or binds) a function. Wrapper
/// nodes (e.g. a block holding a single statement) can share the exact
/// range, so every matching node is considered.
fn targets_function(root: &SyntaxNode, span: luacats::Span) -> bool {
    root.descendants()
        .filter(|n| {
            let r = n.text_range();
            usize::from(r.start()) == span.start && usize::from(r.end()) == span.end
        })
        .any(|node| match node.kind() {
            SyntaxKind::FUNCTION_DECL_STMT | SyntaxKind::LOCAL_FUNCTION_STMT => true,
            SyntaxKind::LOCAL_STMT => ast::LocalStmt::cast(node)
                .and_then(|l| l.values())
                .and_then(|v| v.exprs().next())
                .is_some_and(|e| matches!(e, ast::Expr::Function(_))),
            _ => false,
        })
}

/// The joined plain doc lines of a block. The harvest drops empty `---`
/// lines, so paragraph breaks are reconstructed from the doc-line spans:
/// more than one newline between consecutive lines means a blank line stood
/// between them.
fn docs_of(block: &luacats::AnnotationBlock, source: &str) -> String {
    let mut out = String::new();
    let mut prev_end: Option<usize> = None;
    for line in &block.docs {
        if let Some(end) = prev_end {
            let gap = source.get(end..line.span.start).unwrap_or("");
            if gap.bytes().filter(|&b| b == b'\n').count() > 1 {
                out.push('\n');
            }
            out.push('\n');
        }
        out.push_str(&line.text);
        prev_end = Some(line.span.end);
    }
    out
}

/// Every function declaration in the file (`function f`, `function M.g`,
/// `function C:m`, `local function h`, `local f = function`), enriched with
/// its annotation block — the same set the LSP hover surfaces.
fn extract_functions(
    root: &SyntaxNode,
    items: &[luacats::AnnotatedItem],
    source: &str,
) -> Vec<FunctionDoc> {
    let mut out = Vec::new();
    for node in root.descendants() {
        let (name, params, stmt_range) = match node.kind() {
            SyntaxKind::FUNCTION_DECL_STMT => {
                let Some(decl) = ast::FunctionDeclStmt::cast(node.clone()) else {
                    continue;
                };
                let Some(name) = function_decl_name(&decl) else {
                    continue;
                };
                (name, decl.param_list(), node.text_range())
            }
            SyntaxKind::LOCAL_FUNCTION_STMT => {
                let Some(decl) = ast::LocalFunctionStmt::cast(node.clone()) else {
                    continue;
                };
                let Some(token) = decl.name() else { continue };
                (
                    token.text().to_string(),
                    decl.param_list(),
                    node.text_range(),
                )
            }
            SyntaxKind::LOCAL_STMT => {
                let Some(local) = ast::LocalStmt::cast(node.clone()) else {
                    continue;
                };
                let Some(ast::Expr::Function(func)) = local.values().and_then(|v| v.exprs().next())
                else {
                    continue;
                };
                let Some(token) = local.names().next().and_then(|n| n.name()) else {
                    continue;
                };
                (
                    token.text().to_string(),
                    func.param_list(),
                    node.text_range(),
                )
            }
            _ => continue,
        };
        let item = items.iter().find(|item| {
            item.target.is_some_and(|t| {
                t.start == usize::from(stmt_range.start()) && t.end == usize::from(stmt_range.end())
            })
        });
        out.push(function_doc(name, params.as_ref(), item, source));
    }
    out
}

/// The display name of a `function a.b:c` declaration.
fn function_decl_name(decl: &ast::FunctionDeclStmt) -> Option<String> {
    let name = decl.name()?;
    let segments: Vec<String> = name.segments().map(|t| t.text().to_string()).collect();
    let last = segments.last()?.clone();
    if name.is_method() && segments.len() >= 2 {
        Some(format!(
            "{}:{last}",
            segments[..segments.len() - 1].join(".")
        ))
    } else {
        Some(segments.join("."))
    }
}

fn function_doc(
    name: String,
    params: Option<&ast::ParamList>,
    item: Option<&luacats::AnnotatedItem>,
    source: &str,
) -> FunctionDoc {
    let mut param_tags: HashMap<&str, &luacats::ParamTag> = HashMap::new();
    let mut returns: Vec<ReturnDoc> = Vec::new();
    let mut deprecated = false;
    if let Some(item) = item {
        for tag in &item.block.tags {
            match tag {
                luacats::Tag::Param(p) => {
                    param_tags.insert(p.name.as_str(), p);
                }
                luacats::Tag::Return(r) => {
                    for ret in &r.items {
                        returns.push(ReturnDoc {
                            ty: render_type(&ret.ty),
                            name: ret.name.clone(),
                            desc: r.desc.clone(),
                        });
                    }
                }
                luacats::Tag::Deprecated(_) => deprecated = true,
                _ => {}
            }
        }
    }
    let mut rendered: Vec<ParamDoc> = Vec::new();
    if let Some(list) = params {
        for param in list.params() {
            let pname = if param.is_vararg() {
                "...".to_string()
            } else {
                match param.name() {
                    Some(token) => token.text().to_string(),
                    None => continue,
                }
            };
            let tag = param_tags.get(pname.as_str());
            rendered.push(ParamDoc {
                ty: tag.map(|t| render_type(&t.ty)),
                optional: tag.is_some_and(|t| t.optional),
                desc: tag.and_then(|t| t.desc.clone()),
                name: pname,
            });
        }
    }
    FunctionDoc {
        name,
        params: rendered,
        returns,
        docs: item.map(|i| docs_of(&i.block, source)).unwrap_or_default(),
        deprecated,
    }
}

/// The underlying named type of an annotated type, peeling `?`/parens.
fn named_of(ty: &luacats::TypeExpr) -> Option<String> {
    match &ty.kind {
        luacats::TypeExprKind::Named { name, .. } => Some(name.clone()),
        luacats::TypeExprKind::Optional(inner) | luacats::TypeExprKind::Paren(inner) => {
            named_of(inner)
        }
        _ => None,
    }
}

/// Render a LuaCATS type expression back to source-ish text (the LSP hover
/// rendering).
pub fn render_type(ty: &luacats::TypeExpr) -> String {
    use luacats::TypeExprKind;
    match &ty.kind {
        TypeExprKind::Named { name, args } => {
            if args.is_empty() {
                name.clone()
            } else {
                let args: Vec<String> = args.iter().map(render_type).collect();
                format!("{name}<{}>", args.join(", "))
            }
        }
        TypeExprKind::Optional(inner) => format!("{}?", render_type(inner)),
        TypeExprKind::Array(inner) => format!("{}[]", render_type(inner)),
        TypeExprKind::Union(members) => members
            .iter()
            .map(render_type)
            .collect::<Vec<_>>()
            .join("|"),
        TypeExprKind::Tuple(members) => {
            let members: Vec<String> = members.iter().map(render_type).collect();
            format!("[{}]", members.join(", "))
        }
        TypeExprKind::Table(fields) => {
            let fields: Vec<String> = fields
                .iter()
                .map(|f| match f {
                    luacats::TableField::Named { name, optional, ty } => {
                        let q = if *optional { "?" } else { "" };
                        format!("{name}{q}: {}", render_type(ty))
                    }
                    luacats::TableField::Indexer { key, value } => {
                        format!("[{}]: {}", render_type(key), render_type(value))
                    }
                })
                .collect();
            format!("{{ {} }}", fields.join(", "))
        }
        TypeExprKind::Fun { params, returns } => {
            let params: Vec<String> = params
                .iter()
                .map(|p| match &p.ty {
                    Some(ty) => format!("{}: {}", p.name, render_type(ty)),
                    None => p.name.clone(),
                })
                .collect();
            let mut out = format!("fun({})", params.join(", "));
            if !returns.is_empty() {
                let rets: Vec<String> = returns.iter().map(|r| render_type(&r.ty)).collect();
                out.push_str(": ");
                out.push_str(&rets.join(", "));
            }
            out
        }
        TypeExprKind::StringLit(raw) | TypeExprKind::NumberLit(raw) => raw.clone(),
        TypeExprKind::BoolLit(b) => b.to_string(),
        TypeExprKind::Backtick(inner) => format!("`{inner}`"),
        TypeExprKind::Paren(inner) => format!("({})", render_type(inner)),
        TypeExprKind::Error => "?".to_string(),
    }
}

// === Shape extraction =====================================================

/// Extract the documentation model of one `.luab` shape module.
pub fn shape_module(name: &str, source: &str) -> ShapeModule {
    let parse = shape::parse(source);
    let root = parse.syntax();
    let docs = DocComments::index(&root);

    let mut module = ShapeModule {
        name: name.to_string(),
        structs: Vec::new(),
        traits: Vec::new(),
        aliases: Vec::new(),
        impls: Vec::new(),
    };
    let Some(file) = ShapeFile::cast(root) else {
        return module;
    };
    for item in file.items() {
        match item {
            Item::Struct(st) => {
                let Some(name) = st.name() else { continue };
                module.structs.push(StructDoc {
                    docs: docs.before(st.syntax()),
                    generics: generic_display(st.generic_params()),
                    fields: st
                        .fields()
                        .map(|f| ShapeFieldDoc {
                            docs: docs.before(f.syntax()),
                            name: f.name().unwrap_or_default(),
                            ty: f.ty().as_ref().map(render_shape_type).unwrap_or_default(),
                        })
                        .collect(),
                    sealed: !st.is_open(),
                    name,
                });
            }
            Item::Trait(tr) => {
                let Some(name) = tr.name() else { continue };
                module.traits.push(TraitDoc {
                    docs: docs.before(tr.syntax()),
                    generics: generic_display(tr.generic_params()),
                    supertraits: tr.supertraits(),
                    fns: tr.fns().filter_map(|f| trait_fn_doc(&f, &docs)).collect(),
                    name,
                });
            }
            Item::Impl(imp) => {
                if let (Some(trait_name), Some(struct_name)) = (imp.trait_name(), imp.struct_name())
                {
                    module.impls.push(ImplDoc {
                        trait_name,
                        struct_name,
                    });
                }
            }
            Item::Alias(alias) => {
                let Some(name) = alias.name() else { continue };
                module.aliases.push(ShapeAliasDoc {
                    docs: docs.before(alias.syntax()),
                    generics: generic_display(alias.generic_params()),
                    ty: alias
                        .ty()
                        .as_ref()
                        .map(render_shape_type)
                        .unwrap_or_default(),
                    name,
                });
            }
            Item::Use(_) => {}
        }
    }
    module
}

/// Render a parameter list as display strings (`self`, `a: number`).
fn render_shape_params(list: Option<luabox_syntax::shape::ast::ParamList>) -> Vec<String> {
    let Some(list) = list else {
        return Vec::new();
    };
    list.params()
        .map(|p| {
            if p.is_self() {
                "self".to_string()
            } else {
                let ty = p.ty().as_ref().map(render_shape_type).unwrap_or_default();
                format!("{}: {ty}", p.name().unwrap_or_default())
            }
        })
        .collect()
}

fn trait_fn_doc(func: &TraitFn, docs: &DocComments) -> Option<TraitFnDoc> {
    let name = func.name()?;
    let params = render_shape_params(func.params());
    let mut sig = format!("fn {name}({})", params.join(", "));
    let returns = func.returns();
    if !returns.is_empty() {
        let rets: Vec<String> = returns.iter().map(render_shape_type).collect();
        sig.push_str(" -> ");
        sig.push_str(&rets.join(", "));
    }
    Some(TraitFnDoc {
        docs: docs.before(func.syntax()),
        name,
        sig,
    })
}

fn generic_display(params: Option<luabox_syntax::shape::ast::GenericParams>) -> Vec<String> {
    let Some(params) = params else {
        return Vec::new();
    };
    params
        .params()
        .filter_map(|p| {
            let name = p.name()?;
            let bounds = p.bounds();
            if bounds.is_empty() {
                Some(name)
            } else {
                Some(format!("{name}: {}", bounds.join(" + ")))
            }
        })
        .collect()
}

/// Render a `.luab` type expression back to source-ish text.
pub fn render_shape_type(ty: &ShapeTypeRef) -> String {
    match ty {
        ShapeTypeRef::Named(named) => {
            let name = named.name().unwrap_or_default();
            match named.args() {
                Some(args) => {
                    let args: Vec<String> = args.args().map(|a| render_shape_type(&a)).collect();
                    format!("{name}<{}>", args.join(", "))
                }
                None => name,
            }
        }
        ShapeTypeRef::Optional(opt) => match opt.inner() {
            Some(inner) => format!("{}?", render_shape_type(&inner)),
            None => "?".to_string(),
        },
        ShapeTypeRef::Union(union) => union
            .members()
            .map(|m| render_shape_type(&m))
            .collect::<Vec<_>>()
            .join(" | "),
        ShapeTypeRef::Fn(func) => {
            let params = render_shape_params(func.params());
            let mut out = format!("fn({})", params.join(", "));
            let returns = func.returns();
            if !returns.is_empty() {
                let rets: Vec<String> = returns.iter().map(render_shape_type).collect();
                out.push_str(" -> ");
                out.push_str(&rets.join(", "));
            }
            out
        }
        ShapeTypeRef::Paren(paren) => match paren.inner() {
            Some(inner) => format!("({})", render_shape_type(&inner)),
            None => "()".to_string(),
        },
    }
}

/// `///` doc comments indexed by the source offset of the token they
/// precede. Doc comments are trivia in the `.luab` tree (SHAPES.md §2), so
/// they are collected from the token stream rather than the node structure:
/// a run of `///` lines separated by single newlines, ending directly above
/// the item (no blank line), documents that item.
struct DocComments {
    /// `(kind, start, end, newline_count, text)` for every token, in order.
    tokens: Vec<(ShapeSyntaxKind, usize, usize, usize, String)>,
}

impl DocComments {
    fn index(root: &shape::ShapeSyntaxNode) -> Self {
        let tokens = root
            .descendants_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .map(|t| {
                let r = t.text_range();
                let text = t.text().to_string();
                let newlines = text.bytes().filter(|&b| b == b'\n').count();
                (
                    t.kind(),
                    usize::from(r.start()),
                    usize::from(r.end()),
                    newlines,
                    text,
                )
            })
            .collect();
        Self { tokens }
    }

    /// The doc text attached to the node — the `///` run directly above its
    /// first significant token.
    fn before(&self, node: &shape::ShapeSyntaxNode) -> String {
        let Some(start) = node
            .descendants_with_tokens()
            .filter_map(rowan::NodeOrToken::into_token)
            .find(|t| !t.kind().is_trivia())
            .map(|t| usize::from(t.text_range().start()))
        else {
            return String::new();
        };
        let mut lines: Vec<&str> = Vec::new();
        let upto = self
            .tokens
            .partition_point(|&(_, _, end, _, _)| end <= start);
        for (kind, _, _, newlines, text) in self.tokens[..upto].iter().rev() {
            match kind {
                ShapeSyntaxKind::WHITESPACE if *newlines <= 1 => {}
                ShapeSyntaxKind::DOC_COMMENT => {
                    let body = text.strip_prefix("///").unwrap_or(text);
                    lines.push(body.strip_prefix(' ').unwrap_or(body));
                }
                _ => break,
            }
        }
        lines.reverse();
        lines.join("\n")
    }
}

// === Cross-class queries ==================================================

/// All classes of the model, by name (later declarations shadow earlier
/// duplicates — a documented gap for MVP).
pub fn classes_by_name(model: &DocModel) -> BTreeMap<&str, &ClassDoc> {
    let mut map = BTreeMap::new();
    for module in &model.modules {
        for class in &module.classes {
            map.insert(class.name.as_str(), class);
        }
    }
    map
}

/// The fields `class` inherits, grouped per ancestor in
/// nearest-ancestor-first order (rustdoc's "Fields inherited from" notation).
pub fn inherited_fields<'a>(
    classes: &BTreeMap<&str, &'a ClassDoc>,
    class: &ClassDoc,
) -> Vec<(String, Vec<&'a FieldDoc>)> {
    let mut seen: HashSet<String> = HashSet::new();
    seen.insert(class.name.clone());
    let mut out = Vec::new();
    let mut queue: Vec<String> = class.parents.clone();
    while !queue.is_empty() {
        let mut next = Vec::new();
        for parent in queue {
            if !seen.insert(parent.clone()) {
                continue;
            }
            let Some(&info) = classes.get(parent.as_str()) else {
                continue;
            };
            out.push((parent, info.fields.iter().collect()));
            next.extend(info.parents.iter().cloned());
        }
        queue = next;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(source: &str) -> (Module, Vec<ImplDoc>) {
        lua_module("fixture", source, Dialect::Lua54)
    }

    #[test]
    fn function_signature_from_param_and_return_tags() {
        let (m, _) = module(
            "--- Adds two numbers.\n\
             ---@param a number the left addend\n\
             ---@param b number\n\
             ---@return number sum # the total\n\
             local function add(a, b)\n  return a + b\nend\n",
        );
        assert_eq!(m.functions.len(), 1);
        let f = &m.functions[0];
        assert_eq!(f.signature(), "function add(a: number, b: number): number");
        assert_eq!(f.docs, "Adds two numbers.");
        assert_eq!(f.params[0].desc.as_deref(), Some("the left addend"));
        assert_eq!(f.returns[0].name.as_deref(), Some("sum"));
        assert_eq!(f.returns[0].desc.as_deref(), Some("the total"));
        assert!(!f.deprecated);
    }

    #[test]
    fn untyped_and_vararg_params_render_plain() {
        let (m, _) = module("local function f(x, ...)\nend\n");
        assert_eq!(m.functions[0].signature(), "function f(x, ...)");
    }

    #[test]
    fn class_with_fields_methods_and_binding_alias() {
        let (m, _) = module(
            "--- A circle.\n\
             ---@class geometry.Circle: geometry.Shape\n\
             ---@field radius number the radius\n\
             ---@field private id integer\n\
             local Circle = {}\n\
             \n\
             --- Computes the area.\n\
             ---@return number\n\
             function Circle:area()\n  return 3\nend\n",
        );
        assert_eq!(m.classes.len(), 1);
        let c = &m.classes[0];
        assert_eq!(c.name, "geometry.Circle");
        assert_eq!(c.parents, vec!["geometry.Shape".to_string()]);
        assert_eq!(c.fields.len(), 2);
        assert_eq!(c.fields[0].name, "radius");
        assert_eq!(c.fields[0].ty, "number");
        assert_eq!(c.fields[1].scope.as_deref(), Some("private"));
        // `Circle:area` resolves through the local binding to the class.
        assert_eq!(c.methods.len(), 1);
        assert_eq!(c.methods[0].name, "Circle:area");
        assert!(m.functions.is_empty());
    }

    #[test]
    fn alias_and_enum_are_harvested() {
        let (m, _) = module(
            "--- Directions.\n\
             ---@alias Direction\n\
             ---| \"north\" # up\n\
             ---| \"south\"\n\
             \n\
             ---@enum Color\n\
             local Color = { RED = 1 }\n",
        );
        assert_eq!(m.aliases.len(), 1);
        let a = &m.aliases[0];
        assert_eq!(a.name, "Direction");
        assert_eq!(a.members.len(), 2);
        assert_eq!(a.members[0].0, "\"north\"");
        assert_eq!(a.members[0].1.as_deref(), Some("up"));
        assert_eq!(m.enums.len(), 1);
        assert_eq!(m.enums[0].name, "Color");
    }

    #[test]
    fn module_docs_come_from_a_leading_prose_block() {
        let (m, _) = module("--- The geometry toolkit.\n--- With two lines.\n\nlocal x = 1\n");
        assert_eq!(m.docs, "The geometry toolkit.\nWith two lines.");
    }

    #[test]
    fn blank_doc_lines_become_paragraph_breaks() {
        let (m, _) = module("--- First paragraph.\n---\n--- Second paragraph.\n\nlocal x = 1\n");
        assert_eq!(m.docs, "First paragraph.\n\nSecond paragraph.");
    }

    #[test]
    fn function_doc_block_is_not_stolen_as_module_docs() {
        let (m, _) = module("--- Frobnicates.\nlocal function frob()\nend\n");
        assert_eq!(m.docs, "");
        assert_eq!(m.functions[0].docs, "Frobnicates.");
    }

    #[test]
    fn lua_impl_tags_are_collected() {
        let (_, impls) = module(
            "---@struct Circle\nlocal Circle = {}\n\
             ---@impl Shape for Circle\nfunction Circle.area()\n  return 1\nend\n",
        );
        assert_eq!(
            impls,
            vec![ImplDoc {
                trait_name: "Shape".to_string(),
                struct_name: "Circle".to_string(),
            }]
        );
    }

    #[test]
    fn deprecated_is_flagged() {
        let (m, _) = module("---@deprecated use add2\nlocal function add()\nend\n");
        assert!(m.functions[0].deprecated);
    }

    #[test]
    fn shape_structs_traits_impls_and_docs() {
        let sm = shape_module(
            "geometry",
            "/// A 2D point.\n\
             /// Immutable.\n\
             struct Point {\n    /// Horizontal.\n    x: number,\n    y: number?,\n}\n\
             \n\
             struct Bag<T> { items: T, .. }\n\
             \n\
             /// Things with an area.\n\
             trait Shape: Drawable {\n    /// The enclosed area.\n    fn area(self) -> number;\n}\n\
             \n\
             impl Shape for Point;\n\
             \n\
             type Pair<T> = fn(a: T) -> T;\n",
        );
        assert_eq!(sm.structs.len(), 2);
        let point = &sm.structs[0];
        assert_eq!(point.name, "Point");
        assert!(point.sealed);
        assert_eq!(point.docs, "A 2D point.\nImmutable.");
        assert_eq!(point.fields.len(), 2);
        assert_eq!(point.fields[0].name, "x");
        assert_eq!(point.fields[0].ty, "number");
        assert_eq!(point.fields[0].docs, "Horizontal.");
        assert_eq!(point.fields[1].ty, "number?");
        let bag = &sm.structs[1];
        assert!(!bag.sealed);
        assert_eq!(bag.generics, vec!["T".to_string()]);

        assert_eq!(sm.traits.len(), 1);
        let shape = &sm.traits[0];
        assert_eq!(shape.supertraits, vec!["Drawable".to_string()]);
        assert_eq!(shape.fns.len(), 1);
        assert_eq!(shape.fns[0].sig, "fn area(self) -> number");
        assert_eq!(shape.fns[0].docs, "The enclosed area.");

        assert_eq!(
            sm.impls,
            vec![ImplDoc {
                trait_name: "Shape".to_string(),
                struct_name: "Point".to_string(),
            }]
        );
        assert_eq!(sm.aliases.len(), 1);
        assert_eq!(sm.aliases[0].ty, "fn(a: T) -> T");
    }

    #[test]
    fn blank_line_detaches_shape_docs() {
        let sm = shape_module("m", "/// Stray comment.\n\nstruct Point { x: number }\n");
        assert_eq!(sm.structs[0].docs, "");
    }

    #[test]
    fn module_name_derivation() {
        assert_eq!(module_name("src/main.lua"), "main");
        assert_eq!(module_name("src/geometry/circle.lua"), "geometry.circle");
        assert_eq!(module_name("src/geometry/init.lua"), "geometry");
        assert_eq!(module_name("lib/util.lua"), "lib.util");
        assert_eq!(module_name("src/geometry.luab"), "geometry");
    }

    #[test]
    fn inherited_fields_walk_the_parent_chain() {
        let (m, _) = module(
            "---@class Base\n---@field id integer\nlocal Base = {}\n\
             \n\
             ---@class Mid: Base\n---@field label string\nlocal Mid = {}\n\
             \n\
             ---@class Leaf: Mid\n---@field own boolean\nlocal Leaf = {}\n",
        );
        let model = DocModel {
            package: "p".to_string(),
            modules: vec![m],
            shape_modules: Vec::new(),
            impls: Vec::new(),
        };
        let classes = classes_by_name(&model);
        let leaf = classes["Leaf"];
        let inherited = inherited_fields(&classes, leaf);
        assert_eq!(inherited.len(), 2);
        assert_eq!(inherited[0].0, "Mid");
        assert_eq!(inherited[0].1[0].name, "label");
        assert_eq!(inherited[1].0, "Base");
        assert_eq!(inherited[1].1[0].name, "id");
    }
}
