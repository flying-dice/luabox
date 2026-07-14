//! The documentation model (SPEC.md §13): a renderer-independent view of
//! everything `luabox doc` documents, from **LuaCATS annotations** in `.lua`
//! files (`luabox_syntax::luacats`): classes (+fields), functions
//! (`@param`/`@return`), aliases, enums, plain doc lines, `@deprecated`.
//!
//! Types are captured as *rendered strings* (the same source-ish rendering
//! the LSP hover uses); cross-linking happens later in the renderer via one
//! global name table, so the model stays plain data.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use luabox_syntax::lua::ast::{self, AstNode};
use luabox_syntax::lua::{self, SyntaxKind, SyntaxNode};
use luabox_syntax::{Dialect, luacats};

/// The whole documented surface of one project.
#[derive(Debug, Default)]
pub struct DocModel {
    /// The package name (manifest `[package] name`, else the directory).
    pub package: String,
    /// One entry per project `.lua` file, in walk order.
    pub modules: Vec<Module>,
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
    /// `@see` references, in declaration order.
    pub sees: Vec<String>,
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
    /// `@see` references, in declaration order.
    pub sees: Vec<String>,
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

// === Module naming ========================================================

/// Derive the dotted module name from a root-relative path:
/// `src/geometry/circle.lua` → `geometry.circle` (the conventional `src/`
/// prefix is stripped; a trailing `.init` collapses onto its directory).
pub fn module_name(rel: &str) -> String {
    let path = rel.strip_prefix("src/").unwrap_or(rel);
    let path = path.strip_suffix(".lua").unwrap_or(path);
    let dotted = path.replace('/', ".");
    match dotted.strip_suffix(".init") {
        Some(parent) => parent.to_string(),
        None => dotted,
    }
}

// === Lua extraction =======================================================

/// Extract the documentation model of one `.lua` file.
pub fn lua_module(name: &str, source: &str, dialect: Dialect) -> Module {
    let parse = lua::parse(source, dialect);
    let root = parse.syntax();
    let items = luacats::harvest(&parse);

    let mut classes: Vec<ClassDoc> = Vec::new();
    let mut aliases: Vec<AliasDoc> = Vec::new();
    let mut enums: Vec<EnumDoc> = Vec::new();
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

    Module {
        name: name.to_string(),
        docs: module_docs(&items, &root, source),
        functions,
        classes,
        aliases,
        enums,
    }
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
        sees: sees_of(block),
    }
}

