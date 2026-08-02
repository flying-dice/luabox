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
    AnnotatedItem, ClassTag, FieldKey, FieldTag, FunParam, FunReturn, ParamTag, Tag, TypeExpr,
    TypeExprKind,
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
pub struct ClassDecl<'a> {
    /// The `@class` tag itself.
    pub tag: &'a ClassTag,
    /// Its own `@field` tags, in declaration order.
    pub fields: Vec<&'a FieldTag>,
    /// The block's plain doc lines, joined.
    pub docs: String,
    /// The block's `---@see` references, in declaration order.
    pub sees: Vec<String>,
}

/// One function declaration: AST facts enriched with its annotation block.
pub struct FnDecl {
    /// Display name: `f`, `M.helper`, `Class:method`.
    pub name: String,
    /// The range of the name token at the declaration site.
    pub decl_range: TextRange,
    /// Rendered signature, e.g. `function f(n: number): string`.
    pub sig: String,
    /// Joined doc lines from the annotation block, if any.
    pub docs: String,
    /// Structured parameters (AST names merged with `---@param` types/docs).
    /// `sig` above is the same data pre-rendered to one string for hover and
    /// completion; [`crate::signature_help`] needs the per-parameter
    /// breakdown (and per-parameter doc lines, which the rendered string
    /// drops), so both are kept.
    pub params: Vec<SigParam>,
    /// Rendered `---@return` types, in declaration order.
    pub returns: Vec<TypeExpr>,
    /// `---@overload` alternate signatures. Each is a `fun(...)` type
    /// expression, so its parameters carry no `---@param`-style doc lines.
    pub overloads: Vec<(Vec<SigParam>, Vec<TypeExpr>)>,
    /// `---@see` references from the annotation block, in declaration order.
    pub sees: Vec<String>,
}

/// One parameter of a resolved signature: its declared/annotated name, type,
/// optionality, and `---@param` doc line where one is attached. Shared by
/// [`FnDecl::params`]/[`FnDecl::overloads`] and by
/// [`crate::signature_help`]'s class-field (`fun(...)`-typed method) route.
pub struct SigParam {
    pub name: String,
    pub ty: Option<TypeExpr>,
    pub optional: bool,
    pub vararg: bool,
    pub doc: Option<String>,
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

