//! Per-file semantic access assembled from an [`Analysis`] snapshot: the
//! shared substrate under hover, goto-definition, completion, and document
//! symbols.
//!
//! Everything here is *per file* (cross-file `require` resolution is a later
//! tranche, matching `luabox-types`' per-file environments):
//!
//! - identifiers are located in the lossless syntax tree,
//! - name resolution (local / upvalue / global) comes from the HIR lowering,
//! - types, docs, classes, and signatures come from the LuaCATS annotation
//!   harvest — the same producer the typechecker reads.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use luabox_db::{Analysis, Annotations, LoweredHandle};
use luabox_hir::{Binding, BindingId, Expr as HirExpr, HirId, Resolution};
use luabox_syntax::lua::ast::{self, AstNode};
use luabox_syntax::lua::{SyntaxKind, SyntaxNode, SyntaxToken};
use luabox_syntax::luacats::{
    AnnotatedItem, ClassTag, FieldKey, FieldTag, Tag, TypeExpr, TypeExprKind,
};
use rowan::{TextRange, TextSize, TokenAtOffset};

use crate::line_index::LineIndex;

/// Semantic view of one Lua file, built from an [`Analysis`] snapshot.
pub struct FileSema {
    /// The file's path (the key it is known under in the analysis).
    pub path: PathBuf,
    /// Line index over the file's current text.
    pub index: LineIndex,
    /// The lossless syntax root.
    pub root: SyntaxNode,
    annotations: Annotations,
    lowered: LoweredHandle,
}

/// A `---@class` declaration harvested from the file.
pub struct ClassInfo<'a> {
    /// The `@class` tag itself.
    pub tag: &'a ClassTag,
    /// Its own `@field` tags, in declaration order.
    pub fields: Vec<&'a FieldTag>,
    /// The block's plain doc lines, joined.
    pub docs: String,
}

/// One function declaration: AST facts enriched with its annotation block.
pub struct FnInfo {
    /// Display name: `f`, `M.helper`, `Class:method`.
    pub name: String,
    /// The range of the name token at the declaration site.
    pub decl_range: TextRange,
    /// Rendered signature, e.g. `function f(n: number): string`.
    pub sig: String,
    /// Joined doc lines from the annotation block, if any.
    pub docs: String,
}

impl FileSema {
    /// Build the view for `path`, or `None` if the analysis does not know it.
    #[must_use]
    pub fn new(analysis: &Analysis, path: &Path) -> Option<Self> {
        let text = analysis.file_text(path)?;
        let root = analysis.syntax(path)?;
        let annotations = analysis.annotations(path)?;
        let lowered = analysis.lower(path)?;
        Some(Self {
            path: path.to_path_buf(),
            index: LineIndex::new(text),
            root,
            annotations,
            lowered,
        })
    }

    /// The harvested annotation items.
    #[must_use]
    pub fn items(&self) -> &[AnnotatedItem] {
        self.annotations.items()
    }

    /// The identifier token at (or immediately left of) `offset`.
    #[must_use]
    pub fn ident_at(&self, offset: usize) -> Option<SyntaxToken> {
        let offset = TextSize::new(u32::try_from(offset).ok()?);
        if offset > self.root.text_range().end() {
            return None;
        }
        let pick = |t: SyntaxToken| (t.kind() == SyntaxKind::IDENT).then_some(t);
        match self.root.token_at_offset(offset) {
            TokenAtOffset::None => None,
            TokenAtOffset::Single(t) => pick(t),
            TokenAtOffset::Between(l, r) => pick(l).or_else(|| pick(r)),
        }
    }

    // === Name resolution (HIR) ===========================================

    /// The resolution of the name *use* whose range contains `offset`.
    #[must_use]
    pub fn resolution_at(&self, offset: usize) -> Option<Resolution> {
        let file = self.lowered.file();
        for (body_id, body) in file.bodies() {
            for (expr_id, expr) in body.exprs() {
                if !matches!(expr, HirExpr::Name(_)) {
                    continue;
                }
                let id = HirId::expr(body_id, expr_id);
                let Some(range) = file.source_map().range(id) else {
                    continue;
                };
                if contains(range, offset) {
                    return file.resolution(id).cloned();
                }
            }
        }
        None
    }

    /// Every resolved name *use* in the file as `(range, resolution)` pairs,
    /// in one pass over the HIR (semantic tokens classify all names at once,
    /// so the per-offset [`Self::resolution_at`] scan would be quadratic).
    #[must_use]
    pub fn name_resolutions(&self) -> Vec<(TextRange, Resolution)> {
        let file = self.lowered.file();
        let mut out = Vec::new();
        for (body_id, body) in file.bodies() {
            for (expr_id, expr) in body.exprs() {
                if !matches!(expr, HirExpr::Name(_)) {
                    continue;
                }
                let id = HirId::expr(body_id, expr_id);
                if let (Some(range), Some(res)) = (file.source_map().range(id), file.resolution(id))
                {
                    out.push((range, res.clone()));
                }
            }
        }
        out
    }