/// The `@see` references of a block, in declaration order.
fn sees_of(block: &luacats::AnnotationBlock) -> Vec<String> {
    block
        .tags
        .iter()
        .filter_map(|t| match t {
            luacats::Tag::See(tag) => tag.text.clone(),
            _ => None,
        })
        .collect()
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
        sees: item.map(|i| sees_of(&i.block)).unwrap_or_default(),
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

/// The reverse of [`ClassDoc::parents`] (issue #87): every class's declared
/// parent gains that class back as a subclass/implementor, computed once
/// after every module is harvested (so a parent declared in one module and
/// extended in another still sees the edge). Names, not [`ClassDoc`]
/// references — the renderer already has one global name table (`Links`)
/// to turn a name into a link, and a name is all a parent page needs to
/// list a child that may live in a different module.
pub fn implementors(model: &DocModel) -> BTreeMap<String, Vec<String>> {
    let mut map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for module in &model.modules {
        for class in &module.classes {
            for parent in &class.parents {
                map.entry(parent.clone())
                    .or_default()
                    .insert(class.name.clone());
            }
        }
    }
    map.into_iter()
        .map(|(parent, children)| (parent, children.into_iter().collect()))
        .collect()
}

/// Whether `class`'s page should head its reverse listing "Implementors"
/// (LuaCATS has no interface-vs-class split, so this is a heuristic, not a
/// declared fact): every one of its `@field`s is function-typed and there is
/// at least one, i.e. it reads like a method-only interface
/// (`---@field area fun(self): number`).
/// Anything else — including a class with no fields at all — gets the more
/// neutral "Subclasses", matching rustdoc's trait-vs-struct split without
/// having to fake a distinction LuaCATS doesn't carry.
pub fn is_interface(class: &ClassDoc) -> bool {
    !class.fields.is_empty()
        && class
            .fields
            .iter()
            .all(|f| f.ty.trim_start().starts_with("fun("))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(source: &str) -> Module {
        lua_module("fixture", source, Dialect::Lua54)
    }

    #[test]
    fn function_signature_from_param_and_return_tags() {
        let m = module(
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
        let m = module("local function f(x, ...)\nend\n");
        assert_eq!(m.functions[0].signature(), "function f(x, ...)");
    }

    #[test]
    fn class_with_fields_methods_and_binding_alias() {
        let m = module(
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
        let m = module(
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
        let m = module("--- The geometry toolkit.\n--- With two lines.\n\nlocal x = 1\n");
        assert_eq!(m.docs, "The geometry toolkit.\nWith two lines.");
    }

    #[test]
    fn blank_doc_lines_become_paragraph_breaks() {
        let m = module("--- First paragraph.\n---\n--- Second paragraph.\n\nlocal x = 1\n");
        assert_eq!(m.docs, "First paragraph.\n\nSecond paragraph.");
    }

    #[test]
    fn function_doc_block_is_not_stolen_as_module_docs() {
        let m = module("--- Frobnicates.\nlocal function frob()\nend\n");
        assert_eq!(m.docs, "");
        assert_eq!(m.functions[0].docs, "Frobnicates.");
    }

    #[test]
    fn deprecated_is_flagged() {
        let m = module("---@deprecated use add2\nlocal function add()\nend\n");
        assert!(m.functions[0].deprecated);
    }

    #[test]
    fn see_references_are_harvested_for_functions_and_classes() {
        let m = module(
            "---@see other.frob compare with\n\
             ---@see Point\n\
             local function frob()\nend\n\
             \n\
             ---@class Point\n\
             ---@see geometry.Vec\n\
             local Point = {}\n",
        );
        assert_eq!(
            m.functions[0].sees,
            vec!["other.frob compare with".to_string(), "Point".to_string()]
        );
        assert_eq!(m.classes[0].sees, vec!["geometry.Vec".to_string()]);
    }

    #[test]
    fn module_name_derivation() {
        assert_eq!(module_name("src/main.lua"), "main");
        assert_eq!(module_name("src/geometry/circle.lua"), "geometry.circle");
        assert_eq!(module_name("src/geometry/init.lua"), "geometry");
        assert_eq!(module_name("lib/util.lua"), "lib.util");
    }

    #[test]
    fn inherited_fields_walk_the_parent_chain() {
        let m = module(
            "---@class Base\n---@field id integer\nlocal Base = {}\n\
             \n\
             ---@class Mid: Base\n---@field label string\nlocal Mid = {}\n\
             \n\
             ---@class Leaf: Mid\n---@field own boolean\nlocal Leaf = {}\n",
        );
        let model = DocModel {
            package: "p".to_string(),
            modules: vec![m],
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

    #[test]
    fn implementors_collects_children_by_parent_name() {
        let m = module(
            "---@class Shape\nlocal Shape = {}\n\
             \n\
             ---@class Circle: Shape\nlocal Circle = {}\n\
             \n\
             ---@class Rect: Shape\nlocal Rect = {}\n\
             \n\
             ---@class Lonely\nlocal Lonely = {}\n",
        );
        let model = DocModel {
            package: "p".to_string(),
            modules: vec![m],
        };
        let map = implementors(&model);
        assert_eq!(
            map.get("Shape"),
            Some(&vec!["Circle".to_string(), "Rect".to_string()])
        );
        assert!(!map.contains_key("Lonely"));
        assert!(!map.contains_key("Circle"));
    }

    #[test]
    fn is_interface_true_only_for_all_function_typed_fields() {
        let m = module(
            "---@class Shape\n---@field area fun(self): number\n---@field perimeter fun(self): number\nlocal Shape = {}\n\
             \n\
             ---@class Base\n---@field id integer\nlocal Base = {}\n\
             \n\
             ---@class Empty\nlocal Empty = {}\n\
             \n\
             ---@class Mixed\n---@field id integer\n---@field area fun(self): number\nlocal Mixed = {}\n",
        );
        assert!(is_interface(&m.classes[0]));
        assert!(!is_interface(&m.classes[1]));
        assert!(!is_interface(&m.classes[2]));
        assert!(!is_interface(&m.classes[3]));
    }
}