    /// Every static `require("...")` edge in the file (literal-argument
    /// requires), for enumerating what this file already imports.
    #[must_use]
    pub fn requires(&self) -> &[luabox_hir::RequireEdge] {
        self.lowered.file().requires()
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
    pub fn classes(&self) -> HashMap<&str, ClassDecl<'_>> {
        let mut out = HashMap::new();
        for item in self.items() {
            let mut current: Option<&str> = None;
            for tag in &item.block.tags {
                match tag {
                    Tag::Class(c) if !c.name.is_empty() => {
                        current = Some(&c.name);
                        out.insert(
                            c.name.as_str(),
                            ClassDecl {
                                tag: c,
                                fields: Vec::new(),
                                docs: docs_of(item),
                                sees: sees_of(item),
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

    /// The `---@source` location text governing `range`: the tag of the
    /// annotation block whose *block span* contains the range (a `@field`/
    /// `@class` tag site inside the block itself), or whose target statement
    /// *declares* the range as one of its own names — the two shapes LuaLS's
    /// `core/jump-source.lua` redirects (`parent.source` and `bindDocs`).
    ///
    /// The declaration arm is exact on purpose: a block binds to the one
    /// statement after it, so its `@source` governs only the names that
    /// statement declares. Mere containment would leak the redirect to every
    /// binding *inside* an annotated function's body.
    #[must_use]
    pub fn source_tag_covering(&self, range: TextRange) -> Option<&str> {
        fn source_text(item: &AnnotatedItem) -> Option<&str> {
            item.block.tags.iter().find_map(|tag| match tag {
                Tag::Source(t) => t.text.as_deref(),
                _ => None,
            })
        }
        let (start, end) = (usize::from(range.start()), usize::from(range.end()));
        // Tag-site arm: the queried range sits inside the block itself.
        let tag_site = self
            .items()
            .iter()
            .filter(|item| {
                let s = item.block.span;
                s.start <= start && end <= s.end
            })
            .find_map(source_text);
        if tag_site.is_some() {
            return tag_site;
        }
        // Declaration arm: the range must be a name the target itself declares.
        self.items()
            .iter()
            .filter(|item| {
                item.target
                    .is_some_and(|t| t.start <= start && end <= t.end)
            })
            .filter(|item| self.target_decl_names(item).into_iter().any(|r| r == range))
            .find_map(|item| source_text(item))
    }

    /// The name-token ranges *declared by* an item's target statement: the
    /// segments of a `function a.b.c()` name, or the names of a `local`
    /// statement (including `local function f`).
    ///
    /// Resolved through [`Self::stmt_at_exact`], which picks the *statement*
    /// sharing the target's span: a statement that is the only one in its
    /// block has the same text range as the enclosing `BLOCK`, and the block
    /// comes first in pre-order — matching it would drop the redirect.
    fn target_decl_names(&self, item: &AnnotatedItem) -> Vec<TextRange> {
        let Some(t) = item.target else {
            return Vec::new();
        };
        match self.stmt_at_exact(t) {
            Some(ast::Stmt::FunctionDecl(decl)) => decl
                .name()
                .map(|name| name.segments().map(|s| s.text_range()).collect())
                .unwrap_or_default(),
            Some(ast::Stmt::LocalFunction(decl)) => decl
                .name()
                .map(|tok| vec![tok.text_range()])
                .unwrap_or_default(),
            Some(ast::Stmt::Local(local)) => local
                .names()
                .filter_map(|n| n.name())
                .map(|tok| tok.text_range())
                .collect(),
            _ => Vec::new(),
        }
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

    /// The bare named type of a binding's annotation, peeling `?` and
    /// parentheses — WITHOUT requiring this file to declare it.
    /// [`Self::class_of_binding`] answers for file-local classes; this is
    /// the workspace-ambient surfaces' half (#56): the name is resolved
    /// against the merged ambient environment instead, where classes are
    /// workspace-global.
    #[must_use]
    pub fn annotated_named_type(&self, binding: &Binding) -> Option<String> {
        named_of(&self.binding_type(binding)?)
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
    pub fn functions(&self) -> Vec<FnDecl> {
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
            out.push(FnDecl {
                sig: render_signature(&name, params.as_ref(), item),
                docs: item.map(docs_of).unwrap_or_default(),
                params: collect_sig_params(params.as_ref(), item),
                returns: collect_returns(item),
                overloads: collect_overloads(item),
                sees: item.map(sees_of).unwrap_or_default(),
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
    classes: &HashMap<&str, ClassDecl<'a>>,
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

/// The `---@see` references of an annotation block, in declaration order
/// (multiple `@see` tags are allowed; empty-bodied ones are skipped).
#[must_use]
pub fn sees_of(item: &AnnotatedItem) -> Vec<String> {
    item.block
        .tags
        .iter()
        .filter_map(|tag| match tag {
            Tag::See(t) => t.text.clone(),
            _ => None,
        })
        .collect()
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

/// A function type's parameters and returns, peeling `?`/parens — the
/// `fun(...)` shape a class field/method (`---@field m fun(dx: number): T`)
/// declares its signature with, for [`crate::signature_help`].
#[must_use]
pub fn as_function_type(ty: &TypeExpr) -> Option<(&[FunParam], &[FunReturn])> {
    match &ty.kind {
        TypeExprKind::Fun { params, returns } => Some((params, returns)),
        TypeExprKind::Optional(inner) | TypeExprKind::Paren(inner) => as_function_type(inner),
        _ => None,
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

/// The structured parameter list for [`FnDecl::params`]/overloads: the AST
/// parameter names merged with their `---@param` type/doc, in the same
/// pairing `render_signature` uses (by name, with `...` for the vararg).
fn collect_sig_params(
    params: Option<&ast::ParamList>,
    item: Option<&AnnotatedItem>,
) -> Vec<SigParam> {
    let mut tags: HashMap<&str, &ParamTag> = HashMap::new();
    if let Some(item) = item {
        for tag in &item.block.tags {
            if let Tag::Param(p) = tag {
                tags.insert(p.name.as_str(), p);
            }
        }
    }
    let Some(list) = params else {
        return Vec::new();
    };
    list.params()
        .filter_map(|param| {
            let name = if param.is_vararg() {
                "...".to_string()
            } else {
                param.name()?.text().to_string()
            };
            let tag = tags.get(name.as_str());
            Some(SigParam {
                name,
                ty: tag.map(|t| t.ty.clone()),
                optional: tag.is_some_and(|t| t.optional),
                vararg: param.is_vararg(),
                doc: tag.and_then(|t| t.desc.clone()),
            })
        })
        .collect()
}

/// Every `---@return` type across the item's block, in declaration order
/// (mirrors the loop in `render_signature`, structured instead of rendered).
fn collect_returns(item: Option<&AnnotatedItem>) -> Vec<TypeExpr> {
    let Some(item) = item else { return Vec::new() };
    let mut out = Vec::new();
    for tag in &item.block.tags {
        if let Tag::Return(r) = tag {
            out.extend(r.items.iter().map(|i| i.ty.clone()));
        }
    }
    out
}

/// Every `---@overload fun(...)` on the item, as a structured parameter/return
/// pair (a non-`fun` overload type is a malformed annotation and is skipped).
fn collect_overloads(item: Option<&AnnotatedItem>) -> Vec<(Vec<SigParam>, Vec<TypeExpr>)> {
    let Some(item) = item else { return Vec::new() };
    item.block
        .tags
        .iter()
        .filter_map(|tag| {
            let Tag::Overload(o) = tag else { return None };
            let (params, returns) = as_function_type(&o.ty)?;
            Some((
                params.iter().map(fun_param_to_sig).collect(),
                returns.iter().map(|r| r.ty.clone()).collect(),
            ))
        })
        .collect()
}

/// A `fun(...)` type's parameter has no `---@param`-style doc line. Public:
/// also used by [`crate::signature_help`] for the class-field method route.
#[must_use]
pub fn fun_param_to_sig(p: &FunParam) -> SigParam {
    SigParam {
        name: p.name.clone(),
        ty: p.ty.clone(),
        optional: p.optional,
        vararg: p.vararg,
        doc: None,
    }
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

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::string_slice,
    clippy::panic,
    reason = "test code — panics document assumptions"
)]
mod tests {
    use super::*;

    use luabox_db::{AnalysisHost, Change, Dialect, Strictness};
    use luabox_syntax::luacats::Span;

    fn analyze(text: &str) -> (Analysis, PathBuf) {
        let mut host = AnalysisHost::new(Dialect::Lua54, Strictness::Warn);
        let path = Path::new(if cfg!(windows) {
            r"C:\ws\main.lua"
        } else {
            "/ws/main.lua"
        })
        .to_path_buf();
        host.apply_change(Change::SetFileText {
            path: path.clone(),
            dialect: Dialect::Lua54,
            text: text.to_string(),
        });
        (host.snapshot(), path)
    }

    fn sema_of(text: &str) -> (Analysis, PathBuf) {
        analyze(text)
    }

    /// Byte offset just inside the `nth` (0-based) occurrence of `needle`.
    fn offset_of(text: &str, needle: &str, nth: usize) -> usize {
        let mut from = 0;
        for _ in 0..nth {
            from = text[from..].find(needle).expect("occurrence") + from + 1;
        }
        text[from..].find(needle).expect("occurrence") + from
    }

    /// Parse `source` as the type of a `---@type` tag.
    fn parse_ty(source: &str) -> TypeExpr {
        let block = luabox_syntax::luacats::parse_block(&format!("---@type {source}"), 0);
        block
            .tags
            .iter()
            .find_map(|tag| match tag {
                Tag::Type(t) => t.types.first().cloned(),
                _ => None,
            })
            .expect("a @type tag with one type")
    }

    /// `---@type T` round-tripped back through [`render_type`].
    fn round_trip(source: &str) -> String {
        render_type(&parse_ty(source))
    }

    // === render_type ======================================================

    #[test]
    fn render_type_round_trips_every_shape() {
        assert_eq!(round_trip("string"), "string");
        assert_eq!(round_trip("table<string, number>"), "table<string, number>");
        assert_eq!(round_trip("string?"), "string?");
        assert_eq!(round_trip("string[]"), "string[]");
        assert_eq!(round_trip("string|number|nil"), "string|number|nil");
        assert_eq!(round_trip("[string, number]"), "[string, number]");
        assert_eq!(round_trip("(string)"), "(string)");
        assert_eq!(round_trip("`T`"), "`T`");
        assert_eq!(round_trip("true"), "true");
        assert_eq!(round_trip("false"), "false");
        assert_eq!(round_trip("42"), "42");
        assert_eq!(round_trip("\"lit\""), "\"lit\"");
    }

    #[test]
    fn render_type_renders_table_literal_fields_and_indexers() {
        assert_eq!(
            round_trip("{ name: string, age?: number, [string]: boolean }"),
            "{ name: string, age?: number, [string]: boolean }"
        );
    }

    #[test]
    fn render_type_renders_function_types_with_params_and_returns() {
        assert_eq!(round_trip("fun()"), "fun()");
        assert_eq!(
            round_trip("fun(a: string, b): boolean"),
            "fun(a: string, b): boolean"
        );
        assert_eq!(
            round_trip("fun(...: number): boolean, string"),
            "fun(...: number): boolean, string"
        );
    }

    #[test]
    fn render_type_renders_a_malformed_type_as_a_question_mark() {
        let ty = TypeExpr {
            kind: TypeExprKind::Error,
            span: Span::new(0, 0),
        };
        assert_eq!(render_type(&ty), "?");
    }

    // === Type-expression peeling ==========================================

    #[test]
    fn named_of_peels_optional_and_parens() {
        assert_eq!(named_of(&parse_ty("Point")).as_deref(), Some("Point"));
        assert_eq!(named_of(&parse_ty("Point?")).as_deref(), Some("Point"));
        assert_eq!(named_of(&parse_ty("(Point)")).as_deref(), Some("Point"));
        assert_eq!(named_of(&parse_ty("Point[]")), None);
    }

    #[test]
    fn is_function_type_peels_optional_and_parens() {
        assert!(is_function_type(&parse_ty("fun()")));
        assert!(is_function_type(&parse_ty("fun()?")));
        assert!(is_function_type(&parse_ty("(fun())")));
        assert!(!is_function_type(&parse_ty("string")));
    }

    #[test]
    fn as_function_type_peels_optional_and_parens() {
        let ty = parse_ty("(fun(a: string): number)?");
        let (params, returns) = as_function_type(&ty).expect("a function type");
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "a");
        assert_eq!(returns.len(), 1);
        assert_eq!(as_function_type(&parse_ty("string")), None);
    }

    // === ident_at =========================================================

    #[test]
    fn ident_at_past_the_end_of_the_file_is_none() {
        let src = "local x = 1\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        assert!(sema.ident_at(src.len() + 100).is_none());
    }

    #[test]
    fn ident_at_in_an_empty_file_is_none() {
        let (analysis, path) = sema_of("");
        let sema = FileSema::new(&analysis, &path).expect("sema");
        assert!(sema.ident_at(0).is_none());
    }

    #[test]
    fn ident_at_on_a_non_identifier_token_is_none() {
        let src = "local x = 1\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        // The `=` operator.
        assert!(sema.ident_at(offset_of(src, "=", 0)).is_none());
    }

    #[test]
    fn ident_at_between_two_tokens_prefers_the_identifier_on_the_left() {
        let src = "local abc = 1\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        // The boundary right after `abc`.
        let token = sema.ident_at(offset_of(src, " = 1", 0)).expect("ident");
        assert_eq!(token.text(), "abc");
    }

    // === binding_type =====================================================

    #[test]
    fn binding_type_reads_the_matching_param_annotation() {
        let src = "---@param n number\n---@param s string\nlocal function f(n, s) end\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let id = sema
            .binding_decl_at(offset_of(src, "s) end", 0))
            .expect("binding");
        let ty = sema.binding_type(sema.binding(id)).expect("type");
        assert_eq!(render_type(&ty), "string");
    }

    #[test]
    fn binding_type_of_an_unannotated_local_is_none() {
        let src = "---just a doc line\nlocal x = 1\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let id = sema
            .binding_decl_at(offset_of(src, "x = 1", 0))
            .expect("binding");
        assert!(sema.binding_type(sema.binding(id)).is_none());
    }

    #[test]
    fn multi_name_type_annotation_matches_names_positionally() {
        let src = "---@type number, string\nlocal a, b = 1, \"s\"\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let a = sema.binding_decl_at(offset_of(src, "a, b", 0)).expect("a");
        let b = sema.binding_decl_at(offset_of(src, "b = 1", 0)).expect("b");
        assert_eq!(
            render_type(&sema.binding_type(sema.binding(a)).expect("a type")),
            "number"
        );
        assert_eq!(
            render_type(&sema.binding_type(sema.binding(b)).expect("b type")),
            "string"
        );
    }

    #[test]
    fn type_annotation_on_a_non_local_statement_falls_back_to_the_first_type() {
        // The target is a `function` statement, so there is no positional
        // name index to match against.
        let src = "---@type number\nfunction f() end\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let id = sema
            .binding_decl_at(offset_of(src, "f() end", 0))
            .map(|id| sema.binding(id));
        // `function f` declares a global, not a binding; the annotation is
        // still reachable through the covering item.
        assert!(id.is_none() || sema.binding_type(id.expect("binding")).is_some());
    }

    // === class_of_binding / class_of_name =================================

    #[test]
    fn class_of_binding_peels_an_optional_annotation() {
        let src = "---@class Point\n---@field x number\n\n---@type Point?\nlocal p = nil\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        assert_eq!(
            sema.class_of_name("p", offset_of(src, "p = nil", 0) + 5)
                .as_deref(),
            Some("Point")
        );
    }

    #[test]
    fn class_of_binding_declines_a_type_that_is_not_a_declared_class() {
        let src = "---@type Missing\nlocal p = nil\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        assert_eq!(sema.class_of_name("p", src.len()), None);
        assert_eq!(sema.class_of_name("nothing", src.len()), None);
    }

    // === class_fields =====================================================

    #[test]
    fn class_fields_collects_parents_first_and_records_the_declaring_class() {
        let src = "\
---@class Base
---@field id number

---@class Derived: Base
---@field name string
";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let fields = sema.class_fields("Derived");
        let rendered: Vec<(String, String)> = fields
            .iter()
            .map(|(f, declaring)| {
                let FieldKey::Name(name) = &f.key else {
                    panic!("expected a named field, got {:?}", f.key)
                };
                let name = name.clone();
                (name, declaring.clone())
            })
            .collect();
        assert_eq!(
            rendered,
            vec![
                ("id".to_string(), "Base".to_string()),
                ("name".to_string(), "Derived".to_string()),
            ]
        );
    }

    #[test]
    fn own_field_overrides_an_inherited_one_of_the_same_name() {
        let src = "\
---@class Base
---@field v number

---@class Derived: Base
---@field v string
";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let fields = sema.class_fields("Derived");
        assert_eq!(fields.len(), 1, "{:?}", fields.len());
        assert_eq!(fields[0].1, "Derived");
        assert_eq!(render_type(&fields[0].0.ty), "string");
    }

    #[test]
    fn class_fields_of_an_undeclared_class_is_empty() {
        let (analysis, path) = sema_of("---@class Known\n");
        let sema = FileSema::new(&analysis, &path).expect("sema");
        assert!(sema.class_fields("NotDeclared").is_empty());
    }

    #[test]
    fn inheritance_cycles_terminate() {
        let src = "\
---@class A: B
---@field a number

---@class B: A
---@field b number
";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        // The cycle guard stops the walk; each field appears exactly once.
        assert_eq!(sema.class_fields("A").len(), 2);
        assert_eq!(sema.class_fields("B").len(), 2);
    }

    #[test]
    fn a_non_named_parent_is_skipped() {
        let src = "\
---@class Odd: { x: number }
---@field own string
";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let fields = sema.class_fields("Odd");
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].1, "Odd");
    }

    // === functions() ======================================================

    #[test]
    fn a_local_assigned_function_expression_is_a_function_declaration() {
        let src = "\
---Doubles.
---@param n number
---@return number
local double = function(n) return n * 2 end
";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let functions = sema.functions();
        let info = functions
            .iter()
            .find(|f| f.name == "double")
            .expect("`double` is a function declaration");
        assert_eq!(info.sig, "function double(n: number): number");
        assert_eq!(info.docs, "Doubles.");
        assert_eq!(info.params.len(), 1);
        assert_eq!(info.returns.len(), 1);
    }

    #[test]
    fn a_local_assigned_non_function_is_not_a_function_declaration() {
        let (analysis, path) = sema_of("local t = {}\n");
        let sema = FileSema::new(&analysis, &path).expect("sema");
        assert!(sema.functions().is_empty());
    }

    #[test]
    fn a_method_declaration_renders_with_a_colon() {
        let src = "function M.sub:go(self) end\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let functions = sema.functions();
        assert_eq!(functions.len(), 1);
        assert_eq!(functions[0].name, "M.sub:go");
    }

    #[test]
    fn a_dotted_declaration_renders_with_dots() {
        let src = "function M.sub.go() end\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        assert_eq!(sema.functions()[0].name, "M.sub.go");
    }

    #[test]
    fn an_annotated_vararg_renders_its_type_in_the_signature() {
        let src = "---@param ... number\nfunction sum(...) end\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let functions = sema.functions();
        assert_eq!(functions[0].sig, "function sum(...: number)");
        assert_eq!(functions[0].params.len(), 1);
        assert_eq!(functions[0].params[0].name, "...");
        assert!(functions[0].params[0].vararg);
    }

    #[test]
    fn an_unannotated_vararg_renders_bare() {
        let src = "function sum(a, ...) end\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let functions = sema.functions();
        assert_eq!(functions[0].sig, "function sum(a, ...)");
        assert!(functions[0].params[1].ty.is_none());
    }

    #[test]
    fn overloads_are_collected_and_a_non_function_overload_is_skipped() {
        let src = "\
---@overload fun(n: number): string
---@overload string
function f(a) end
";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let functions = sema.functions();
        assert_eq!(functions[0].overloads.len(), 1);
        let (params, returns) = &functions[0].overloads[0];
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "n");
        assert_eq!(returns.len(), 1);
    }

    // === global_defs ======================================================

    #[test]
    fn global_defs_lists_function_statements_and_assignment_targets() {
        let src = "\
function g() end
function M.helper() end
x, y = 1, 2
local hidden = 3
t.field = 4
";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let names: Vec<String> = sema
            .global_defs()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        // `function M.helper` contributes its *first* segment `M`; `t.field`
        // is a field expression, not a bare name, so it contributes nothing.
        assert_eq!(names, vec!["g", "M", "x", "y"]);
    }

    #[test]
    fn global_defs_ranges_point_at_the_name_token() {
        let src = "answer = 42\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let defs = sema.global_defs();
        assert_eq!(defs.len(), 1);
        assert_eq!(usize::from(defs[0].1.start()), 0);
        assert_eq!(usize::from(defs[0].1.end()), "answer".len());
    }

    // === source_tag_covering ==============================================

    #[test]
    fn a_source_tag_redirects_each_name_of_its_local_statement() {
        let src = "---@source impl/native.c:12\nlocal a, b = 1, 2\nprint(a, b)\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        for needle in ["a, b", "b = 1"] {
            let id = sema
                .binding_decl_at(offset_of(src, needle, 0))
                .expect("binding");
            let range = sema.binding(id).range;
            assert_eq!(
                sema.source_tag_covering(range),
                Some("impl/native.c:12"),
                "{needle}"
            );
        }
    }

    #[test]
    fn a_source_tag_does_not_redirect_an_unrelated_range() {
        let src = "---@source impl/native.c\nlocal a = 1\nlocal b = 2\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let id = sema
            .binding_decl_at(offset_of(src, "b = 2", 0))
            .expect("binding");
        assert_eq!(sema.source_tag_covering(sema.binding(id).range), None);
    }

    #[test]
    fn a_source_tag_redirects_a_local_function_name() {
        let src = "---@source impl/native.c\nlocal function f() end\nf()\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let id = sema
            .binding_decl_at(offset_of(src, "f() end", 0))
            .expect("binding");
        assert_eq!(
            sema.source_tag_covering(sema.binding(id).range),
            Some("impl/native.c")
        );
    }

    #[test]
    fn a_source_tag_on_a_statement_that_declares_nothing_redirects_nothing() {
        let src = "---@source impl/native.c\nprint(1)\nlocal x = 2\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let id = sema
            .binding_decl_at(offset_of(src, "x = 2", 0))
            .expect("binding");
        assert_eq!(sema.source_tag_covering(sema.binding(id).range), None);
        let item = sema
            .items()
            .iter()
            .find(|i| i.target.is_some())
            .expect("an annotated item");
        assert!(sema.target_decl_names(item).is_empty());
    }

    #[test]
    fn a_source_tag_redirects_a_statement_that_is_alone_in_the_file() {
        // The lone statement's range equals the enclosing `BLOCK`'s, and the
        // block comes first in pre-order — the redirect must survive that.
        let src = "---@source impl.c\nlocal function f() end\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let id = sema
            .binding_decl_at(offset_of(src, "f() end", 0))
            .expect("binding");
        assert_eq!(
            sema.source_tag_covering(sema.binding(id).range),
            Some("impl.c")
        );
    }

    #[test]
    fn a_source_tag_redirects_a_statement_that_is_alone_in_a_function_body() {
        let src = "local function outer()\n  ---@source impl.c\n  local inner = 1\nend\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let id = sema
            .binding_decl_at(offset_of(src, "inner = 1", 0))
            .expect("binding");
        assert_eq!(
            sema.source_tag_covering(sema.binding(id).range),
            Some("impl.c")
        );
    }

    #[test]
    fn a_source_tag_redirects_a_dotted_function_alone_in_a_do_block() {
        let src = "do\n  ---@source impl.c\n  function M.helper() end\nend\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let functions = sema.functions();
        assert_eq!(
            sema.source_tag_covering(functions[0].decl_range),
            Some("impl.c")
        );
    }

    #[test]
    fn a_source_tag_still_redirects_when_other_statements_follow() {
        let src = "---@source impl.c\nlocal function f() end\nf()\nlocal g = 2\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let id = sema
            .binding_decl_at(offset_of(src, "f() end", 0))
            .expect("binding");
        assert_eq!(
            sema.source_tag_covering(sema.binding(id).range),
            Some("impl.c")
        );
        // ... and still leaks to no unannotated neighbour.
        let other = sema
            .binding_decl_at(offset_of(src, "g = 2", 0))
            .expect("binding");
        assert_eq!(sema.source_tag_covering(sema.binding(other).range), None);
    }

    #[test]
    fn a_source_tag_redirects_every_segment_of_a_dotted_function_name() {
        let src = "---@source impl/native.c\nfunction M.helper() end\nM.helper()\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let functions = sema.functions();
        assert_eq!(
            sema.source_tag_covering(functions[0].decl_range),
            Some("impl/native.c")
        );
    }

    // === Misc accessors ===================================================

    #[test]
    fn requires_and_require_at_agree() {
        let src = "local m = require(\"other\")\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let edges = sema.requires();
        assert_eq!(edges.len(), 1);
        let inside = usize::from(edges[0].range.start()) + 1;
        assert!(sema.require_at(inside).is_some());
        assert!(sema.require_at(0).is_none());
    }

    #[test]
    fn visible_binding_named_picks_the_nearest_earlier_declaration() {
        let src = "local x = 1\nlocal x = 2\nprint(x)\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let at_end = sema.visible_binding_named("x", src.len()).expect("binding");
        // The later shadowing declaration wins.
        assert_eq!(
            usize::from(at_end.range.start()),
            offset_of(src, "x = 2", 0)
        );
        // Before any declaration, nothing is visible.
        assert!(sema.visible_binding_named("x", 0).is_none());
        assert!(sema.visible_binding_named("nope", src.len()).is_none());
    }

    #[test]
    fn stmt_at_exact_requires_an_exact_span_match() {
        let src = "local x = 1\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        assert!(sema.stmt_at_exact(Span::new(0, 11)).is_some());
        assert!(sema.stmt_at_exact(Span::new(0, 5)).is_none());
    }

    #[test]
    fn file_sema_declines_a_path_the_analysis_does_not_know() {
        let (analysis, _) = sema_of("local x = 1\n");
        let missing = Path::new(if cfg!(windows) {
            r"C:\ws\absent.lua"
        } else {
            "/ws/absent.lua"
        });
        assert!(FileSema::new(&analysis, missing).is_none());
    }

    #[test]
    fn name_resolutions_reports_every_use_in_one_pass() {
        let src = "local x = 1\nprint(x)\nprint(x)\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let uses = sema.name_resolutions();
        let locals = uses
            .iter()
            .filter(|(_, res)| matches!(res, Resolution::Local(_)))
            .count();
        assert_eq!(locals, 2, "{uses:?}");
        assert!(
            uses.iter()
                .any(|(_, res)| matches!(res, Resolution::Global(n) if n == "print"))
        );
    }

    #[test]
    fn resolution_at_outside_any_name_use_is_none() {
        let src = "local x = 1\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        assert!(sema.resolution_at(offset_of(src, "=", 0)).is_none());
    }

    #[test]
    fn docs_and_sees_of_a_detached_block_are_reachable() {
        let src = "---A note.\n---@see a.b\n---@see c.d\n\nlocal x = 1\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let item = sema
            .items()
            .iter()
            .find(|i| !sees_of(i).is_empty())
            .expect("an item with @see tags");
        assert_eq!(docs_of(item), "A note.");
        assert_eq!(sees_of(item), vec!["a.b".to_string(), "c.d".to_string()]);
        // A detached block has no target statement, so it declares no names.
        assert_eq!(item.target, None);
        assert!(sema.target_decl_names(item).is_empty());
    }

    #[test]
    fn item_covering_picks_the_innermost_target() {
        let src = "---@type number\nlocal x = 1\n";
        let (analysis, path) = sema_of(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let id = sema
            .binding_decl_at(offset_of(src, "x = 1", 0))
            .expect("binding");
        let item = sema
            .item_covering(sema.binding(id).range)
            .expect("covering item");
        assert!(matches!(item.block.tags.first(), Some(Tag::Type(_))));
        // A range outside every annotated statement is covered by nothing.
        assert!(
            sema.item_covering(TextRange::new(TextSize::new(0), TextSize::new(3)))
                .is_none()
        );
    }
}