    /// The binding *declared* at `offset` (the cursor is on the definition).
    #[must_use]
    pub fn binding_decl_at(&self, offset: usize) -> Option<BindingId> {
        self.lowered
            .file()
            .bindings()
            .find(|(_, b)| contains(b.range, offset))
            .map(|(id, _)| id)
    }

    /// The binding data for a handle.
    #[must_use]
    pub fn binding(&self, id: BindingId) -> &Binding {
        self.lowered.file().binding(id)
    }

    /// All bindings declared before `offset`, for scope completion.
    ///
    /// Approximation for tranche 1: declaration order stands in for true
    /// lexical scoping (a sibling-scope local may leak into the list).
    pub fn bindings_before(&self, offset: usize) -> impl Iterator<Item = &Binding> {
        self.lowered
            .file()
            .bindings()
            .map(|(_, b)| b)
            .filter(move |b| usize::from(b.range.end()) <= offset)
    }

    /// The nearest binding named `name` declared before `offset` (same
    /// approximation as [`Self::bindings_before`]).
    #[must_use]
    pub fn visible_binding_named(&self, name: &str, offset: usize) -> Option<&Binding> {
        self.bindings_before(offset)
            .filter(|b| b.name == name)
            .max_by_key(|b| b.range.start())
    }

    /// The static `require("...")` edge whose call range contains `offset`.
    #[must_use]
    pub fn require_at(&self, offset: usize) -> Option<&luabox_hir::RequireEdge> {
        self.lowered
            .file()
            .requires()
            .iter()
            .find(|edge| contains(edge.range, offset))
    }

    // === Annotations ======================================================

