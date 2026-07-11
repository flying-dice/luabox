//! The per-file type environment: every declaration the annotations make.
//!
//! Built in two passes over [`luacats::harvest`] output: first collect the
//! *names* of classes/aliases/enums (so forward references resolve), then
//! lower every annotation body against them. The environment is strictly
//! per file (cross-file *class/alias/enum* sharing is the ambient
//! definition layer's job — #108); cross-file `require` *values* are typed
//! separately, by threading a module-export registry into inference rather
//! than into this environment (see [`crate::check_file_with_requires`], #85).

use std::collections::{BTreeMap, HashMap, HashSet};

use luabox_syntax::lua::ast::{AstNode, Expr, LocalStmt, Stmt};
use luabox_syntax::lua::{self, SyntaxKind, SyntaxNode};
use luabox_syntax::luacats::{
    self, AliasTag, CastKind, FieldKey, ParamTag, ReturnTag, Tag, TypeExprKind,
};

use crate::lower::{Declared, GenericClass, Lowerer};
use crate::ty::{FieldTy, FunctionTy, ParamTy, TableTy, Ty, TypeParam};

/// A statement's byte range, used to key annotations to their target.
pub(crate) type Target = (usize, usize);

/// A declared `---@class`: parents plus *own* members (inherited members
/// are merged on demand by [`TypeEnv::class_shape`]).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ClassDef {
    pub parents: Vec<String>,
    pub fields: BTreeMap<String, FieldTy>,
    pub indexers: Vec<(Ty, Ty)>,
    /// Generic type-parameter names from `---@class Name<T>` — empty for a
    /// plain class. A reference `Name<arg>` monomorphises the class shape by
    /// substituting these (#84).
    pub params: Vec<String>,
    /// Members attached to the class *carrier* by statements —
    /// `function Class:method()`, `function Class.fn()`, `Class.const = v` —
    /// as collected from the declaring project file (luals parity:
    /// `---@class` declarations and their member attachments are
    /// workspace-global). They resolve on reads and method calls exactly
    /// like `---@field` members ([`TypeEnv::class_shape`] folds them in) but
    /// carry **no table-literal obligation**: luals's `missing-fields` only
    /// requires `---@field`-declared members, so the literal classifiers
    /// skip these (see [`TypeEnv::class_method_names`]).
    pub methods: BTreeMap<String, FieldTy>,
}

/// A declared `---@enum`: member name → value type, plus the union of all
/// member values (what the enum *type* accepts).
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// The workspace-global type surface one checked project source file
/// contributes (luals parity: `---@class` declarations are
/// workspace-global): its `---@class` definitions — parents, `---@field`
/// members, and carrier member attachments (`function Class:method` etc.)
/// — plus its `---@enum` definitions and its `---@alias` declarations.
/// Collected per file by [`crate::module_surface`] and merged beneath every
/// other file's declarations via [`crate::Ambient::with_project_types`].
///
/// Aliases are carried **raw** (the harvested [`AliasTag`], not a lowered
/// [`Ty`]) exactly as the ambient definition layer carries them: an alias
/// body is only expanded at *lowering* time, inside the consuming file where
/// every workspace-global class/enum/alias it might reference is already in
/// scope. This defers cross-file alias expansion to the one place all the
/// names resolve, and reuses the lowerer's existing cycle guard so a
/// self-referential or mutually-referential alias across files terminates
/// (collapsing to `unknown`) rather than looping (#110).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FileTypes {
    pub(crate) classes: BTreeMap<String, ClassDef>,
    pub(crate) enums: BTreeMap<String, EnumDef>,
    pub(crate) aliases: BTreeMap<String, AliasTag>,
}

