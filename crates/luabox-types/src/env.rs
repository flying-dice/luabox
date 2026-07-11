//! The per-file type environment: every declaration the annotations make.
//!
//! Built in two passes over [`luacats::harvest`] output: first collect the
//! *names* of classes/aliases/enums (so forward references resolve), then
//! lower every annotation body against them. Cross-file `require`
//! resolution is P1 — the environment is strictly per file for now.

use std::collections::{BTreeMap, HashMap, HashSet};

use luabox_syntax::lua::ast::{AstNode, Expr, LocalStmt, Stmt};
use luabox_syntax::lua::{self, SyntaxKind, SyntaxNode};
use luabox_syntax::luacats::{
    self, AliasTag, CastKind, FieldKey, ParamTag, ReturnTag, Tag, TypeExprKind,
};

use crate::lower::{Declared, Lowerer};
use crate::ty::{FieldTy, FunctionTy, ParamTy, TableTy, Ty};

/// A statement's byte range, used to key annotations to their target.
pub(crate) type Target = (usize, usize);

/// A declared `---@class`: parents plus *own* members (inherited members
/// are merged on demand by [`TypeEnv::class_shape`]).
#[derive(Debug, Default, Clone)]
pub(crate) struct ClassDef {
    pub parents: Vec<String>,
    pub fields: BTreeMap<String, FieldTy>,
    pub indexers: Vec<(Ty, Ty)>,
}

/// A declared `---@enum`: member name → value type, plus the union of all
/// member values (what the enum *type* accepts).
#[derive(Debug, Clone)]
pub(crate) struct EnumDef {
    pub members: BTreeMap<String, Ty>,
    pub value_union: Ty,
}

/// A lowered `---@cast var [+|-]T[, ...]` bound to the statement it
/// precedes: applied as a flow-state override when inference reaches that
/// statement.
#[derive(Debug, Clone)]
pub(crate) struct CastEntry {
    pub var: String,
    pub ops: Vec<(CastKind, Ty)>,
}

/// Everything the annotations of one file declare.
#[derive(Debug, Default)]
pub struct TypeEnv {
    classes: BTreeMap<String, ClassDef>,
    enums: BTreeMap<String, EnumDef>,
    /// Ambient `.luab` types with structural table bodies, by FQ name:
    /// sealed shapes `Ty::Named` references resolve to (SHAPES-V2.md).
    shape_structs: BTreeMap<String, TableTy>,
    /// Annotated functions by (dotted) name — `f`, `M.helper`.
    functions: BTreeMap<String, FunctionTy>,
    /// `---@type` annotations keyed by their target `local` statement.
    typed_locals: HashMap<Target, Vec<Ty>>,
    /// The source span of each `---@type` annotation, keyed by its target
    /// `local` statement — the anchor a deferred whole-carrier conformance
    /// error points at (SHAPES-V2.md).
    typed_local_spans: HashMap<Target, std::ops::Range<usize>>,
    /// Function signatures keyed by their target statement (for return
    /// checking inside the body).
    fn_sigs: HashMap<Target, FunctionTy>,
    /// `---@class` / `---@struct` names keyed by the statement they
    /// annotate (the carrier `local`). Inference uses this to associate a
    /// locally-constructed table with its declaration.
    declared_targets: HashMap<Target, String>,
    /// The `---@class` tag span keyed by the carrier statement it annotates —
    /// the anchor a `: Interface` conformance error (LB0300) points at
    /// (mirrors [`Self::typed_local_spans`] for the `---@type` path). #107.
    class_tag_spans: HashMap<Target, std::ops::Range<usize>>,
    /// The `---@class` tag span keyed by the class *name*, for classes
    /// declared in *this* file — the secondary "declared here" label a
    /// conformance error attaches to the parent it names (#107). Only
    /// same-file classes are recorded; ambient/defs parents have no in-file
    /// span to point at.
    class_decl_spans: BTreeMap<String, std::ops::Range<usize>>,
    /// Ambient / global values by name: stdlib module tables (`string`,
    /// `math`, ...) and scalar globals (`_VERSION`) declared by definition
    /// packages (`---@meta` `.d.lua`). Populated only for the ambient
    /// layer; per-file annotations shadow these.
    global_types: BTreeMap<String, Ty>,
    /// `---@cast` overrides keyed by the statement they precede.
    casts: HashMap<Target, Vec<CastEntry>>,
    /// Inline `--[[@as T]]` casts keyed by the byte offset of the end of
    /// the expression they follow (the anchor).
    as_casts: HashMap<usize, Ty>,
    /// References to undeclared type names (LB0305): `(name, span)`.
    pub(crate) unknown_names: Vec<(String, luacats::Span)>,
    /// Bad `.luab` generic instantiations reached from an annotation site
    /// (LB2007): `(message, span)`.
    pub(crate) shape_ref_errors: Vec<(String, luacats::Span)>,
    /// Every fully-qualified `.luab` shape type name in scope — the
    /// candidate list LB0305 draws its "did you mean" hint from (#79).
    shape_names: Vec<String>,
    /// FQ shape type name → (declaring file, declaration byte range), so a
    /// conformance error at an annotation site can attach a secondary label
    /// at the declaration (#80).
    shape_decl_sites: BTreeMap<String, (String, std::ops::Range<usize>)>,
}