    /// Every `---@class` in the file, by name.
    #[must_use]
    pub fn classes(&self) -> HashMap<&str, ClassInfo<'_>> {
        let mut out = HashMap::new();
        for item in self.items() {
            let mut current: Option<&str> = None;
            for tag in &item.block.tags {
                match tag {
                    Tag::Class(c) if !c.name.is_empty() => {
                        current = Some(&c.name);
                        out.insert(
                            c.name.as_str(),
                            ClassInfo {
                                tag: c,
                                fields: Vec::new(),
                                docs: docs_of(item),
                            },
                        );
                    }
                    Tag::Field(f) => {
                        if let Some(info) = current.and_then(|n| out.get_mut(n)) {
                            info.fields.push(f);
                        }
                    }
                    _ => {}
                }
            }
        }
        out
    }

    /// The named fields of `class`, parents first (own fields override),
    /// with a cycle guard. Returns `(field, declaring class)` pairs.
    #[must_use]
    pub fn class_fields(&self, class: &str) -> Vec<(&FieldTag, String)> {
        let classes = self.classes();
        let mut seen = HashSet::new();
        let mut by_name: Vec<(String, (&FieldTag, String))> = Vec::new();
        collect_fields(&classes, class, &mut seen, &mut by_name);
        let mut merged: Vec<(&FieldTag, String)> = Vec::new();
        let mut names = HashSet::new();
        // Later entries (own fields) override earlier (inherited) ones.
        for (name, entry) in by_name.into_iter().rev() {
            if names.insert(name) {
                merged.push(entry);
            }
        }
        merged.reverse();
        merged
    }

    /// The annotation item whose target statement contains `range`
    /// (innermost when several nest).
    #[must_use]
    pub fn item_covering(&self, range: TextRange) -> Option<&AnnotatedItem> {
        self.items()
            .iter()
            .filter(|item| {
                item.target.is_some_and(|t| {
                    t.start <= usize::from(range.start()) && usize::from(range.end()) <= t.end
                })
            })
            .min_by_key(|item| item.target.map_or(usize::MAX, |t| t.end - t.start))
    }

    /// The annotated type of a binding: an `---@type` on its `local`
    /// statement (matched positionally to the declared name), or the
    /// `---@param` of the enclosing annotated function.
    #[must_use]
    pub fn binding_type(&self, binding: &Binding) -> Option<TypeExpr> {
        let item = self.item_covering(binding.range)?;
        for tag in &item.block.tags {
            match tag {
                Tag::Type(t) => {
                    let idx = self.local_name_index(item, binding).unwrap_or(0);
                    return t.types.get(idx).or_else(|| t.types.first()).cloned();
                }
                Tag::Param(p) if p.name == binding.name => {
                    return Some(p.ty.clone());
                }
                _ => {}
            }
        }
        None
    }

    /// The class name a binding's annotated type resolves to, if the type is
    /// a declared `---@class` (peeling `?` and parentheses).
    #[must_use]
    pub fn class_of_binding(&self, binding: &Binding) -> Option<String> {
        let ty = self.binding_type(binding)?;
        let name = named_of(&ty)?;
        self.classes().contains_key(name.as_str()).then_some(name)
    }

    /// [`Self::class_of_binding`] for the nearest binding named `name`
    /// visible at `offset`.
    #[must_use]
    pub fn class_of_name(&self, name: &str, offset: usize) -> Option<String> {
        let binding = self.visible_binding_named(name, offset)?;
        self.class_of_binding(binding)
    }

    /// Which of the target `local` statement's names this binding is
    /// (positional index), for multi-name `---@type A, B` matching.
    fn local_name_index(&self, item: &AnnotatedItem, binding: &Binding) -> Option<usize> {
        let target = item.target?;
        let stmt = self.stmt_at_exact(target)?;
        let ast::Stmt::Local(local) = stmt else {
            return None;
        };
        local.names().position(|n| {
            n.name()
                .is_some_and(|token| token.text_range() == binding.range)
        })
    }

    /// The statement whose range is exactly `span`.
    #[must_use]
    pub fn stmt_at_exact(&self, span: luabox_syntax::luacats::Span) -> Option<ast::Stmt> {
        self.root
            .descendants()
            .filter(|node| {
                let r = node.text_range();
                usize::from(r.start()) == span.start && usize::from(r.end()) == span.end
            })
            .find_map(ast::Stmt::cast)
    }

    // === Functions ========================================================

    /// Every function declaration in the file (`function f`, `function M.g`,
    /// `function C:m`, `local function h`, `local f = function`), enriched
    /// with its annotation block when one is attached.
    #[must_use]
    pub fn functions(&self) -> Vec<FnInfo> {
        let mut out = Vec::new();
        for node in self.root.descendants() {
            let (name, decl_token, params, stmt_range) = match node.kind() {
                SyntaxKind::FUNCTION_DECL_STMT => {
                    let Some(decl) = ast::FunctionDeclStmt::cast(node.clone()) else {
                        continue;
                    };
                    let Some((name, token)) = function_decl_name(&decl) else {
                        continue;
                    };
                    (name, token, decl.param_list(), node.text_range())
                }
                SyntaxKind::LOCAL_FUNCTION_STMT => {
                    let Some(decl) = ast::LocalFunctionStmt::cast(node.clone()) else {
                        continue;
                    };
                    let Some(token) = decl.name() else { continue };
                    (
                        token.text().to_string(),
                        token,
                        decl.param_list(),
                        node.text_range(),
                    )
                }
                SyntaxKind::LOCAL_STMT => {
                    let Some(local) = ast::LocalStmt::cast(node.clone()) else {
                        continue;
                    };
                    let Some(ast::Expr::Function(func)) =
                        local.values().and_then(|v| v.exprs().next())
                    else {
                        continue;
                    };
                    let Some(token) = local.names().next().and_then(|n| n.name()) else {
                        continue;
                    };
                    (
                        token.text().to_string(),
                        token,
                        func.param_list(),
                        node.text_range(),
                    )
                }
                _ => continue,
            };
            let item = self.item_covering_exact(stmt_range);
            out.push(FnInfo {
                sig: render_signature(&name, params.as_ref(), item),
                docs: item.map(docs_of).unwrap_or_default(),
                name,
                decl_range: decl_token.text_range(),
            });
        }
        out
    }

    /// The annotation item targeting exactly this statement range.
    fn item_covering_exact(&self, range: TextRange) -> Option<&AnnotatedItem> {
        self.items().iter().find(|item| {
            item.target.is_some_and(|t| {
                t.start == usize::from(range.start()) && t.end == usize::from(range.end())
            })
        })
    }

    /// Global definition sites in this file: non-local `function` statements'
    /// first name segment and top-level `name = ...` assignment targets.
    #[must_use]
    pub fn global_defs(&self) -> Vec<(String, TextRange)> {
        let mut out = Vec::new();
        for node in self.root.descendants() {
            match node.kind() {
                SyntaxKind::FUNCTION_DECL_STMT => {
                    let Some(decl) = ast::FunctionDeclStmt::cast(node) else {
                        continue;
                    };
                    if let Some(first) = decl.name().and_then(|n| n.segments().next()) {
                        out.push((first.text().to_string(), first.text_range()));
                    }
                }
                SyntaxKind::ASSIGN_STMT => {
                    let Some(assign) = ast::AssignStmt::cast(node) else {
                        continue;
                    };
                    let Some(targets) = assign.targets() else {
                        continue;
                    };
                    for target in targets.exprs() {
                        if let ast::Expr::Name(name) = target
                            && let Some(token) = name.name()
                        {
                            out.push((token.text().to_string(), token.text_range()));
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }
}

/// Whether `range` contains `offset` (half-open, but a cursor at the very end
/// of a token still counts — hover after the last character should hit).
fn contains(range: TextRange, offset: usize) -> bool {
    let (start, end) = (usize::from(range.start()), usize::from(range.end()));
    start <= offset && offset <= end && !(offset == end && start == end)
}

fn collect_fields<'a>(
    classes: &HashMap<&str, ClassInfo<'a>>,
    name: &str,
    seen: &mut HashSet<String>,
    out: &mut Vec<(String, (&'a FieldTag, String))>,
) {
    if !seen.insert(name.to_string()) {
        return;
    }
    let Some(info) = classes.get(name) else {
        return;
    };
    for parent in &info.tag.parents {
        if let Some(parent_name) = match &parent.kind {
            TypeExprKind::Named { name, .. } => Some(name.clone()),
            _ => None,
        } {
            collect_fields(classes, &parent_name, seen, out);
        }
    }
    for field in &info.fields {
        if let FieldKey::Name(field_name) = &field.key {
            out.push((field_name.clone(), (field, name.to_string())));
        }
    }
}

/// The joined plain doc lines of an annotation block.
#[must_use]
pub fn docs_of(item: &AnnotatedItem) -> String {
    item.block
        .docs
        .iter()
        .map(|d| d.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The underlying named type of an annotated type, peeling `?`/parens.
#[must_use]
pub fn named_of(ty: &TypeExpr) -> Option<String> {
    match &ty.kind {
        TypeExprKind::Named { name, .. } => Some(name.clone()),
        TypeExprKind::Optional(inner) | TypeExprKind::Paren(inner) => named_of(inner),
        _ => None,
    }
}

/// Whether an annotated type is a function type.
#[must_use]
pub fn is_function_type(ty: &TypeExpr) -> bool {
    match &ty.kind {
        TypeExprKind::Fun { .. } => true,
        TypeExprKind::Optional(inner) | TypeExprKind::Paren(inner) => is_function_type(inner),
        _ => false,
    }
}

/// The display name and name token of a `function a.b:c` declaration.
fn function_decl_name(decl: &ast::FunctionDeclStmt) -> Option<(String, SyntaxToken)> {
    let name = decl.name()?;
    let segments: Vec<SyntaxToken> = name.segments().collect();
    let last = segments.last()?.clone();
    let rendered = if name.is_method() && segments.len() >= 2 {
        let base: Vec<&str> = segments[..segments.len() - 1]
            .iter()
            .map(SyntaxToken::text)
            .collect();
        format!("{}:{}", base.join("."), last.text())
    } else {
        segments
            .iter()
            .map(SyntaxToken::text)
            .collect::<Vec<_>>()
            .join(".")
    };
    Some((rendered, last))
}

/// Render `function name(params): returns` from the AST parameter list plus
/// the annotation block's `@param`/`@return` tags.
fn render_signature(
    name: &str,
    params: Option<&ast::ParamList>,
    item: Option<&AnnotatedItem>,
) -> String {
    let mut param_types: HashMap<&str, &TypeExpr> = HashMap::new();
    let mut returns: Vec<String> = Vec::new();
    if let Some(item) = item {
        for tag in &item.block.tags {
            match tag {
                Tag::Param(p) => {
                    param_types.insert(p.name.as_str(), &p.ty);
                }
                Tag::Return(r) => {
                    for ret in &r.items {
                        returns.push(render_type(&ret.ty));
                    }
                }
                _ => {}
            }
        }
    }
    let mut rendered_params: Vec<String> = Vec::new();
    if let Some(list) = params {
        for param in list.params() {
            if param.is_vararg() {
                match param_types.get("...") {
                    Some(ty) => rendered_params.push(format!("...: {}", render_type(ty))),
                    None => rendered_params.push("...".to_string()),
                }
            } else if let Some(token) = param.name() {
                match param_types.get(token.text()) {
                    Some(ty) => {
                        rendered_params.push(format!("{}: {}", token.text(), render_type(ty)));
                    }
                    None => rendered_params.push(token.text().to_string()),
                }
            }
        }
    }
    let mut sig = format!("function {name}({})", rendered_params.join(", "));
    if !returns.is_empty() {
        sig.push_str(": ");
        sig.push_str(&returns.join(", "));
    }
    sig
}

/// Render a LuaCATS type expression back to source-ish text.
#[must_use]
pub fn render_type(ty: &TypeExpr) -> String {
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
                    luabox_syntax::luacats::TableField::Named { name, optional, ty } => {
                        let q = if *optional { "?" } else { "" };
                        format!("{name}{q}: {}", render_type(ty))
                    }
                    luabox_syntax::luacats::TableField::Indexer { key, value } => {
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