impl FileTypes {
    /// Whether this file contributes nothing to the workspace surface.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.classes.is_empty() && self.enums.is_empty() && self.aliases.is_empty()
    }

    /// Collect the file's own `---@class`/`---@enum`/`---@alias` declarations
    /// out of its built environment, folding each class carrier's reified
    /// member attachments (from inference, `carriers`: class name → reified
    /// carrier shape) into the class's `methods` map. Only names the file
    /// itself declares are collected — ambient (defs/stdlib) declarations
    /// seeded into `env` are not re-exported. Aliases are collected raw from
    /// the harvested tags (expansion is deferred to each consumer's lowerer).
    pub(crate) fn collect(
        items: &[luacats::AnnotatedItem],
        env: &TypeEnv,
        carriers: &HashMap<String, Ty>,
    ) -> FileTypes {
        let mut out = FileTypes::default();
        for item in items {
            for tag in &item.block.tags {
                match tag {
                    Tag::Class(c) if !c.name.is_empty() => {
                        let mut def = env.classes.get(&c.name).cloned().unwrap_or_default();
                        if let Some(Ty::Table(carrier)) = carriers.get(&c.name) {
                            for (member, field) in &carrier.fields {
                                if !def.fields.contains_key(member) {
                                    def.methods.insert(member.clone(), field.clone());
                                }
                            }
                        }
                        out.classes.insert(c.name.clone(), def);
                    }
                    Tag::Enum(e) if !e.name.is_empty() => {
                        if let Some(def) = env.enums.get(&e.name) {
                            out.enums.insert(e.name.clone(), def.clone());
                        }
                    }
                    Tag::Alias(a) if !a.name.is_empty() => {
                        out.aliases.insert(a.name.clone(), a.clone());
                    }
                    _ => {}
                }
            }
        }
        out
    }

    /// The `---@alias` declarations this file contributes to the workspace
    /// (name → raw [`AliasTag`]) — folded into the ambient alias map by
    /// [`crate::Ambient::with_project_types`] so a consumer file can name
    /// them (#110).
    pub(crate) fn aliases(&self) -> &BTreeMap<String, AliasTag> {
        &self.aliases
    }
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
    /// The source span of each `---@type` annotation, keyed by its target
    /// `local` statement — the anchor a deferred whole-carrier conformance
    /// error points at.
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
}

impl TypeEnv {
    /// Build the environment for one parsed file.
    #[must_use]
    pub fn build(parse: &lua::Parse) -> TypeEnv {
        let items = luacats::harvest(parse);
        Self::build_from_items(parse, &items, None)
    }