impl TypeEnv {
    /// Build the environment for one parsed file.
    #[must_use]
    pub fn build(parse: &lua::Parse) -> TypeEnv {
        let items = luacats::harvest(parse);
        Self::build_from_items(parse, &items, None, None)
    }

    /// Build the environment from pre-harvested annotations, optionally
    /// with `.luab` shapes in scope (interop: shape structs/traits/aliases
    /// become referenceable from LuaCATS annotations, and resolvable
    /// through [`TypeEnv::resolve_named`] / [`TypeEnv::class_shape`]).
    ///
    /// `ambient` is the definition-package layer (stdlib + project `defs`,
    /// [`crate::defs::Ambient`]) merged *beneath* the file's own
    /// declarations: its classes/enums/functions/globals seed the
    /// environment first, so a same-named file declaration shadows them.
    pub(crate) fn build_from_items(
        parse: &lua::Parse,
        items: &[luacats::AnnotatedItem],
        shapes: Option<&crate::shape::ShapeScope>,
        ambient: Option<&crate::defs::Ambient>,
    ) -> TypeEnv {
        let mut decl = Declared::default();
        // Ambient type names must be visible to the file's lowerer so
        // annotations that reference stdlib classes/aliases don't trip
        // LB0305. File-declared names (inserted below) win on collision.
        if let Some(ambient) = ambient {
            for name in ambient.env.classes.keys() {
                decl.classes.insert(name.clone());
            }
            for name in ambient.env.enums.keys() {
                decl.enums.insert(name.clone());
            }
            for (name, alias) in &ambient.aliases {
                decl.aliases.insert(name.clone(), alias.clone());
            }
        }
        for item in items {
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
        // Seed the ambient declarations beneath the file's own.
        if let Some(ambient) = ambient {
            env.classes = ambient.env.classes.clone();
            env.enums = ambient.env.enums.clone();
            env.functions = ambient.env.functions.clone();
            env.global_types = ambient.env.global_types.clone();
        }
        if let Some(scope) = shapes {
            // Concrete object types resolve nominally: seed their sealed
            // structural tables so `Ty::Named(fq)` resolves through
            // `class_shape`/`resolve_named`. Templates and alias-like types
            // are handled at the lowerer (monomorphised / expanded inline).
            for (name, shape) in &scope.types {
                if shape.params.is_empty()
                    && let crate::ty::Ty::Table(table) = &shape.ty
                {
                    env.shape_structs.insert(name.clone(), (**table).clone());
                }
            }
            env.shape_names = scope.types.keys().cloned().collect();
            for (name, shape) in &scope.types {
                env.shape_decl_sites
                    .insert(name.clone(), (shape.file.clone(), shape.range.clone()));
            }
        }
        let mut lowerer = Lowerer::new(&decl);
        lowerer.shape_scope = shapes;
        let root = parse.syntax();
        for item in items {
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
        // SHAPES-V2 `self` typing: tie each shape-typed constructor's carrier
        // to its instance type, so `self` in the carrier's methods resolves
        // as the shape — mirroring the `---@class` carrier association an
        // explicit tag would set (an explicit `---@class` still wins).
        env.tie_shape_carriers(items, &root);
        // Inline `--[[@as T]]` casts: anchor each to the end offset of the
        // expression it directly follows (skipping back over whitespace).
        let inline_as = luacats::harvest_inline_as(parse);
        if !inline_as.is_empty() {
            lowerer.generics.clear();
            let text = root.text().to_string();
            let bytes = text.as_bytes();
            for cast in inline_as {
                let ty = lowerer.lower(&cast.ty);
                let mut anchor = cast.span.start.min(bytes.len());
                while anchor > 0 && bytes[anchor - 1].is_ascii_whitespace() {
                    anchor -= 1;
                }
                if anchor > 0 {
                    env.as_casts.insert(anchor, ty);
                }
            }
        }
        env.unknown_names = std::mem::take(&mut lowerer.unknown_names);
        env.shape_ref_errors = std::mem::take(&mut lowerer.shape_ref_errors);
        env
    }

    /// Build the ambient environment for a definition package: a set of
    /// `---@meta` `.d.lua` sources, each already parsed and harvested.
    ///
    /// All files share one declared-name universe (so a class declared in
    /// `io.d.lua` is referenceable from `os.d.lua`), then each is lowered
    /// independently and its name-keyed declarations merged — module tables
    /// and scalar globals surface as [`TypeEnv::global_type`] entries. The
    /// returned alias map lets a consuming file's lowerer expand ambient
    /// `---@alias`es without re-parsing the packages.
    pub(crate) fn build_ambient(
        files: &[(lua::Parse, Vec<luacats::AnnotatedItem>)],
    ) -> (TypeEnv, BTreeMap<String, AliasTag>) {
        let mut decl = Declared::default();
        let mut aliases: BTreeMap<String, AliasTag> = BTreeMap::new();
        for (_, items) in files {
            for item in items {
                for tag in &item.block.tags {
                    match tag {
                        Tag::Class(c) if !c.name.is_empty() => {
                            decl.classes.insert(c.name.clone());
                        }
                        Tag::Alias(a) if !a.name.is_empty() => {
                            decl.aliases.insert(a.name.clone(), a.clone());
                            aliases.insert(a.name.clone(), a.clone());
                        }
                        Tag::Enum(e) if !e.name.is_empty() => {
                            decl.enums.insert(e.name.clone());
                        }
                        _ => {}
                    }
                }
            }
        }

        let mut env = TypeEnv::default();
        for (parse, items) in files {
            let root = parse.syntax();
            let mut file_env = TypeEnv::default();
            let mut lowerer = Lowerer::new(&decl);
            for item in items {
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
                file_env.absorb_block(item, &mut lowerer, &root);
            }
            file_env.collect_global_types(&root);
            // Name-keyed maps merge without collision across files (byte
            // ranges would — hence the per-file lowering above).
            env.classes.append(&mut file_env.classes);
            env.enums.append(&mut file_env.enums);
            env.functions.append(&mut file_env.functions);
            env.global_types.append(&mut file_env.global_types);
            env.unknown_names.append(&mut lowerer.unknown_names.clone());
        }
        (env, aliases)
    }

    /// Bind module tables (`math = {}` under `---@class mathlib`) and scalar
    /// globals (`_VERSION` under `---@type string`) to their declared types,
    /// so field reads like `math.pi` and `_VERSION` resolve. Called once per
    /// definition file, before its range-keyed maps are merged away.
    fn collect_global_types(&mut self, root: &SyntaxNode) {
        for node in root.descendants() {
            let Some(Stmt::Assign(assign)) = Stmt::cast(node.clone()) else {
                continue;
            };
            let targets: Vec<Expr> = assign
                .targets()
                .map(|t| t.exprs().collect())
                .unwrap_or_default();
            let [Expr::Name(name)] = &targets[..] else {
                continue; // only simple single-name globals: `NAME = ...`
            };
            let Some(name) = name.name() else { continue };
            let range = node.text_range();
            let key = (usize::from(range.start()), usize::from(range.end()));
            if let Some(class) = self.declared_targets.get(&key) {
                self.global_types
                    .insert(name.text().to_string(), Ty::Named(class.clone()));
            } else if let Some(types) = self.typed_locals.get(&key)
                && let Some(ty) = types.first()
            {
                self.global_types
                    .insert(name.text().to_string(), ty.clone());
            }
        }
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
        let mut type_span: Option<std::ops::Range<usize>> = None;
        let mut overloads: Vec<FunctionTy> = Vec::new();
        let mut casts: Vec<CastEntry> = Vec::new();

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
                    // Validate each parent reference: lowering a `: Base` that
                    // no class/alias/enum/shape declares records an unknown
                    // name at the parent's own span → LB0305 (#107). Forward
                    // references and defs/ambient parents already sit in the
                    // lowerer's declared-name universe, so they do not fire.
                    for parent in &c.parents {
                        lowerer.lower(parent);
                    }
                    self.classes.insert(
                        c.name.clone(),
                        ClassDef {
                            parents,
                            ..ClassDef::default()
                        },
                    );
                    current_class = Some(c.name.clone());
                    self.class_decl_spans
                        .insert(c.name.clone(), c.span.start..c.span.end);
                    if let Some(span) = item.target {
                        self.declared_targets
                            .insert((span.start, span.end), c.name.clone());
                        self.class_tag_spans
                            .insert((span.start, span.end), c.span.start..c.span.end);
                    }
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
                    type_span = Some(t.span.start..t.span.end);
                }
                Tag::Enum(e) if !e.name.is_empty() => {
                    let def = enum_def(e, item.target, root);
                    self.enums.insert(e.name.clone(), def);
                }
                Tag::Overload(o) => {
                    if let Ty::Function(func) = lowerer.lower(&o.ty) {
                        overloads.push(*func);
                    }
                }
                Tag::Cast(c) if !c.var.is_empty() => casts.push(lower_cast(c, lowerer)),
                _ => {}
            }
        }

