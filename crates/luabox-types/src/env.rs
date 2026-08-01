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
    self, AliasTag, CastKind, FieldKey, FieldScope, ParamTag, ReturnTag, Tag, TypeExprKind,
    VarargTag,
};

use crate::lower::{Declared, GenericClass, Lowerer};
use crate::ty::{FieldTy, FunctionTy, OperatorSig, ParamTy, TableTy, Ty, TypeParam};

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
    /// `---@operator <op>[(<input>)]: <result>` overloads declared on the
    /// class, keyed by operator name (`add`, `sub`, `unm`, `len`, ...). A key
    /// maps to every overload of that operator in declaration order; inference
    /// picks the first whose `input` accepts the other operand (#114). Like
    /// `---@field`s these ride the workspace-global class surface, so an
    /// operator declared on a class in one file applies wherever the class is
    /// used (cross-file / defs-package).
    pub operators: BTreeMap<String, Vec<OperatorSig>>,
    /// Non-public member visibility (luals `invisible`, #115): member name →
    /// its declared scope, recorded only for `private` / `protected` /
    /// `package` members (a public member has no entry). Populated from
    /// `---@field <scope> name` modifiers and from standalone `---@private` /
    /// `---@protected` / `---@package` doc blocks on `function Class:method`
    /// declarations. Like `---@field`s these ride the workspace-global class
    /// surface, so a member's visibility declared on a class in one file is
    /// enforced wherever the class is used (cross-file / defs-package). The
    /// *owner* of a restricted member is the class this map lives on; a
    /// subclass re-declaring the member as a plain `---@field` (no scope)
    /// overrides it back to public (see [`TypeEnv::member_visibility`]).
    pub visibility: BTreeMap<String, FieldScope>,
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
                                // A same-name `---@field` shadows the attachment
                                // on type, but keeps its use-site tags (#33).
                                if let Some(declared) = def.fields.get(member) {
                                    let merged = with_carrier_tags(declared, field);
                                    def.fields.insert(member.clone(), merged);
                                } else {
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
    /// Generic references whose `<...>` argument count is wrong (LB0313, #117).
    pub(crate) arity_errors: Vec<crate::lower::ArityError>,
    /// Cyclic aliases caught by the lowerer's cycle guard (LB0314, #123).
    pub(crate) cyclic_aliases: Vec<crate::lower::CyclicAlias>,
    /// The `---@class` names declared by *this* file's own annotations (not
    /// the ambient/defs layer seeded beneath). The `package`-visibility test
    /// (luals `invisible`, #115): a `package` member is accessible only in the
    /// file that declares its class, so an access is in-package iff the owner
    /// class is one this file declares.
    local_classes: HashSet<String>,
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
        decl.absorb_tags(items);

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
        // The standalone `---@private`/`---@protected`/`---@package` visibility
        // on `function Class:method` doc blocks, resolved once every class in
        // the file is known. (`local_classes` — the classes this file itself
        // declares, the `package`-visibility test of #115 — is filled by
        // `absorb_block` as each `---@class` is seen, because the duplicate
        // merge of #49 needs it during the walk, not after it.)
        env.absorb_standalone_visibility(items, &root);
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
        env.arity_errors = std::mem::take(&mut lowerer.arity_errors);
        env.cyclic_aliases = std::mem::take(&mut lowerer.cyclic_aliases);
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
        for (_, items) in files {
            decl.absorb_tags(items);
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
            // Definition files are never inferred, so their carrier-style
            // member definitions are folded into the class surface here (#39).
            file_env.absorb_carrier_members(items, &root);
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
            env.arity_errors.append(&mut lowerer.arity_errors.clone());
            env.cyclic_aliases
                .append(&mut lowerer.cyclic_aliases.clone());
        }
        // The declared-name universe already holds every ambient alias raw;
        // hand that map back rather than maintaining a second copy.
        (env, decl.aliases)
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
                    // Type parameters follow the same first-wins rule as
                    // members, with the same allowance the in-file merge makes:
                    // a declaration that named none has none to keep.
                    if existing.params.is_empty() {
                        existing.params.clone_from(&def.params);
                    }
                    // …and one that spells them differently is unified
                    // **positionally**, exactly as two declarations in one file
                    // are, so its field bodies still substitute at an
                    // instantiation site instead of leaking the other
                    // declaration's parameter name through the merged class.
                    let rename = (!def.params.is_empty() && def.params != existing.params)
                        .then(|| positional_rename(&def.params, &existing.params));
                    let subst = |field: &FieldTy| match &rename {
                        Some(map) => FieldTy {
                            ty: crate::generics::subst_ty(&field.ty, map),
                            optional: field.optional,
                        },
                        None => field.clone(),
                    };
                    for (field, ty) in &def.fields {
                        existing
                            .fields
                            .entry(field.clone())
                            .or_insert_with(|| subst(ty));
                    }
                    for (member, ty) in &def.methods {
                        // As in [`FileTypes::collect`]: the existing `---@field`
                        // declaration wins on type, but inherits the incoming
                        // attachment's use-site tags (#33).
                        if let Some(declared) = existing.fields.get(member) {
                            let merged = with_carrier_tags(declared, &subst(ty));
                            existing.fields.insert(member.clone(), merged);
                        } else {
                            existing
                                .methods
                                .entry(member.clone())
                                .or_insert_with(|| subst(ty));
                        }
                    }
                    for (key, value) in &def.indexers {
                        let indexer = match &rename {
                            Some(map) => (
                                crate::generics::subst_ty(key, map),
                                crate::generics::subst_ty(value, map),
                            ),
                            None => (key.clone(), value.clone()),
                        };
                        if !existing.indexers.contains(&indexer) {
                            existing.indexers.push(indexer);
                        }
                    }
                    for (op, sigs) in &def.operators {
                        let slot = existing.operators.entry(op.clone()).or_default();
                        for sig in sigs {
                            if !slot.contains(sig) {
                                slot.push(sig.clone());
                            }
                        }
                    }
                    for (member, scope) in &def.visibility {
                        existing.visibility.entry(member.clone()).or_insert(*scope);
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

    /// Merge one *implicit* surface — a class/enum set harvested from a
    /// vendored rock source (#30) — beneath this environment, keeping any name
    /// already claimed **whole**.
    ///
    /// The difference from [`Self::merge_file_types`] is deliberate. Two
    /// declarations of a class in code the user *wrote* are two halves of one
    /// intent, so luals (and luabox) union their members. A rock's declaration
    /// is not the user's: when a project declares `mylib.Point` itself — in
    /// `[types] defs` or in its own source — it is correcting or replacing what
    /// the rock says, and unioning the rock's fields back in would defeat the
    /// escape hatch. So an already-claimed name is left exactly as it is, and
    /// only unclaimed names are inserted.
    pub(crate) fn insert_unclaimed_types(&mut self, file: &FileTypes) {
        for (name, def) in &file.classes {
            self.classes
                .entry(name.clone())
                .or_insert_with(|| def.clone());
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
        // The positional unification a re-declaration's field bodies need when
        // it spells the class's type parameters differently from the canonical
        // (first) list — `None` while the two agree, which is the common case.
        let mut class_rename: Option<BTreeMap<String, Ty>> = None;
        let mut params: Vec<&ParamTag> = Vec::new();
        let mut returns: Vec<&ReturnTag> = Vec::new();
        // Legacy `---@vararg Type`: the deprecated EmmyLua spelling of
        // `---@param ... Type`. luals binds both a `doc.vararg` tag and a
        // `doc.param` `...` tag onto the same AST vararg source and merges
        // (unions) every bound doc's type via `vm.setNode` — see
        // `script/vm/compiler.lua`'s `'...'` case, which loops over
        // `source.bindDocs` and calls `vm.setNode` (additive, not `cover`)
        // for each `doc.vararg`/`doc.param` match. So when both a `---@vararg`
        // and a `---@param ...` tag appear on the same block, the resulting
        // vararg element type is their union, not "last tag wins".
        let mut varargs: Vec<&VarargTag> = Vec::new();
        let mut types: Option<Vec<Ty>> = None;
        let mut type_span: Option<std::ops::Range<usize>> = None;
        let mut overloads: Vec<FunctionTy> = Vec::new();
        let mut casts: Vec<CastEntry> = Vec::new();
        let mut fn_generics: Vec<TypeParam> = Vec::new();
        // `---@deprecated` / `---@nodiscard` / `---@async` attach to the whole
        // doc block, so a block that annotates a function carries them onto
        // that function's signature (#111, #112). Scanned alongside the other
        // tags. `---@async` drives the `await-in-sync` call-site check
        // (LB0316): luals decides a `function` is async iff any of its
        // `bindDocs` is a `doc.async` tag (`script/vm/doc.lua`'s local
        // `isAsync`: `for _, doc in ipairs(value.bindDocs) do if doc.type ==
        // 'doc.async' then value._async = true`).
        let mut deprecated = false;
        let mut nodiscard = false;
        let mut is_async = false;
        // `---@version` rides the same block→signature path as `---@deprecated`
        // (luals has no dedicated version diagnostic — see [`crate::version`]).
        // Parsed out of the raw `SimpleTag` body; an empty/unrecognised body
        // yields `None` and gates nothing.
        let mut version: Option<crate::version::VersionReq> = None;

        for tag in &item.block.tags {
            match tag {
                Tag::Deprecated(_) => deprecated = true,
                Tag::Nodiscard(_) => nodiscard = true,
                Tag::Async(_) => is_async = true,
                Tag::Version(v) => {
                    version = v
                        .text
                        .as_deref()
                        .and_then(crate::version::VersionReq::parse);
                }
                Tag::Class(c) if !c.name.is_empty() => {
                    let parents: Vec<String> = c
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
                    // Two declarations of one class in the same file are two
                    // halves of one intent, exactly as two declarations across
                    // files are: they **union** (#49). Before this, the second
                    // declaration replaced the first and every member it had
                    // collected vanished — a merge rule that depended on the
                    // file boundary, which luals has no notion of.
                    //
                    // The *first* declaration in this file still replaces any
                    // ambient (stdlib / `[types] defs`) class of the same name
                    // whole: that is the escape hatch (see
                    // `Ambient::with_rock_types`), a different axis from
                    // duplicate declarations in code the user wrote.
                    // `local_classes` — the set of names *this file* declares —
                    // is what tells the two apart, so it is filled here rather
                    // than in a later sweep.
                    if self.local_classes.insert(c.name.clone()) {
                        self.classes.insert(
                            c.name.clone(),
                            ClassDef {
                                parents,
                                params: c.params.clone(),
                                ..ClassDef::default()
                            },
                        );
                    } else if let Some(existing) = self.classes.get_mut(&c.name) {
                        for parent in parents {
                            if !existing.parents.contains(&parent) {
                                existing.parents.push(parent);
                            }
                        }
                        // A re-declaration may name the type parameters the
                        // first one omitted; it never renames them (first-wins,
                        // as for every other member).
                        if existing.params.is_empty() {
                            existing.params.clone_from(&c.params);
                        }
                    }
                    // A re-declaration's `---@field` bodies are written against
                    // the type parameters *it* names, so when those differ from
                    // the canonical list they are unified **positionally** as
                    // the fields are absorbed: slot `i` is one type variable
                    // however the two declarations spell it. Without this the
                    // merged class carries a field typed by a name its own
                    // parameter list never mentions, and instantiating it
                    // leaves that name unsubstituted — visible across the
                    // `require` boundary, where the merged class is what the
                    // consumer sees.
                    class_rename = self
                        .classes
                        .get(&c.name)
                        .filter(|def| !c.params.is_empty() && def.params != c.params)
                        .map(|def| positional_rename(&c.params, &def.params));
                    current_class = Some(c.name.clone());
                    // First-wins, matching the fields: the "declared here"
                    // label points at the declaration that introduced the name.
                    self.class_decl_spans
                        .entry(c.name.clone())
                        .or_insert(c.span.start..c.span.end);
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
                    let ty = match &class_rename {
                        Some(map) => crate::generics::subst_ty(&ty, map),
                        None => ty,
                    };
                    match &f.key {
                        FieldKey::Name(name) => {
                            // **First declaration wins**, whether the duplicate
                            // sits in this `---@class` block, in a second block
                            // for the same class in this file (#49), or in
                            // another file (`TypeEnv::merge_file_types`, which
                            // has always been first-wins). `LB0311`
                            // (`duplicate-doc-field`) already promised exactly
                            // this in its note — "the first declaration wins;
                            // remove or rename this one" — and now the stored
                            // type agrees with the message. luals unions the
                            // two declared types instead; luabox keeps the
                            // first deterministically and warns, the same trade
                            // it makes for duplicate aliases and enums (#110).
                            if class.fields.contains_key(name) {
                                continue;
                            }
                            class.fields.insert(
                                name.clone(),
                                FieldTy {
                                    ty,
                                    optional: f.optional,
                                },
                            );
                            // Record a non-public `---@field <scope>` modifier
                            // for the visibility check (#115). A plain (public)
                            // field is left out of the map — and clears any
                            // inherited restriction — so a same-name public
                            // re-declaration on a *subclass* reads as public.
                            match f.scope {
                                Some(FieldScope::Public) | None => {
                                    class.visibility.remove(name);
                                }
                                Some(scope) => {
                                    class.visibility.insert(name.clone(), scope);
                                }
                            }
                        }
                        FieldKey::Indexer(key) => {
                            let key = lowerer.lower(key);
                            let key = match &class_rename {
                                Some(map) => crate::generics::subst_ty(&key, map),
                                None => key,
                            };
                            class.indexers.push((key, ty));
                        }
                    }
                }
                Tag::Operator(o) if !o.op.is_empty() => {
                    let Some(class) = current_class
                        .as_ref()
                        .and_then(|name| self.classes.get_mut(name))
                    else {
                        continue; // a stray @operator outside a @class block
                    };
                    let input = o.input.as_ref().map(|t| lowerer.lower(t));
                    let result = lowerer.lower(&o.result);
                    class
                        .operators
                        .entry(o.op.clone())
                        .or_default()
                        .push(OperatorSig { input, result });
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
                Tag::Vararg(v) => varargs.push(v),
                _ => {}
            }
        }

        let target = item.target.map(|span| (span.start, span.end));
        if let Some(target) = target
            && !casts.is_empty()
        {
            self.casts.entry(target).or_default().append(&mut casts);
        }
        // A bare `---@deprecated`/`---@nodiscard` block (no `---@param`/
        // `---@return`) must still attach a signature so the flag reaches the
        // function's use sites; `attach_function` no-ops on non-function
        // targets, so deprecating a `---@class` carrier is harmless here.
        let has_sig_tags = !params.is_empty()
            || !returns.is_empty()
            || !overloads.is_empty()
            || !varargs.is_empty();
        // The block's use-site tags on their own — `fun(…)` syntax has nowhere
        // to write them, so a `---@type fun(…)` declaration folds them in
        // ([`merge_block_tags`], #38).
        let block_tags = FunctionTy {
            deprecated,
            nodiscard,
            is_async,
            version: version.clone(),
            ..FunctionTy::default()
        };
        if (has_sig_tags || deprecated || nodiscard || is_async || version.is_some())
            && let Some(target) = target
        {
            let mut func = FunctionTy {
                overloads,
                generics: fn_generics,
                deprecated,
                nodiscard,
                is_async,
                version,
                // A signature was *written* here, so calls may be checked
                // against it — including from another module, where this flag
                // is the only surviving evidence that a human wrote one (#46).
                // A block carrying only use-site flags declares no signature.
                declared: has_sig_tags,
                ..FunctionTy::default()
            };
            // With no signature tags the block contributes only the flag:
            // stay fully arity/type-permissive (an unannotated function is
            // never arity-checked), so `---@deprecated`/`---@async` alone never
            // manufactures an `LB0301`/`LB0300`.
            if !has_sig_tags {
                func.varargs = Some(Ty::Any);
            }
            self.attach_function(&params, &returns, &varargs, func, target, lowerer, root);
        }
        if let (Some(types), Some(target)) = (types, target) {
            self.attach_typed_functions(&types, target, root, &block_tags);
            self.typed_locals.insert(target, types);
            if let Some(span) = type_span {
                self.typed_local_spans.insert(target, span);
            }
        }
    }

    /// Register the callables a `---@type fun(…)` declares on an *assignment*
    /// under their dotted names (#38).
    ///
    /// A dotted call (`M.a(…)`) is argument-checked through the by-name
    /// callable map — the same map `function M.a() end` and
    /// `---@param`-annotated `M.a = function() end` register into — so an
    /// explicit `---@type fun(…)` has to land there too, or the declared
    /// signature reaches the value but never the call site. `---@type A, B` is
    /// positional, so each target takes the type in its own slot and a lone
    /// annotation over `a, b = f, g` declares `a` only.
    ///
    /// Restricted to a function-*literal* right-hand side: `---@type fun(…)`
    /// over a call result or a name declares that variable's type without
    /// defining a callable here, and inference already carries it.
    fn attach_typed_functions(
        &mut self,
        types: &[Ty],
        target: Target,
        root: &SyntaxNode,
        block_tags: &FunctionTy,
    ) {
        let Some(Stmt::Assign(assign)) = stmt_at(root, target) else {
            return;
        };
        let Some(target_list) = assign.targets() else {
            return;
        };
        let values: Vec<Expr> = assign
            .values()
            .map(|v| v.exprs().collect())
            .unwrap_or_default();
        for (i, assigned) in target_list.exprs().enumerate() {
            let Some(Ty::Function(sig)) = types.get(i) else {
                continue;
            };
            if !matches!(values.get(i), Some(Expr::Function(_))) {
                continue;
            }
            let Some(name) = assign_target_name(&assigned) else {
                continue;
            };
            self.functions
                .insert(name, merge_block_tags((**sig).clone(), Some(block_tags)));
        }
    }

    /// Record the standalone `---@private` / `---@protected` / `---@package`
    /// visibility declared on `function Class:method` (or `function
    /// Class.member`) doc blocks (#115). The `---@field <scope>` modifier form
    /// is handled inline in [`Self::absorb_block`]; this covers the tag-above-a-
    /// method form luals also accepts.
    ///
    /// The carrier *variable* a method attaches to (`function M:m()`) need not
    /// share the `---@class` name (`---@class Animal local M = {}`), so a
    /// variable→class map is built first from every class carrier's declaring
    /// statement, then each method block's first path segment is resolved
    /// through it. A block that also carries a `---@class` is skipped (the tag
    /// then documents the class, not a member). Unresolvable carriers are left
    /// alone — the conservative direction (no false `invisible`).
    fn absorb_standalone_visibility(
        &mut self,
        items: &[luacats::AnnotatedItem],
        root: &SyntaxNode,
    ) {
        let var_to_class = carrier_var_classes(items, root);

        for item in items {
            // A block documenting a class is not a member visibility block.
            if item.block.tags.iter().any(|t| matches!(t, Tag::Class(_))) {
                continue;
            }
            let Some(scope) = item.block.tags.iter().find_map(standalone_scope) else {
                continue;
            };
            let Some(span) = item.target else { continue };
            let Some(stmt) = stmt_at(root, (span.start, span.end)) else {
                continue;
            };
            let Some((carrier, member)) = visibility_carrier_member(&stmt) else {
                continue;
            };
            let Some(class) = var_to_class.get(&carrier) else {
                continue;
            };
            if let Some(def) = self.classes.get_mut(class) {
                def.visibility.insert(member, scope);
            }
        }
    }

    /// Fold carrier-style member definitions — `function Class:method(…)` and
    /// `function Class.fn(…)` — into their class's surface (#39).
    ///
    /// A checked *project* file gets this for free: inference walks the carrier
    /// table and [`FileTypes::collect`] folds the reified shape into the class.
    /// A `---@meta` definition file is never inferred (it declares, it does not
    /// compute), so a carrier attachment there previously reached nothing and
    /// every use site reported `LB0306`. luals makes no such distinction — a
    /// `function Class:method()` in a library file is a member of `Class`
    /// wherever it is written — so the same attachments are harvested
    /// syntactically here, tags and all.
    ///
    /// The `---@field` declaration stays authoritative on collision, with the
    /// attachment's use-site tags folded in exactly as [`with_carrier_tags`]
    /// does elsewhere (#33). An attachment with no doc block at all still joins
    /// the surface, at a fully permissive signature — its member *exists*, and
    /// an unannotated function is never arity-checked.
    fn absorb_carrier_members(&mut self, items: &[luacats::AnnotatedItem], root: &SyntaxNode) {
        let var_to_class = carrier_var_classes(items, root);
        if var_to_class.is_empty() {
            return;
        }
        for node in root.descendants() {
            let Some(Stmt::FunctionDecl(decl)) = Stmt::cast(node.clone()) else {
                continue;
            };
            let Some(name) = decl.name() else { continue };
            let segments: Vec<String> = name.segments().map(|s| s.text().to_string()).collect();
            // One hop only: `function C.a.b()` attaches to the member table
            // `C.a`, not to `C` — this harvest models class members, not
            // nested tables. It is not dropped, though: a dotted declaration
            // is also registered by its full name (`resolve_callable_target`),
            // which is how the stdlib's `function io.open()` is reached.
            let [carrier, member] = segments.as_slice() else {
                continue;
            };
            let Some(class) = var_to_class.get(carrier).cloned() else {
                continue;
            };
            let r = node.text_range();
            let sig = self
                .fn_sigs
                .get(&(usize::from(r.start()), usize::from(r.end())))
                .cloned()
                .unwrap_or_else(FunctionTy::opaque);
            let attached = FieldTy {
                ty: Ty::Function(Box::new(sig)),
                optional: false,
            };
            let Some(def) = self.classes.get_mut(&class) else {
                continue;
            };
            match def.fields.get(member) {
                Some(declared) => {
                    let merged = with_carrier_tags(declared, &attached);
                    def.fields.insert(member.clone(), merged);
                }
                None => {
                    def.methods.insert(member.clone(), attached);
                }
            }
        }
    }

    /// Build a [`FunctionTy`] from `@param`/`@return` tags, reconcile it
    /// with the target function's AST parameter list, and register it under
    /// the function's name. Orchestration only: [`lower_signature_tags`]
    /// lowers the tags, [`resolve_callable_target`] finds the name and AST
    /// parameter list, [`reconcile_params`] binds the two together.
    #[allow(clippy::too_many_arguments)]
    fn attach_function(
        &mut self,
        params: &[&ParamTag],
        returns: &[&ReturnTag],
        varargs_tags: &[&VarargTag],
        mut func: FunctionTy,
        target: Target,
        lowerer: &mut Lowerer<'_>,
        root: &SyntaxNode,
    ) {
        let tag_params = lower_signature_tags(&mut func, params, returns, varargs_tags, lowerer);

        let Some(stmt) = stmt_at(root, target) else {
            return;
        };
        let Some((name, param_list)) = resolve_callable_target(&stmt) else {
            return;
        };
        // A `:` method's implicit `self` is absent from the AST parameter
        // list; [`reconcile_params`] restores it from an explicit
        // `---@param self T` tag, so the callable kind is decided here.
        let is_method =
            matches!(&stmt, Stmt::FunctionDecl(f) if f.name().is_some_and(|n| n.is_method()));
        reconcile_params(&mut func, tag_params, param_list, is_method);

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

    /// Every `---@operator <op>` overload in scope for a class, own
    /// declarations first then inherited (depth-first over the parent chain),
    /// in declaration order — the sequence inference scans for the first
    /// overload whose parameter accepts the other operand (#114). Own
    /// operators precede inherited ones so a subclass override wins.
    pub(crate) fn class_operators(&self, name: &str, op: &str) -> Vec<OperatorSig> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        self.collect_operators(name, op, &mut out, &mut seen);
        out
    }

    fn collect_operators(
        &self,
        name: &str,
        op: &str,
        out: &mut Vec<OperatorSig>,
        seen: &mut HashSet<String>,
    ) {
        if !seen.insert(name.to_string()) {
            return;
        }
        let Some(def) = self.classes.get(name) else {
            return;
        };
        if let Some(sigs) = def.operators.get(op) {
            out.extend(sigs.iter().cloned());
        }
        for parent in &def.parents {
            self.collect_operators(parent, op, out, seen);
        }
    }

    pub(crate) fn enum_member(&self, enum_name: &str, member: &str) -> Option<&Ty> {
        self.enums.get(enum_name)?.members.get(member)
    }

    /// The members of a declared `---@enum`, as `(member name, value type)`
    /// pairs in declaration-stable order — the finite domain the exhaustiveness
    /// check (`LB0315`) enumerates. `None` when `name` is not an enum.
    pub(crate) fn enum_members(&self, name: &str) -> Option<Vec<(String, Ty)>> {
        let def = self.enums.get(name)?;
        Some(
            def.members
                .iter()
                .map(|(member, ty)| (member.clone(), ty.clone()))
                .collect(),
        )
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

    /// The visibility of `member` as seen through `class`, plus the class that
    /// *owns* (declares) the restriction — the luals `invisible` lookup (#115).
    /// `None` when the member is public (no restriction anywhere the resolution
    /// reaches). Resolution walks own declarations first, then parents: the
    /// nearest declaration wins, so a subclass that re-declares an inherited
    /// restricted member as a plain `---@field` (recorded as *not* in
    /// `visibility`, but present in `fields`) overrides it back to public.
    pub(crate) fn member_visibility(
        &self,
        class: &str,
        member: &str,
    ) -> Option<(FieldScope, String)> {
        let mut stack = vec![class.to_string()];
        let mut seen = HashSet::new();
        while let Some(name) = stack.pop() {
            if !seen.insert(name.clone()) {
                continue;
            }
            let Some(def) = self.classes.get(&name) else {
                continue;
            };
            if let Some(scope) = def.visibility.get(member) {
                return Some((*scope, name));
            }
            // A plain (public) re-declaration of the member here shadows any
            // parent restriction — resolution stops, member is public.
            if def.fields.contains_key(member) || def.methods.contains_key(member) {
                return None;
            }
            stack.extend(def.parents.iter().cloned());
        }
        None
    }

    /// Whether `class` is `ancestor` or transitively extends it (`---@class
    /// Child : ancestor`) — the `protected` reachability test (#115).
    pub(crate) fn is_subclass(&self, class: &str, ancestor: &str) -> bool {
        let mut stack = vec![class.to_string()];
        let mut seen = HashSet::new();
        while let Some(name) = stack.pop() {
            if name == ancestor {
                return true;
            }
            if !seen.insert(name.clone()) {
                continue;
            }
            if let Some(def) = self.classes.get(&name) {
                stack.extend(def.parents.iter().cloned());
            }
        }
        false
    }

    /// Whether `class` is declared by *this* file's own annotations (the
    /// `package`-visibility test — a `package` member is reachable only in the
    /// file that declares its owner class, #115).
    pub(crate) fn declares_class_locally(&self, class: &str) -> bool {
        self.local_classes.contains(class)
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

/// Lower a function block's `@param`/`@return`/`@vararg` tags into `func`,
/// returning the non-vararg `@param` tags as [`ParamTy`]s for reconciliation
/// against the AST parameter list ([`reconcile_params`]).
fn lower_signature_tags(
    func: &mut FunctionTy,
    params: &[&ParamTag],
    returns: &[&ReturnTag],
    varargs_tags: &[&VarargTag],
    lowerer: &mut Lowerer<'_>,
) -> Vec<ParamTy> {
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
    // Legacy `---@vararg Type`. luals precedent (see the comment
    // in `absorb_block`): a `---@vararg` and a `---@param ... Type` on
    // the same block both bind to the AST `...` node and their types are
    // merged, so union rather than overwrite — this covers both a
    // `---@vararg` next to a `---@param ...` and several `---@vararg`
    // tags on one block (each is one more bound doc in luals).
    for v in varargs_tags {
        let ty = lowerer.lower(&v.ty);
        func.varargs = Some(match func.varargs.take() {
            Some(existing) => Ty::union(vec![existing, ty]),
            None => ty,
        });
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
    tag_params
}

/// Extract the callable's registered name (dotted, `None` for an anonymous or
/// method target) and its AST parameter list from the statement it annotates.
/// `None` when the target is not a function definition at all — there is then
/// nothing to attach a signature to.
fn resolve_callable_target(stmt: &Stmt) -> Option<(Option<String>, Option<lua::ast::ParamList>)> {
    match stmt {
        Stmt::LocalFunction(f) => Some((f.name().map(|t| t.text().to_string()), f.param_list())),
        Stmt::FunctionDecl(f) => {
            let name = f.name().and_then(|n| {
                // Methods (`function M:m()`) have an implicit `self`
                // parameter — TODO(P1): resolve method calls; skipped
                // from the callable map for now.
                if n.is_method() {
                    None
                } else {
                    let joined: Vec<String> = n.segments().map(|s| s.text().to_string()).collect();
                    Some(joined.join("."))
                }
            });
            Some((name, f.param_list()))
        }
        Stmt::Local(l) => {
            let value = l.values().and_then(|v| v.exprs().next());
            let Some(Expr::Function(f)) = value else {
                return None;
            };
            Some((
                l.names()
                    .next()
                    .and_then(|n| n.name())
                    .map(|t| t.text().to_string()),
                f.param_list(),
            ))
        }
        // `return function(…) end` — a module whose entire export *is* a
        // function, the `direct` row of #46's table. The doc block above the
        // `return` binds to the function value exactly as it does above a
        // `local f = function(…) end`, so a single-function module can carry a
        // signature at all; without this its `---@param`s bind to nothing and
        // the module is unchecked in its own file as well as in every
        // consumer. No name to register under — the function is reached only
        // through `require`, never by a dotted name in this file.
        Stmt::Return(ret) => {
            let value = ret.exprs().and_then(|v| v.exprs().next());
            let Some(Expr::Function(f)) = value else {
                return None;
            };
            Some((None, f.param_list()))
        }
        // `Carrier.m = function(…) end` / `g = function(…) end` — the
        // assignment spelling of a function definition. luals binds a doc
        // block's `---@param`/`---@return`/`---@deprecated` to the function
        // value on the right of a `setfield`/`setglobal` exactly as it does
        // for `function Carrier.m()`, so the same signature attaches here
        // (#38). Single-target only, matching [`visibility_carrier_member`]:
        // `a, b = f, g` has no single function to attach to.
        Stmt::Assign(assign) => {
            let target_list = assign.targets()?;
            let mut targets = target_list.exprs();
            let target = targets.next()?;
            if targets.next().is_some() {
                return None;
            }
            let Some(Expr::Function(f)) = assign.values().and_then(|v| v.exprs().next()) else {
                return None;
            };
            Some((assign_target_name(&target), f.param_list()))
        }
        _ => None,
    }
}

/// The dotted name an assignment target registers a function under —
/// `f` for `f = …`, `M.helper` for `M.helper = …`. `None` for a computed
/// (`t[k]`) or otherwise unnameable target, which attaches a signature to the
/// statement without publishing a by-name callable.
fn assign_target_name(target: &Expr) -> Option<String> {
    match target {
        Expr::Name(name) => Some(name.name()?.text().to_string()),
        Expr::Field(field) => {
            let base = assign_target_name(&field.base()?)?;
            Some(format!("{base}.{}", field.field_name()?.text()))
        }
        _ => None,
    }
}

/// Reconcile the lowered `@param` tags against the target's real AST parameter
/// list, writing the final ordered parameters (and any vararg ceiling) onto
/// `func`.
fn reconcile_params(
    func: &mut FunctionTy,
    tag_params: Vec<ParamTy>,
    param_list: Option<lua::ast::ParamList>,
    is_method: bool,
) {
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
        // A `:` method's implicit `self` is absent from the AST parameter
        // list; keep an explicit `---@param self T` tag so inference can
        // honor it (the standard-LuaCATS `self` fallback).
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
    // The *canonical* parameter list for each name — first non-empty
    // declaration wins, matching every other first-wins rule in the file.
    // Collected ahead of the templates so a *bare* `---@class Name` block that
    // only adds members is recognised as a declaration of the generic class
    // rather than of some unrelated plain one, whichever order the two appear
    // in. It fixes the merged template's parameter *names and arity*; it is
    // deliberately NOT the list a declaration's own field bodies resolve
    // against (see below).
    let mut params_of: BTreeMap<&str, &Vec<String>> = BTreeMap::new();
    for item in items {
        for tag in &item.block.tags {
            let Tag::Class(c) = tag else { continue };
            if c.name.is_empty() || c.params.is_empty() {
                continue;
            }
            params_of.entry(&c.name).or_insert(&c.params);
        }
    }
    // Every declaration of a generic name contributes its members, unioning
    // first-wins — duplicate `---@class` declarations merge here exactly as
    // they do in `absorb_block` (#49); before this the last one replaced the
    // template and the earlier declarations' fields vanished from it. The
    // file's *first* declaration still replaces an ambient generic class of
    // the same name whole (the `[types] defs` escape hatch).
    //
    // Each declaration's `---@field` bodies are lowered against the parameter
    // list *that declaration* declares — its type parameters are scoped to it,
    // exactly as luals scopes them — and the resulting templates are unified
    // *positionally* as they merge: a second declaration's slot-0 parameter is
    // the same type variable as the first's, whatever the two are spelled.
    // Handing the canonical list to every declaration instead resolved a
    // renamed duplicate's own, valid annotation against names that were never
    // in scope for it, reporting LB0305 on it.
    //
    // This pass is template *construction*, not diagnosis: the main
    // `absorb_block` walk lowers every one of these `---@field` bodies again
    // and is the sole reporter for them, so whatever this pass records is
    // rolled back below. Leaving it in would double-report a field whose type
    // name is genuinely unknown, once per pass.
    let quiet = QuietMark::of(lowerer);
    let mut claimed: HashSet<&str> = HashSet::new();
    for item in items {
        for (i, tag) in item.block.tags.iter().enumerate() {
            let Tag::Class(c) = tag else { continue };
            let Some(canonical) = params_of.get(c.name.as_str()).copied() else {
                continue;
            };
            // A bare re-declaration declares no parameters of its own, so it
            // borrows the canonical ones — there is nothing to rename.
            let own: &[String] = if c.params.is_empty() {
                canonical
            } else {
                &c.params
            };
            let mut template = lower_class_template(&item.block.tags, i, own, lowerer);
            if own != canonical.as_slice() {
                template = rename_template_params(&template, own, canonical);
            }
            if claimed.insert(&c.name) {
                out.insert(
                    c.name.clone(),
                    GenericClass {
                        params: canonical.clone(),
                        template,
                    },
                );
            } else if let Some(existing) = out.get_mut(&c.name) {
                for (name, field) in template.fields {
                    existing.template.fields.entry(name).or_insert(field);
                }
                for indexer in template.indexers {
                    if !existing.template.indexers.contains(&indexer) {
                        existing.template.indexers.push(indexer);
                    }
                }
            }
        }
    }
    quiet.rollback(lowerer);
    out
}

/// The lengths of a [`Lowerer`]'s diagnostic buffers before a lowering that is
/// performed only to *build* a type, so anything it records can be discarded.
struct QuietMark {
    unknown_names: usize,
    arity_errors: usize,
    cyclic_aliases: usize,
}

impl QuietMark {
    fn of(lowerer: &Lowerer<'_>) -> Self {
        QuietMark {
            unknown_names: lowerer.unknown_names.len(),
            arity_errors: lowerer.arity_errors.len(),
            cyclic_aliases: lowerer.cyclic_aliases.len(),
        }
    }

    fn rollback(self, lowerer: &mut Lowerer<'_>) {
        lowerer.unknown_names.truncate(self.unknown_names);
        lowerer.arity_errors.truncate(self.arity_errors);
        lowerer.cyclic_aliases.truncate(self.cyclic_aliases);
    }
}

/// The substitution that carries one declaration's type-variable names onto
/// the canonical ones, matched by **position**: `own[i]` and `canonical[i]`
/// are one type variable however the two declarations spell it.
///
/// A surplus parameter (the declaration names more than the canonical list has
/// slots for) has no canonical variable to become, so it maps to `unknown` —
/// the same leniency a bare generic reference gets for the arguments it omits,
/// rather than a dangling placeholder no instantiation could ever substitute.
fn positional_rename(own: &[String], canonical: &[String]) -> BTreeMap<String, Ty> {
    own.iter()
        .enumerate()
        .map(|(i, name)| {
            let to = canonical
                .get(i)
                .map_or(Ty::Unknown, |c| Ty::Named(c.clone()));
            (name.clone(), to)
        })
        .collect()
}

/// Rewrite one declaration's type-variable placeholders onto the canonical
/// parameter names, **positionally** — `own[i]` and `canonical[i]` name the
/// same type variable, so `---@class Boxed<U>`'s field typed `U` becomes the
/// canonical `T` before the templates merge.
///
/// A surplus parameter (the declaration names more than the canonical list
/// has slots for) has no canonical variable to become, so it collapses to
/// `unknown` — the same leniency a bare generic reference gets for the
/// arguments it omits, rather than a dangling placeholder no instantiation
/// could ever substitute.
fn rename_template_params(template: &TableTy, own: &[String], canonical: &[String]) -> TableTy {
    let map = positional_rename(own, canonical);
    match crate::generics::subst_ty(&Ty::Table(Box::new(template.clone())), &map) {
        Ty::Table(table) => *table,
        // `subst_ty` maps a `Ty::Table` to a `Ty::Table`; this arm is
        // unreachable, and falling back to the un-renamed template keeps the
        // function total without a panic.
        _ => template.clone(),
    }
}

/// Lower the `---@field`s belonging to the `---@class` at `tags[class]` into a
/// template table, with that declaration's `<T>` params in scope so each `T`
/// becomes a `Ty::Named(T)` placeholder.
///
/// Ownership is **positional**: the fields between the class's tag and the next
/// `---@class` in the block are its own (mirrors [`TypeEnv::absorb_block`]'s
/// `current_class` tracking). Matching on the class *name* instead cannot tell
/// two declarations of one name apart when both sit in a single block, and so
/// handed each of them the other's fields — harmless while both were lowered
/// against one parameter list, wrong once each has its own.
fn lower_class_template(
    tags: &[Tag],
    class: usize,
    params: &[String],
    lowerer: &mut Lowerer<'_>,
) -> TableTy {
    let saved = std::mem::replace(&mut lowerer.generics, params.iter().cloned().collect());
    let mut table = TableTy::default();
    for tag in tags.iter().skip(class + 1) {
        match tag {
            Tag::Class(_) => break,
            Tag::Field(f) => {
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

/// The scope a standalone `---@private` / `---@protected` / `---@package` tag
/// declares, or `None` for any other tag (#115).
fn standalone_scope(tag: &Tag) -> Option<FieldScope> {
    match tag {
        Tag::Private(_) => Some(FieldScope::Private),
        Tag::Protected(_) => Some(FieldScope::Protected),
        Tag::Package(_) => Some(FieldScope::Package),
        _ => None,
    }
}

/// Map every variable that carries a `---@class` to that class's name, so a
/// method attached to it (`function M:m()`) can be traced back to the class.
///
/// Two spellings reach a class, and they are **not** equal in rank:
///
/// 1. **Lexical** — the carrier variable, which need not share the class name
///    (`---@class Wrapper` over `local Animal = {}`). `function Animal:m()`
///    names that *binding*, which is what Lua itself resolves.
/// 2. **Nominal** — the class name, which is also a valid carrier reference
///    (`---@class Animal` … `function Animal:m()`).
///
/// **Lexical beats nominal**, and the two passes below are ordered to say so.
/// Filling one map with both, last-write-wins, made the answer depend on
/// declaration order: a later `---@class Animal` whose own carrier is some
/// other variable would enter the nominal alias `Animal -> Animal` on top of
/// an earlier lexical binding `Animal -> Wrapper`, and
/// [`absorb_carrier_members`] then folded `function Animal:speak()` onto the
/// wrong class — while the project-source inference path, which resolves the
/// binding, got it right. Swapping the two class blocks flipped the verdict
/// (Shockwave round 3); #39's whole point is defs/project parity.
///
/// Within the **nominal** rank the first declaration wins, matching every
/// other collision rule in the crate. Within the **lexical** rank the *last*
/// one does, because that rank is not a collision rule at all — it is a
/// binding lookup, and Lua's answer for a repeated `local M` is the most
/// recent binding. `function M:m()` after two `local M = {}` carriers names
/// the second; the project-source inference path resolves the binding and
/// says so, while this path used to answer with the first, so the two
/// disagreed on every shape with a repeated carrier variable (Shockwave
/// round 4). #39's whole point is defs/project parity, so the defs path
/// follows the binding too. This is a different axis from the rank order
/// above: lexical still beats nominal, whichever declaration order they come
/// in.
///
/// Shared by the standalone-visibility pass (#115) and the defs
/// carrier-member fold (#39).
fn carrier_var_classes(
    items: &[luacats::AnnotatedItem],
    root: &SyntaxNode,
) -> HashMap<String, String> {
    let classes = || {
        items.iter().flat_map(|item| {
            item.block.tags.iter().filter_map(move |tag| match tag {
                Tag::Class(c) if !c.name.is_empty() => Some((item, c)),
                _ => None,
            })
        })
    };

    // Rank 2 first, so rank 1 can overwrite it.
    let mut var_to_class: HashMap<String, String> = HashMap::new();
    for (_, c) in classes() {
        var_to_class
            .entry(c.name.clone())
            .or_insert_with(|| c.name.clone());
    }

    // Rank 1, collected last-wins among themselves (the binding a later
    // `function M:m()` names), then applied over the nominal aliases — one
    // write per variable, so the rank order survives whatever the declaration
    // order was.
    let mut lexical: HashMap<String, String> = HashMap::new();
    for (item, c) in classes() {
        if let Some(span) = item.target
            && let Some(name) =
                stmt_at(root, (span.start, span.end)).and_then(|s| carrier_var_name(&s))
        {
            lexical.insert(name, c.name.clone());
        }
    }
    var_to_class.extend(lexical);
    var_to_class
}

/// The variable a `---@class` carrier statement binds: the first name of a
/// `local M = {}`, or the sole `Name` target of an `M = {}` assignment.
fn carrier_var_name(stmt: &Stmt) -> Option<String> {
    match stmt {
        Stmt::Local(local) => Some(local.names().next()?.name()?.text().to_string()),
        Stmt::Assign(assign) => match assign.targets()?.exprs().next()? {
            Expr::Name(name) => Some(name.name()?.text().to_string()),
            _ => None,
        },
        _ => None,
    }
}

/// The `(carrier, member)` a standalone `---@private`/`---@protected`/
/// `---@package` tag targets (#115): either a `function Carrier.member()` /
/// `function Carrier:method()` declaration, or a single-target
/// `Carrier.member = <value>` field assignment. The assignment form mirrors
/// luals, which resolves the class from the assignment base variable
/// (`script/vm/visible.lua` `getParentClass` → `vm.getDefinedClass(uri,
/// source.node)` for `setfield`/`setindex` nodes), so `---@private` on
/// `Carrier.method = function() end` associates `method` with `Carrier`'s
/// class. Conservative: multi-target assignments (`a.x, b.y = f, g`) and
/// non-field targets stay unwired — no false `invisible`.
fn visibility_carrier_member(stmt: &Stmt) -> Option<(String, String)> {
    match stmt {
        Stmt::FunctionDecl(f) => {
            let segments: Vec<String> =
                f.name()?.segments().map(|s| s.text().to_string()).collect();
            let [carrier, .., member] = segments.as_slice() else {
                return None;
            };
            Some((carrier.clone(), member.clone()))
        }
        Stmt::Assign(assign) => {
            let target_list = assign.targets()?;
            let mut targets = target_list.exprs();
            let first = targets.next()?;
            // Single-target only — `a.x, b.y = f, g` stays unwired.
            if targets.next().is_some() {
                return None;
            }
            let Expr::Field(field) = first else {
                return None;
            };
            let Expr::Name(base) = field.base()? else {
                return None;
            };
            let carrier = base.name()?.text().to_string();
            let member = field.field_name()?.text().to_string();
            Some((carrier, member))
        }
        _ => None,
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

/// Fold a doc block's use-site tags onto the signature its `---@type fun(…)`
/// declares.
///
/// `---@deprecated`, `---@async`, `---@nodiscard` and `---@version` attach to
/// the whole block, and `fun(…)` syntax has nowhere to write them, so a block
/// carrying both an explicit `---@type` and a tag would otherwise publish the
/// declared shape with the tags stripped (#38). The declared signature stays
/// authoritative for everything it *can* express — parameters, returns,
/// overloads and generics.
pub(crate) fn merge_block_tags(mut declared: FunctionTy, block: Option<&FunctionTy>) -> FunctionTy {
    let Some(block) = block else {
        return declared;
    };
    declared.deprecated |= block.deprecated;
    declared.is_async |= block.is_async;
    declared.nodiscard |= block.nodiscard;
    if declared.version.is_none() {
        declared.version.clone_from(&block.version);
    }
    declared
}

/// A declared `---@field` that shadows a same-class carrier attachment, with
/// the attachment's use-site tags (`---@deprecated`, `---@async`,
/// `---@version`) folded in.
///
/// The `---@field` line stays authoritative for the member's type — parameters,
/// returns, overloads and generics all come from it — but its `fun(...)` syntax
/// cannot express those tags, so they only ever live on the carrier
/// (`---@deprecated` above `function C:m()`). Applied at the two points a
/// shadowed attachment is otherwise dropped — [`FileTypes::collect`], which
/// folds a file's carriers into its exported surface, and
/// [`TypeEnv::merge_file_types`], which folds that surface into the ambient —
/// so the tags survive into the *consumer* file's class shape. The same-file
/// half is `crate::infer::Inferencer::carrier_tagged`, which reads the live
/// carrier shape directly. Non-function members, and attachments carrying no
/// tags, pass the declaration through unchanged (#33).
fn with_carrier_tags(declared: &FieldTy, carrier: &FieldTy) -> FieldTy {
    let (Ty::Function(declared_sig), Ty::Function(carrier_sig)) = (&declared.ty, &carrier.ty)
    else {
        return declared.clone();
    };
    if !carrier_sig.deprecated && !carrier_sig.is_async && carrier_sig.version.is_none() {
        return declared.clone();
    }
    let mut sig = (**declared_sig).clone();
    sig.deprecated |= carrier_sig.deprecated;
    sig.is_async |= carrier_sig.is_async;
    sig.version = sig.version.or_else(|| carrier_sig.version.clone());
    FieldTy {
        ty: Ty::Function(Box::new(sig)),
        optional: declared.optional,
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
#[expect(
    clippy::string_slice,
    reason = "both slices are bounded by ASCII delimiters verified on the byte array first (quotes at [0]/[len-1], or `[==[`/`]==]` long brackets), so the indices are char boundaries within bounds"
)]
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

#[cfg(test)]
// test code — panics document assumptions
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]
mod tests {
    use luabox_syntax::lua::{Dialect, parse};

    use super::*;

    fn env_of(source: &str) -> TypeEnv {
        let parsed = parse(source, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        TypeEnv::build(&parsed)
    }

    /// The workspace-global surface one source file contributes.
    fn surface(source: &str) -> FileTypes {
        let parsed = parse(source, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        let items = luacats::harvest(&parsed);
        let env = TypeEnv::build_from_items(&parsed, &items, None);
        FileTypes::collect(&items, &env, &HashMap::new())
    }

    // --- string-literal delimiters ---------------------------------------

    #[test]
    fn unquote_strips_matching_quotes() {
        assert_eq!(unquote_lua("\"abc\""), "abc");
        assert_eq!(unquote_lua("'abc'"), "abc");
        // Escapes are kept verbatim — literal-type comparison is textual.
        assert_eq!(unquote_lua("\"a\\nb\""), "a\\nb");
        assert_eq!(unquote_lua("\"\""), "");
    }

    #[test]
    fn unquote_strips_long_brackets_at_every_level() {
        assert_eq!(unquote_lua("[[abc]]"), "abc");
        assert_eq!(unquote_lua("[=[abc]=]"), "abc");
        assert_eq!(unquote_lua("[===[abc]===]"), "abc");
        // A nested `]]` inside a level-1 long string survives.
        assert_eq!(unquote_lua("[=[a]]b]=]"), "a]]b");
        assert_eq!(unquote_lua("[[]]"), "");
    }

    #[test]
    fn unquote_leaves_unrecognized_text_alone() {
        // Mismatched or absent delimiters: return the raw text rather than
        // slicing into something that is not there.
        assert_eq!(unquote_lua("\"abc'"), "\"abc'");
        assert_eq!(unquote_lua("[=[abc]==]"), "[=[abc]==]");
        assert_eq!(unquote_lua("[["), "[[");
        assert_eq!(unquote_lua(""), "");
        assert_eq!(unquote_lua("\""), "\"");
    }

    // --- definition-layer merge order ------------------------------------

    #[test]
    fn merge_keep_first_is_first_wins() {
        // The `workspace.library` precedence: the value already present on a
        // key survives (the opposite of `BTreeMap::append`).
        let mut into: BTreeMap<String, u8> = BTreeMap::from([("a".into(), 1)]);
        merge_keep_first(
            &mut into,
            BTreeMap::from([("a".into(), 2), ("b".into(), 3)]),
        );
        assert_eq!(into, BTreeMap::from([("a".into(), 1), ("b".into(), 3)]));
    }

    #[test]
    fn merging_a_file_surface_inserts_classes_absent_from_the_base() {
        let mut env = TypeEnv::default();
        env.merge_file_types(&surface("---@class Fresh\n---@field n number\n"));
        let shape = env.class_shape("Fresh").expect("class merged in whole");
        assert_eq!(shape.fields["n"].ty, Ty::Number);
    }

    #[test]
    fn merging_a_present_class_is_member_wise_with_the_base_winning() {
        // The base (defs layer) already declares `Both`; the project file
        // re-declares it with a different `n` and adds `extra`, a parent, an
        // indexer, an operator and a private member. Base members win on a
        // collision; everything new is additive.
        let mut env = TypeEnv::default();
        env.merge_file_types(&surface(
            "\
---@class Parent
---@class Both
---@field n number
",
        ));
        env.merge_file_types(&surface(
            "\
---@class Other
---@class Both : Parent, Other
---@field n string
---@field extra boolean
---@field [integer] string
---@field private hidden number
---@operator add(Both): Both
",
        ));
        let def = env.classes.get("Both").expect("merged class");
        // Base's `n` survives; the second declaration's `extra` is added.
        assert_eq!(def.fields["n"].ty, Ty::Number);
        assert_eq!(def.fields["extra"].ty, Ty::Boolean);
        assert_eq!(def.parents, vec!["Parent".to_string(), "Other".to_string()]);
        assert_eq!(def.indexers, vec![(Ty::Integer, Ty::String)]);
        assert_eq!(def.operators["add"].len(), 1);
        assert_eq!(def.visibility.get("hidden"), Some(&FieldScope::Private));
    }

    #[test]
    fn merging_repeated_declarations_never_duplicates_parents_or_operators() {
        let file = surface(
            "\
---@class Base
---@class Dup : Base
---@field [integer] string
---@operator add(Dup): Dup
",
        );
        let mut env = TypeEnv::default();
        env.merge_file_types(&file);
        env.merge_file_types(&file);
        let def = env.classes.get("Dup").expect("merged class");
        assert_eq!(def.parents, vec!["Base".to_string()]);
        assert_eq!(def.indexers.len(), 1);
        assert_eq!(def.operators["add"].len(), 1);
    }

    #[test]
    fn merging_never_lets_a_carrier_attachment_shadow_a_declared_field() {
        // `method` is a real `---@field` on the base and only a carrier
        // attachment on the incoming file: the declared field must win.
        let mut env = TypeEnv::default();
        env.merge_file_types(&surface(
            "\
---@class Shadow
---@field method string
",
        ));
        let mut incoming = FileTypes::default();
        incoming.classes.insert(
            "Shadow".to_string(),
            ClassDef {
                methods: BTreeMap::from([(
                    "method".to_string(),
                    FieldTy {
                        ty: Ty::Boolean,
                        optional: false,
                    },
                )]),
                ..ClassDef::default()
            },
        );
        env.merge_file_types(&incoming);
        let def = env.classes.get("Shadow").expect("merged class");
        assert_eq!(def.fields["method"].ty, Ty::String);
        assert!(!def.methods.contains_key("method"));
    }

    #[test]
    fn merging_adds_carrier_attachments_the_base_does_not_declare() {
        let mut env = TypeEnv::default();
        env.merge_file_types(&surface(
            "\
---@class Attach
---@field declared string
",
        ));
        let mut incoming = FileTypes::default();
        incoming.classes.insert(
            "Attach".to_string(),
            ClassDef {
                methods: BTreeMap::from([(
                    "extra".to_string(),
                    FieldTy {
                        ty: Ty::Boolean,
                        optional: false,
                    },
                )]),
                ..ClassDef::default()
            },
        );
        env.merge_file_types(&incoming);
        let def = env.classes.get("Attach").expect("merged class");
        assert_eq!(def.methods["extra"].ty, Ty::Boolean);
        // Attachments resolve on reads, folded in beside the declared field.
        let shape = env.class_shape("Attach").expect("shape");
        assert_eq!(shape.fields["declared"].ty, Ty::String);
        assert_eq!(shape.fields["extra"].ty, Ty::Boolean);
    }

    #[test]
    fn merging_enums_is_first_wins() {
        let mut env = TypeEnv::default();
        env.merge_file_types(&surface(
            "\
---@enum Color
local Color = { red = 1 }
",
        ));
        env.merge_file_types(&surface(
            "\
---@enum Color
local Color = { blue = 2 }
",
        ));
        let members = env.enum_members("Color").expect("enum merged");
        assert_eq!(
            members,
            vec![("red".to_string(), Ty::NumberLit("1".into()))]
        );
    }

    #[test]
    fn a_file_surface_carries_its_enum_definitions() {
        let types = surface(
            "\
---@enum Status
local Status = { ok = 1, err = 2 }
return Status
",
        );
        let def = types.enums.get("Status").expect("enum on the surface");
        assert_eq!(
            def.members.keys().collect::<Vec<_>>(),
            vec!["err", "ok"] // BTreeMap order
        );
        assert!(!types.is_empty());
    }

    // --- `---@enum` table shapes -----------------------------------------

    #[test]
    fn key_mode_enum_takes_its_member_names_as_values() {
        // `---@enum (key)`: the enum's values are its *keys*, whatever the
        // table's values are.
        let env = env_of(
            "\
---@enum (key) Mode
local Mode = { fast = 1, slow = \"two\" }
",
        );
        let members = env.enum_members("Mode").expect("enum declared");
        assert_eq!(
            members,
            vec![
                ("fast".to_string(), Ty::StringLit("fast".into())),
                ("slow".to_string(), Ty::StringLit("slow".into())),
            ]
        );
    }

    #[test]
    fn enum_members_with_non_literal_values_stay_unknown() {
        let env = env_of(
            "\
---@enum Computed
local Computed = { a = 1, b = some_call(), c = { nested = true } }
",
        );
        let members = env.enum_members("Computed").expect("enum declared");
        assert_eq!(
            members,
            vec![
                ("a".to_string(), Ty::NumberLit("1".into())),
                ("b".to_string(), Ty::Unknown),
                ("c".to_string(), Ty::Unknown),
            ]
        );
    }

    #[test]
    fn enum_on_a_non_table_target_has_no_members() {
        // `---@enum` must annotate a table constructor; anything else yields
        // an empty (but declared) enum rather than a panic or a guess.
        let env = env_of(
            "\
---@enum Empty
local Empty = other_table
",
        );
        assert_eq!(env.enum_members("Empty"), Some(Vec::new()));
    }

    #[test]
    fn boolean_and_nil_enum_members_lower_to_literal_types() {
        let env = env_of(
            "\
---@enum Flags
local Flags = { on = true, off = false, none = nil }
",
        );
        let members = env.enum_members("Flags").expect("enum declared");
        assert_eq!(
            members,
            vec![
                ("none".to_string(), Ty::Nil),
                ("off".to_string(), Ty::BoolLit(false)),
                ("on".to_string(), Ty::BoolLit(true)),
            ]
        );
    }

    // --- carriers & standalone visibility --------------------------------

    #[test]
    fn a_global_assignment_carrier_maps_back_to_its_class() {
        // The carrier variable need not share the class name, and `Zoo = {}`
        // (a global assignment, not a `local`) is a carrier just the same: the
        // standalone `---@private` on `Zoo.hide` has to land on `Animal`.
        let env = env_of(
            "\
---@class Animal
Zoo = {}
---@private
function Zoo.hide() end
",
        );
        let def = env.classes.get("Animal").expect("class declared");
        assert_eq!(def.visibility.get("hide"), Some(&FieldScope::Private));
    }

    /// Shockwave round 3: a later `---@class` whose NAME equals an earlier
    /// class's carrier VARIABLE used to clobber the lexical binding, so
    /// `function Animal:speak()` folded onto class `Animal` instead of onto
    /// `Wrapper` — the class the local `Animal` actually carries. Swapping the
    /// two class blocks flipped the verdict; both orders must now agree.
    #[test]
    fn a_carrier_variable_binding_beats_a_same_named_class() {
        let wrapper_first = "\
---@class Wrapper
local Animal = {}

---@class Animal
local Zoo = {}

---@private
function Animal:speak() end
";
        let animal_first = "\
---@class Animal
local Zoo = {}

---@class Wrapper
local Animal = {}

---@private
function Animal:speak() end
";
        for (label, source) in [
            ("Wrapper first", wrapper_first),
            ("Animal first", animal_first),
        ] {
            let env = env_of(source);
            let wrapper = env.classes.get("Wrapper").expect("Wrapper declared");
            let animal = env.classes.get("Animal").expect("Animal declared");
            assert_eq!(
                wrapper.visibility.get("speak"),
                Some(&FieldScope::Private),
                "{label}: `speak` belongs to the class the local `Animal` carries"
            );
            assert!(
                animal.visibility.is_empty(),
                "{label}: nothing attaches to the same-named class: {:?}",
                animal.visibility
            );
        }
    }

    /// The nominal spelling still works when nothing shadows it: a class whose
    /// own name is used as the carrier reference.
    #[test]
    fn a_class_name_is_still_a_carrier_reference_when_unshadowed() {
        let env = env_of(
            "\
---@class Solo
local Solo = {}
---@private
function Solo:hide() end
",
        );
        let def = env.classes.get("Solo").expect("class declared");
        assert_eq!(def.visibility.get("hide"), Some(&FieldScope::Private));
    }

    /// Two carriers with the same variable name: the **last** one wins,
    /// because `function M:only()` names the binding in scope and Lua's
    /// answer for a repeated `local M` is the most recent binding. This is
    /// not a collision rule — it is a lookup, and the project-source
    /// inference path resolves it that way (Shockwave round 4).
    #[test]
    fn the_last_carrier_wins_a_repeated_variable_name() {
        let env = env_of(
            "\
---@class First
local M = {}

---@class Second
local M = {}

---@private
function M:only() end
",
        );
        let first = env.classes.get("First").expect("First declared");
        let second = env.classes.get("Second").expect("Second declared");
        assert_eq!(second.visibility.get("only"), Some(&FieldScope::Private));
        assert!(first.visibility.is_empty(), "{:?}", first.visibility);
    }

    /// The three shapes Shockwave measured against `lua5.4` and against the
    /// project-source path. Each names two carriers `M`; the member belongs
    /// to whichever class the *second* `M` carries, whatever the class names
    /// and whichever carrier form is used.
    #[test]
    fn a_repeated_carrier_variable_follows_the_binding_in_every_shape() {
        let shapes = [
            (
                "two local carriers",
                "\
---@class Alpha
local M = {}

---@class Beta
local M = {}

---@private
function M:only() end
",
                "Beta",
                "Alpha",
            ),
            (
                "the same two, declared the other way round",
                "\
---@class Beta
local M = {}

---@class Alpha
local M = {}

---@private
function M:only() end
",
                "Alpha",
                "Beta",
            ),
            (
                "a local carrier then an assigned one",
                "\
---@class Alpha
local M = {}

---@class Beta
M = {}

---@private
function M:only() end
",
                "Beta",
                "Alpha",
            ),
        ];
        for (label, source, winner, loser) in shapes {
            let env = env_of(source);
            let winner_def = env.classes.get(winner).expect("class declared");
            let loser_def = env.classes.get(loser).expect("class declared");
            assert_eq!(
                winner_def.visibility.get("only"),
                Some(&FieldScope::Private),
                "{label}: `only` belongs to `{winner}`"
            );
            assert!(
                loser_def.visibility.is_empty(),
                "{label}: nothing attaches to `{loser}`: {:?}",
                loser_def.visibility
            );
        }
    }

    #[test]
    fn a_non_name_assignment_target_is_not_a_carrier() {
        // `outer.inner = {}` names no single carrier variable, so the class is
        // declared but no variable maps to it and the standalone tag is
        // dropped rather than misattributed.
        let env = env_of(
            "\
local outer = {}
---@class Nested
outer.inner = {}
---@private
function outer.hidden() end
",
        );
        let def = env.classes.get("Nested").expect("class declared");
        assert!(def.visibility.is_empty(), "{:?}", def.visibility);
    }

    #[test]
    fn standalone_visibility_tags_bind_to_dotted_and_assigned_members() {
        let env = env_of(
            "\
---@class Vis
local Vis = {}
---@private
function Vis.hidden() end
---@protected
function Vis:guarded() end
---@package
Vis.internal = 1
function Vis.open() end
",
        );
        let def = env.classes.get("Vis").expect("class declared");
        assert_eq!(def.visibility.get("hidden"), Some(&FieldScope::Private));
        assert_eq!(def.visibility.get("guarded"), Some(&FieldScope::Protected));
        assert_eq!(def.visibility.get("internal"), Some(&FieldScope::Package));
        assert_eq!(def.visibility.get("open"), None);
    }

    #[test]
    fn visibility_tags_on_unaddressable_targets_are_dropped() {
        // Each of these has no `(carrier, member)` pair to bind to, so the
        // tag is discarded rather than misattributed.
        let env = env_of(
            "\
---@class Drop
local Drop = {}
---@private
function bare() end
---@private
Drop = {}
---@private
local shadow = 1
---@private
Drop[1] = 2
",
        );
        let def = env.classes.get("Drop").expect("class declared");
        assert!(def.visibility.is_empty(), "{:?}", def.visibility);
    }

    #[test]
    fn a_multi_target_assignment_never_carries_a_visibility_tag() {
        // `a.x, b.y = ...` is deliberately left unwired — the tag would be
        // ambiguous, so nothing is recorded rather than the wrong thing.
        let env = env_of(
            "\
---@class Multi
local Multi = {}
---@private
Multi.a, Multi.b = 1, 2
",
        );
        let def = env.classes.get("Multi").expect("class declared");
        assert!(def.visibility.is_empty(), "{:?}", def.visibility);
    }

    // --- signature reconciliation ----------------------------------------

    #[test]
    fn an_explicit_param_self_survives_reconciliation() {
        // A `:` method's `self` is absent from the AST parameter list, so the
        // explicit `---@param self T` tag has to be re-inserted at position 0
        // rather than dropped as "unmatched".
        let env = env_of(
            "\
---@class Recv
local Recv = {}
---@param self Recv
---@param n number
function Recv:m(n) end
",
        );
        // `:` methods are range-keyed, not registered by dotted name.
        let sig = env
            .fn_sigs
            .values()
            .find(|f| f.params.iter().any(|p| p.name == "n"))
            .expect("method signature");
        assert_eq!(sig.params.len(), 2);
        assert_eq!(sig.params[0].name, "self");
        assert_eq!(sig.params[0].ty, Ty::Named("Recv".into()));
        assert_eq!(sig.params[1].name, "n");
        assert_eq!(sig.params[1].ty, Ty::Number);
    }

    #[test]
    fn an_ast_vararg_without_a_tag_becomes_unknown_varargs() {
        let env = env_of(
            "\
---@param a number
function spread(a, ...) end
",
        );
        let sig = env.function("spread").expect("signature");
        assert_eq!(sig.varargs, Some(Ty::Unknown));
    }

    #[test]
    fn an_undeclared_ast_parameter_is_optional_unknown() {
        // A parameter with no `---@param` tag must not become a required
        // `unknown` slot — that would reject every call.
        let env = env_of(
            "\
---@param a number
function partial(a, b) end
",
        );
        let sig = env.function("partial").expect("signature");
        assert_eq!(sig.params[1].name, "b");
        assert_eq!(sig.params[1].ty, Ty::Unknown);
        assert!(sig.params[1].optional);
    }
}