    /// Build the environment from pre-harvested annotations.
    ///
    /// `ambient` is the definition-package layer (stdlib + project `defs`,
    /// [`crate::defs::Ambient`]) merged *beneath* the file's own
    /// declarations: its classes/enums/functions/globals seed the
    /// environment first, so a same-named file declaration shadows them.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn build_from_items(
        parse: &lua::Parse,
        items: &[luacats::AnnotatedItem],
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
        let mut lowerer = Lowerer::new(&decl);
        // Build generic `---@class Name<T>` templates before the main pass so
        // references (`Name<number>`) resolve regardless of declaration order,
        // and ambient generic classes are reachable too (#84).
        lowerer.generic_classes = collect_generic_classes(items, ambient, &mut lowerer);
        let root = parse.syntax();
        for item in items {
            lowerer.generics = block_generics(item);
            env.absorb_block(item, &mut lowerer, &root);
        }
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
                lowerer.generics = block_generics(item);
                file_env.absorb_block(item, &mut lowerer, &root);
            }
            file_env.collect_global_types(&root);
            // Name-keyed maps merge *first-wins*: definition files are supplied
            // in winner-first order (stdlib base, then project-local defs, then
            // dependency defs alphabetically — #108's `workspace.library`
            // precedence), so an earlier source's declaration of a name is kept
            // and a later duplicate is dropped. The collision itself is
            // reported separately (`LB0307`); here we only pick the winner. The
            // stdlib layer never collides across its own files, so this is a
            // no-op for the cached stdlib path.
            merge_keep_first(&mut env.classes, file_env.classes);
            merge_keep_first(&mut env.enums, file_env.enums);
            merge_keep_first(&mut env.functions, file_env.functions);
            merge_keep_first(&mut env.global_types, file_env.global_types);
            env.unknown_names.append(&mut lowerer.unknown_names.clone());
        }
        (env, aliases)
    }

    /// Clone the name-keyed ambient surface of this environment — the four
    /// maps [`Self::build_from_items`] seeds a file's environment from
    /// (classes, enums, functions, global types). Range-keyed per-file maps
    /// are irrelevant to an ambient layer and stay empty. Used by
    /// [`crate::Ambient::with_project_types`] to derive a merged layer
    /// without mutating the shared base.
    pub(crate) fn clone_surface(&self) -> TypeEnv {
        TypeEnv {
            classes: self.classes.clone(),
            enums: self.enums.clone(),
            functions: self.functions.clone(),
            global_types: self.global_types.clone(),
            ..TypeEnv::default()
        }
    }

    /// Merge one project file's workspace-global declarations beneath this
    /// (ambient) environment. See [`crate::Ambient::with_project_types`] for
    /// the semantics: absent classes insert whole; present classes merge
    /// member-wise with the existing (defs) declaration winning same-name
    /// collisions, and a carrier attachment never shadowing a `---@field`;
    /// enums merge first-wins.
    pub(crate) fn merge_file_types(&mut self, file: &FileTypes) {
        for (name, def) in &file.classes {
            match self.classes.get_mut(name) {
                None => {
                    self.classes.insert(name.clone(), def.clone());
                }
                Some(existing) => {
                    for parent in &def.parents {
                        if !existing.parents.contains(parent) {
                            existing.parents.push(parent.clone());
                        }
                    }
                    for (field, ty) in &def.fields {
                        existing
                            .fields
                            .entry(field.clone())
                            .or_insert_with(|| ty.clone());
                    }
                    for (member, ty) in &def.methods {
                        if !existing.fields.contains_key(member) {
                            existing
                                .methods
                                .entry(member.clone())
                                .or_insert_with(|| ty.clone());
                        }
                    }
                    for indexer in &def.indexers {
                        if !existing.indexers.contains(indexer) {
                            existing.indexers.push(indexer.clone());
                        }
                    }
                }
            }
        }
        for (name, def) in &file.enums {
            self.enums
                .entry(name.clone())
                .or_insert_with(|| def.clone());
        }
    }

    /// Bind module tables (`math = {}` under `---@class mathlib`) and scalar
    /// globals (`_VERSION` under `---@type string`) to their declared types,
    /// so field reads like `math.pi` and `_VERSION` resolve. Called once per
    /// definition file, before its range-keyed maps are merged away.
    ///
    /// Also folds def-declared *value* fields on global tables
    /// (`zlib.version = "1.3.1"` with a `---@type string`, or a bare typed
    /// literal) into the base table's shape — the class shape when the base is
    /// `---@class`-declared, or the structural table registered for a plain
    /// `zlib = {}` (the dominant def style) — so `zlib.version` reads back as
    /// its declared/literal type. Function fields already register by dotted
    /// name, but scalar fields had no home before (#105). Statements are
    /// visited in document order, so an intermediate `mylib.sub = {}` folds
    /// before the nested `mylib.sub.const = ...` that resolves through it.
    fn collect_global_types(&mut self, root: &SyntaxNode) {
        for node in root.descendants() {
            let Some(Stmt::Assign(assign)) = Stmt::cast(node.clone()) else {
                continue;
            };
            let targets: Vec<Expr> = assign
                .targets()
                .map(|t| t.exprs().collect())
                .unwrap_or_default();
            let [target] = &targets[..] else {
                continue; // only single-target assignments carry a type here
            };
            let range = node.text_range();
            let key = (usize::from(range.start()), usize::from(range.end()));
            match target {
                Expr::Name(name) => {
                    let Some(name) = name.name() else { continue };
                    if let Some(class) = self.declared_targets.get(&key) {
                        self.global_types
                            .insert(name.text().to_string(), Ty::Named(class.clone()));
                    } else if let Some(types) = self.typed_locals.get(&key)
                        && let Some(ty) = types.first()
                    {
                        self.global_types
                            .insert(name.text().to_string(), ty.clone());
                    } else if is_table_constructor(&assign) {
                        // A *plain* global table (`love = {}`, no `---@class`
                        // — the dominant def style): register an empty
                        // structural table so later dotted value-field
                        // assignments fold into it (#105). `or_insert` keeps
                        // already-folded fields if the name is (unusually)
                        // assigned twice.
                        self.global_types
                            .entry(name.text().to_string())
                            .or_insert_with(|| Ty::Table(Box::default()));
                    }
                }
                Expr::Field(field) => {
                    let (Some(field_name), Some(base)) = (field.field_name(), field.base()) else {
                        continue;
                    };
                    let Some(ty) = self.field_value_ty(&assign, key) else {
                        continue;
                    };
                    // A `---@class`-declared base folds into the class shape; a
                    // plain-table base folds into the structural table stored in
                    // `global_types` (nested paths walk intermediate table
                    // fields either way).
                    if let Some(class) = self.resolve_path_class(&base) {
                        if let Some(def) = self.classes.get_mut(&class) {
                            def.fields
                                .entry(field_name.text().to_string())
                                .or_insert(FieldTy {
                                    ty,
                                    optional: false,
                                });
                        }
                    } else if let Some(table) = self.resolve_path_table_mut(&base) {
                        table
                            .fields
                            .entry(field_name.text().to_string())
                            .or_insert(FieldTy {
                                ty,
                                optional: false,
                            });
                    }
                }
                _ => {}
            }
        }
    }

    /// The declared or literal-widened type written to a global-table value
    /// field: a `---@class`/`---@type` on the assignment first, else the
    /// widened type of a bare literal right-hand side (`= "1.3.1"` → `string`,
    /// matching luals), else an empty structural table for a bare `= {}`
    /// sub-table (so deeper `mylib.sub.const = ...` assignments fold through
    /// it). `None` when nothing types the field.
    fn field_value_ty(&self, assign: &lua::ast::AssignStmt, key: Target) -> Option<Ty> {
        if let Some(class) = self.declared_targets.get(&key) {
            return Some(Ty::Named(class.clone()));
        }
        if let Some(ty) = self.typed_locals.get(&key).and_then(|t| t.first()) {
            return Some(ty.clone());
        }
        let value = assign.values().and_then(|v| v.exprs().next())?;
        if matches!(value, Expr::Table(_)) {
            return Some(Ty::Table(Box::default()));
        }
        literal_ty(&value).map(|t| t.widened())
    }

    /// Resolve a dotted global-table path to the mutable structural table it
    /// names: `zlib` → the plain (`---@class`-less) `zlib = {}` table stored
    /// in `global_types`; `zlib.sub` → its nested table field. `None` when the
    /// path is not a plain-table chain — class-declared bases fold through
    /// [`Self::resolve_path_class`] instead.
    fn resolve_path_table_mut(&mut self, expr: &Expr) -> Option<&mut TableTy> {
        let mut segments: Vec<String> = Vec::new();
        let mut cur = expr.clone();
        let base = loop {
            match cur {
                Expr::Name(name) => break name.name()?.text().to_string(),
                Expr::Field(field) => {
                    segments.push(field.field_name()?.text().to_string());
                    cur = field.base()?;
                }
                _ => return None,
            }
        };
        segments.reverse();
        let mut table = match self.global_types.get_mut(&base)? {
            Ty::Table(table) => table.as_mut(),
            _ => return None,
        };
        for segment in &segments {
            table = match &mut table.fields.get_mut(segment)?.ty {
                Ty::Table(next) => next.as_mut(),
                _ => return None,
            };
        }
        Some(table)
    }

    /// Resolve a dotted global-table path to the `---@class` name it is
    /// declared as: `zlib` via its global binding, `mylib.sub` by walking the
    /// intermediate class's field type. `None` for paths that do not bottom
    /// out at a class-typed global table.
    fn resolve_path_class(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Name(name) => match self.global_types.get(name.name()?.text()) {
                Some(Ty::Named(class)) => Some(class.clone()),
                _ => None,
            },
            Expr::Field(field) => {
                let base_class = self.resolve_path_class(&field.base()?)?;
                let member = field.field_name()?;
                match self.classes.get(&base_class)?.fields.get(member.text()) {
                    Some(FieldTy {
                        ty: Ty::Named(class),
                        ..
                    }) => Some(class.clone()),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Process one annotation block: class/field members, function
    /// signatures, `---@type` locals, and enums.
    #[allow(clippy::too_many_lines)]
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
        let mut fn_generics: Vec<TypeParam> = Vec::new();

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
                    // no class/alias/enum declares records an unknown
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
                            params: c.params.clone(),
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
                Tag::Generic(g) => {
                    for p in &g.params {
                        fn_generics.push(TypeParam {
                            name: p.name.clone(),
                            constraint: p.constraint.as_ref().map(|c| lowerer.lower(c)),
                        });
                    }
                }
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
            let func = FunctionTy {
                overloads,
                generics: fn_generics,
                ..FunctionTy::default()
            };
            self.attach_function(&params, &returns, func, target, lowerer, root);
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
        mut func: FunctionTy,
        target: Target,
        lowerer: &mut Lowerer<'_>,
        root: &SyntaxNode,
    ) {
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
        // honor it (the standard-LuaCATS `self` fallback).
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
        // Carrier-attached members first, then `---@field` declarations —
        // both override inherited members, and a declaration wins over a
        // same-name attachment (annotations are authoritative).
        for (member, ty) in &def.methods {
            shape.fields.insert(member.clone(), ty.clone());
        }
        for (field, ty) in &def.fields {
            shape.fields.insert(field.clone(), ty.clone());
        }
        shape.indexers.extend(def.indexers.iter().cloned());
    }

    /// The member names of `name`'s shape that are carrier attachments
    /// (`function Class:method()` et al.) rather than `---@field`
    /// declarations, across the parent chain. These resolve on reads but
    /// carry no table-literal obligation (luals `missing-fields` parity) —
    /// the literal classifiers exclude them from the required set.
    pub(crate) fn class_method_names(&self, name: &str) -> HashSet<String> {
        let mut methods = HashSet::new();
        let mut declared = HashSet::new();
        let mut stack = vec![name.to_string()];
        let mut seen = HashSet::new();
        while let Some(class) = stack.pop() {
            if !seen.insert(class.clone()) {
                continue;
            }
            let Some(def) = self.classes.get(&class) else {
                continue;
            };
            methods.extend(def.methods.keys().cloned());
            declared.extend(def.fields.keys().cloned());
            stack.extend(def.parents.iter().cloned());
        }
        &methods - &declared
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

    /// Whether `name` is a LuaCATS `---@class` (in-file, def-package, or
    /// cross-package). The `undefined-field` read rule (#90) fires only for
    /// real classes.
    pub(crate) fn is_class(&self, name: &str) -> bool {
        self.classes.contains_key(name)
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
}

/// Merge `from` into `into` keeping the value already present on a key
/// collision (first-wins) — the `workspace.library` precedence for merging
/// definition-package layers (#108). Contrast [`BTreeMap::append`], which is
/// last-wins.
fn merge_keep_first<V>(into: &mut BTreeMap<String, V>, from: BTreeMap<String, V>) {
    for (key, value) in from {
        into.entry(key).or_insert(value);
    }
}

/// The generic type-variable names in scope for one annotation block:
/// `---@generic T` parameters plus any `---@class Name<T>`'s own `<T>`
/// params. Both lower to `Ty::Named` placeholders rather than tripping
/// LB0305 (#84).
fn block_generics(item: &luacats::AnnotatedItem) -> HashSet<String> {
    let mut names = HashSet::new();
    for tag in &item.block.tags {
        match tag {
            Tag::Generic(g) => names.extend(g.params.iter().map(|p| p.name.clone())),
            Tag::Class(c) => names.extend(c.params.iter().cloned()),
            _ => {}
        }
    }
    names
}

/// Build the generic `---@class Name<T>` templates the lowerer instantiates at
/// reference sites: file-declared classes (their `---@field` bodies lowered
/// with the params in scope) and ambient generic classes (already lowered with
/// placeholders in the definition package). Parent fields are not folded into
/// the template — field-level substitution through the class's own declared
/// fields is the bar (#84); inherited generic fields are deliberately shallow.
fn collect_generic_classes(
    items: &[luacats::AnnotatedItem],
    ambient: Option<&crate::defs::Ambient>,
    lowerer: &mut Lowerer<'_>,
) -> BTreeMap<String, GenericClass> {
    let mut out: BTreeMap<String, GenericClass> = BTreeMap::new();
    if let Some(ambient) = ambient {
        for (name, def) in &ambient.env.classes {
            if def.params.is_empty() {
                continue;
            }
            out.insert(
                name.clone(),
                GenericClass {
                    params: def.params.clone(),
                    template: TableTy {
                        fields: def.fields.clone(),
                        indexers: def.indexers.clone(),
                        ..TableTy::default()
                    },
                },
            );
        }
    }
    for item in items {
        for tag in &item.block.tags {
            let Tag::Class(c) = tag else { continue };
            if c.params.is_empty() || c.name.is_empty() {
                continue;
            }
            let template = lower_class_template(&item.block.tags, &c.name, &c.params, lowerer);
            out.insert(
                c.name.clone(),
                GenericClass {
                    params: c.params.clone(),
                    template,
                },
            );
        }
    }
    out
}

/// Lower one generic class's own `---@field`s into a template table, with its
/// `<T>` params in scope so each `T` becomes a `Ty::Named(T)` placeholder.
/// Fields between the class's tag and the next `---@class` in the block belong
/// to it (mirrors [`TypeEnv::absorb_block`]'s `current_class` tracking).
fn lower_class_template(
    tags: &[Tag],
    class_name: &str,
    params: &[String],
    lowerer: &mut Lowerer<'_>,
) -> TableTy {
    let saved = std::mem::replace(&mut lowerer.generics, params.iter().cloned().collect());
    let mut table = TableTy::default();
    let mut active = false;
    for tag in tags {
        match tag {
            Tag::Class(c) => active = c.name == class_name,
            Tag::Field(f) if active => {
                let ty = lowerer.lower(&f.ty);
                match &f.key {
                    FieldKey::Name(name) => {
                        table.fields.insert(
                            name.clone(),
                            FieldTy {
                                ty,
                                optional: f.optional,
                            },
                        );
                    }
                    FieldKey::Indexer(key) => {
                        let key = lowerer.lower(key);
                        table.indexers.push((key, ty));
                    }
                }
            }
            _ => {}
        }
    }
    lowerer.generics = saved;
    table
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

/// Whether an assignment's (first) right-hand side is a table constructor —
/// the `NAME = {}` def idiom that declares a plain global module table (#105).
fn is_table_constructor(assign: &lua::ast::AssignStmt) -> bool {
    matches!(
        assign.values().and_then(|v| v.exprs().next()),
        Some(Expr::Table(_))
    )
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