        let target = item.target.map(|span| (span.start, span.end));
        if let Some(target) = target
            && !casts.is_empty()
        {
            self.casts.entry(target).or_default().append(&mut casts);
        }
        if (!params.is_empty() || !returns.is_empty() || !overloads.is_empty())
            && let Some(target) = target
        {
            self.attach_function(&params, &returns, overloads, target, lowerer, root);
        }
        if let (Some(types), Some(target)) = (types, target) {
            self.typed_locals.insert(target, types);
            if let Some(span) = type_span {
                self.typed_local_spans.insert(target, span);
            }
        }
    }

    /// Build a [`FunctionTy`] from `@param`/`@return` tags, reconcile it
    /// with the target function's AST parameter list, and register it under
    /// the function's name.
    #[allow(clippy::too_many_lines)]
    fn attach_function(
        &mut self,
        params: &[&ParamTag],
        returns: &[&ReturnTag],
        overloads: Vec<FunctionTy>,
        target: Target,
        lowerer: &mut Lowerer<'_>,
        root: &SyntaxNode,
    ) {
        let mut func = FunctionTy {
            overloads,
            ..FunctionTy::default()
        };
        // Lower every tag up front (unknown type names must be reported even
        // for tags that end up unbound); non-vararg tags are *reconciled by
        // name* against the AST parameter list below.
        let mut tag_params: Vec<ParamTy> = Vec::new();
        for param in params {
            let ty = lowerer.lower(&param.ty);
            if param.vararg {
                func.varargs = Some(ty);
            } else {
                tag_params.push(ParamTy {
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

        // Reconcile with the real parameter list: `@param` tags bind to the
        // parameter of the *same name* (names are mandatory in the tag
        // syntax), so a partially-annotated function never misassociates a
        // tag with the wrong position. Unannotated parameters become
        // optional `unknown` (permissive — partial annotation must not
        // manufacture arity errors), and an unannotated `...` still lifts
        // the arity ceiling.
        //
        // TODO(P2): tags naming no parameter are silently unbound today
        // (LuaLS warns); surface a diagnostic for them.
        // A `:` method's implicit `self` is absent from the AST parameter
        // list; keep an explicit `---@param self T` tag so inference can
        // honor it (the standard-LuaCATS `self` fallback, SHAPES-V2.md).
        let is_method =
            matches!(&stmt, Stmt::FunctionDecl(f) if f.name().is_some_and(|n| n.is_method()));
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
            let mut used = vec![false; tag_params.len()];
            for name in &ast_names {
                let tag = tag_params
                    .iter()
                    .enumerate()
                    .find(|(i, tag)| !used[*i] && &tag.name == name);
                match tag {
                    Some((i, tag)) => {
                        func.params.push(tag.clone());
                        used[i] = true;
                    }
                    None => func.params.push(ParamTy {
                        name: name.clone(),
                        ty: Ty::Unknown,
                        optional: true,
                    }),
                }
            }
            if is_method
                && let Some(pos) = tag_params.iter().position(|p| p.name == "self")
                && !used[pos]
            {
                func.params.insert(0, tag_params[pos].clone());
            }
            if ast_vararg && func.varargs.is_none() {
                func.varargs = Some(Ty::Unknown);
            }
        } else {
            // No AST parameter list to reconcile against (malformed source):
            // fall back to tag order.
            func.params = tag_params;
        }

        if let Some(name) = name {
            self.functions.insert(name, func.clone());
        }
        self.fn_sigs.insert(target, func);
    }

    /// SHAPES-V2 `self` typing: for each shape-typed constructor
    /// (`---@return <fq>` whose first return is a `.luab` object type) whose
    /// body is `return setmetatable(<expr>, <Ident>)`, bind `<Ident>`'s
    /// carrier `local` to the instance type `<fq>`. Inference then resolves
    /// `self` in the carrier's methods through the shape, exactly as a
    /// `---@class` carrier does. An explicit declaration on the carrier wins;
    /// among shape constructors, the first in source order wins.
    fn tie_shape_carriers(&mut self, items: &[luacats::AnnotatedItem], root: &SyntaxNode) {
        let mut ties: Vec<(Target, String)> = Vec::new();
        for item in items {
            let Some(span) = item.target else { continue };
            let target = (span.start, span.end);
            let Some(sig) = self.fn_sigs.get(&target) else {
                continue;
            };
            let Some(Ty::Named(fq)) = sig.returns.first() else {
                continue;
            };
            if !self.shape_structs.contains_key(fq) {
                continue;
            }
            let fq = fq.clone();
            let Some(carrier) = stmt_at(root, target).and_then(|stmt| setmetatable_carrier(&stmt))
            else {
                continue;
            };
            let Some(carrier_stmt) = carrier_local(root, &carrier, target.0) else {
                continue;
            };
            ties.push((carrier_stmt, fq));
        }
        for (target, fq) in ties {
            self.declared_targets.entry(target).or_insert(fq);
        }
    }

    // --- lookups -----------------------------------------------------

    /// The merged structural shape of a class: parents first (depth-first),
    /// own members overriding, with a cycle guard. `.luab` structs and
    /// traits in scope resolve here too (one checker, one IR).
    pub(crate) fn class_shape(&self, name: &str) -> Option<TableTy> {
        if !self.classes.contains_key(name) {
            if let Some(table) = self.shape_structs.get(name) {
                return Some(table.clone());
            }
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
    /// (or `.luab` struct/trait) becomes its table shape, an enum the union
    /// of its member values.
    pub(crate) fn resolve_named(&self, name: &str) -> Option<Ty> {
        if let Some(shape) = self.class_shape(name) {
            return Some(Ty::Table(Box::new(shape)));
        }
        self.enums.get(name).map(|e| e.value_union.clone())
    }

    pub(crate) fn function(&self, name: &str) -> Option<&FunctionTy> {
        self.functions.get(name)
    }

    /// The ambient type of a global value (stdlib module table or scalar
    /// global) declared by a definition package. `None` for names the
    /// active definition packages do not declare.
    pub(crate) fn global_type(&self, name: &str) -> Option<&Ty> {
        self.global_types.get(name)
    }

    pub(crate) fn typed_local(&self, target: Target) -> Option<&[Ty]> {
        self.typed_locals.get(&target).map(Vec::as_slice)
    }

    /// The source span of the `---@type` annotation on a `local` statement.
    pub(crate) fn typed_local_span(&self, target: Target) -> Option<std::ops::Range<usize>> {
        self.typed_local_spans.get(&target).cloned()
    }

    pub(crate) fn fn_sig(&self, target: Target) -> Option<&FunctionTy> {
        self.fn_sigs.get(&target)
    }

    /// The `---@class`/`---@struct` name bound to a statement, if any.
    pub(crate) fn declared_target(&self, target: Target) -> Option<&str> {
        self.declared_targets.get(&target).map(String::as_str)
    }

    /// The `---@class` tag span on the carrier statement `target` — the
    /// anchor a `: Interface` conformance error points at (#107).
    pub(crate) fn class_tag_span(&self, target: Target) -> Option<std::ops::Range<usize>> {
        self.class_tag_spans.get(&target).cloned()
    }

    /// The in-file `---@class` tag span of `name`, when it is declared in the
    /// file under check — the "declared here" secondary label for a parent a
    /// conformance error names (#107). `None` for ambient/defs classes.
    pub(crate) fn class_decl_span(&self, name: &str) -> Option<std::ops::Range<usize>> {
        self.class_decl_spans.get(name).cloned()
    }

    /// The declared parents of a `---@class`, in declaration order.
    pub(crate) fn class_parents(&self, name: &str) -> Option<&[String]> {
        self.classes.get(name).map(|def| def.parents.as_slice())
    }

    /// Whether `name` declares `field` as one of its *own* `---@field`s (as
    /// opposed to inheriting it) — the set a conformance obligation excludes,
    /// since a re-declared member is governed by the class's own declaration
    /// rather than the parent's (#107).
    pub(crate) fn class_declares_own(&self, name: &str, field: &str) -> bool {
        self.classes
            .get(name)
            .is_some_and(|def| def.fields.contains_key(field))
    }

    /// The `---@cast` overrides attached to a statement, if any.
    pub(crate) fn casts_at(&self, target: Target) -> Option<&[CastEntry]> {
        self.casts.get(&target).map(Vec::as_slice)
    }

    /// The inline `--[[@as T]]` cast anchored to an expression ending at
    /// `end_offset`, if any.
    pub(crate) fn as_cast_at(&self, end_offset: usize) -> Option<&Ty> {
        self.as_casts.get(&end_offset)
    }

    /// The `.luab` declaration site of a fully-qualified shape type name:
    /// its declaring file and byte range (#80's "type declared here" label).
    pub(crate) fn shape_decl_site(&self, name: &str) -> Option<(&str, std::ops::Range<usize>)> {
        self.shape_decl_sites
            .get(name)
            .map(|(file, range)| (file.as_str(), range.clone()))
    }

    /// Fully-qualified shape names whose *last* dotted segment matches
    /// `name`'s last segment — the LB0305 "did you mean" candidate list
    /// (#79): covers both a bare short name (`Point`) and a typo'd
    /// namespace (`geomtry.Point`). Capped at 3, in FQ-name order.
    pub(crate) fn shape_name_candidates(&self, name: &str) -> Vec<String> {
        let last = name.rsplit('.').next().unwrap_or(name);
        let mut candidates: Vec<String> = self
            .shape_names
            .iter()
            .filter(|fq| fq.rsplit('.').next() == Some(last))
            .cloned()
            .collect();
        candidates.truncate(3);
        candidates
    }
}

/// Lower one `---@cast` tag's operation list.
fn lower_cast(tag: &luacats::CastTag, lowerer: &mut Lowerer<'_>) -> CastEntry {
    CastEntry {
        var: tag.var.clone(),
        ops: tag
            .ops
            .iter()
            .map(|op| (op.kind, lowerer.lower(&op.ty)))
            .collect(),
    }
}

/// The metatable identifier of a function whose body directly returns
/// `setmetatable(<expr>, <Ident>)` (the SHAPES-V2 constructor idiom).
/// Nested closures are not inspected — a constructor's `setmetatable` return
/// is a top-level statement of its own body.
fn setmetatable_carrier(stmt: &Stmt) -> Option<String> {
    let block = match stmt {
        Stmt::FunctionDecl(f) => f.body(),
        Stmt::LocalFunction(f) => f.body(),
        Stmt::Local(l) => match l.values()?.exprs().next()? {
            Expr::Function(f) => f.body(),
            _ => None,
        },
        _ => None,
    }?;
    for stmt in block.stmts() {
        let Stmt::Return(ret) = stmt else { continue };
        let Some(Expr::Call(call)) = ret.exprs().and_then(|list| list.exprs().next()) else {
            continue;
        };
        let Some(Expr::Name(callee)) = call.callee() else {
            continue;
        };
        if callee.name().is_none_or(|t| t.text() != "setmetatable") {
            continue;
        }
        let Some(args) = call.args().and_then(|a| a.expr_list()) else {
            continue;
        };
        if let Some(Expr::Name(meta)) = args.exprs().nth(1)
            && let Some(name) = meta.name()
        {
            return Some(name.text().to_string());
        }
    }
    None
}

/// The top-level `local <name> = ...` statement nearest before `before`
/// (byte offset). File-local carriers only — no scope construction.
fn carrier_local(root: &SyntaxNode, name: &str, before: usize) -> Option<Target> {
    let block = lua::ast::SourceFile::cast(root.clone())?.block()?;
    let mut best: Option<Target> = None;
    for stmt in block.stmts() {
        let Stmt::Local(local) = &stmt else { continue };
        let declares = local
            .names()
            .filter_map(|n| n.name())
            .any(|t| t.text() == name);
        if !declares {
            continue;
        }
        let range = stmt.syntax().text_range();
        let span = (usize::from(range.start()), usize::from(range.end()));
        if span.0 < before && best.is_none_or(|b| span.0 > b.0) {
            best = Some(span);
        }
    }
    best
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
