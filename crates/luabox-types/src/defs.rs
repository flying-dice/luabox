//! Definition packages — ambient `---@meta` `.d.lua` type surfaces
//! (SPEC.md §3).
//!
//! Each supported dialect ships a set of `.d.lua` files (under
//! `assets/defs/<dialect>/`) describing its real stdlib: basic globals plus
//! the `string`, `table`, `math`, `io`, `os`, `coroutine`, `debug` modules
//! and version-specific ones (`utf8` on 5.3+, `bit32` on 5.2, `bit`/`jit`
//! on LuaJIT). They are embedded into the binary with [`include_str!`],
//! parsed and lowered **once per dialect** (cached in a [`OnceLock`]), and
//! merged *beneath* per-file annotations by [`crate::TypeEnv`].
//!
//! Project-local packages named in `[types] defs` (SPEC.md §5) layer on top
//! of the stdlib set: the frontend resolves them to sources and calls
//! [`build_ambient`]. Registry-distributed defs are P2+.

use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;

use luabox_diag::{Diagnostic, Label, Span};
use luabox_hir::{Expr, ExprId, HirId, Resolution, Stmt};
use luabox_syntax::lua::{self, Dialect};
use luabox_syntax::luacats::{self, AliasTag, AnnotatedItem, Tag};

use crate::codes::{ALIAS_COLLISION, CLASS_COLLISION};
use crate::env::TypeEnv;

/// A definition-package source paired with the file it was read from — the
/// unit cross-package collision reporting (`LB0307`, #108) attributes to.
#[derive(Debug, Clone)]
pub struct DefFile {
    /// A display label for the declaring file (e.g. `defs/geometry.d.lua`, or
    /// `<dep>/defs/geometry.d.lua` for a dependency's def) — the name a
    /// collision diagnostic prints.
    pub file: String,
    /// The `.d.lua` source text.
    pub text: String,
}

/// A parsed, lowered definition-package layer: the ambient environment plus
/// the `---@alias`es it declares (so a consuming file's lowerer can expand
/// them). Passed by reference to [`crate::check_file_with_ambient`].
#[derive(Debug)]
pub struct Ambient {
    pub(crate) env: TypeEnv,
    pub(crate) aliases: BTreeMap<String, AliasTag>,
    /// Every top-level global name this definition package declares (module
    /// tables like `math`, scalar globals like `_VERSION`, and bare
    /// functions like `print`) — the `undefined-global` lint's read-only
    /// name-enumeration surface (ticket #103). `TypeEnv` only exposes
    /// by-name lookups (`function`/`global_type`), not enumeration, so this
    /// is harvested independently by lowering each source through
    /// `luabox-hir` and collecting every assignment target that resolves to
    /// a bare global (dotted targets like `function math.abs() end` count
    /// their *first* segment, `math`).
    global_names: HashSet<String>,
}

impl Ambient {
    /// Build an ambient layer from a set of `.d.lua` source strings.
    #[must_use]
    pub(crate) fn build(sources: &[&str]) -> Ambient {
        let files: Vec<(lua::Parse, Vec<AnnotatedItem>)> = sources
            .iter()
            .map(|src| {
                // Definition files use only the common syntax; parse them
                // with the richest dialect so nothing is rejected.
                let parse = lua::parse(src, Dialect::Lua54);
                let items = luacats::harvest(&parse);
                (parse, items)
            })
            .collect();
        let (env, aliases) = TypeEnv::build_ambient(&files);
        let mut global_names = HashSet::new();
        for (parse, _) in &files {
            global_names.extend(declared_global_names(parse));
        }
        Ambient {
            env,
            aliases,
            global_names,
        }
    }

    /// Every top-level global name this ambient layer declares — the
    /// `undefined-global` lint's known-globals surface (ticket #103).
    #[must_use]
    pub fn global_names(&self) -> &HashSet<String> {
        &self.global_names
    }

    /// The merged member surface of a class this layer declares: parents
    /// folded in depth-first, carrier attachments and `---@field`
    /// declarations unioned — exactly the shape the checker resolves a
    /// member access against. `None` for a name that is not a class here.
    ///
    /// This is the editor surfaces' read path (#56): hover and completion
    /// on a class-typed value resolve members through the same ambient
    /// environment the type pass holds (build the layer with
    /// [`Self::with_project_types`] / [`Self::with_rock_types`] first, as
    /// the diagnostics pipeline does), so what the editor offers is what
    /// `luabox check` enforces — one environment, not a parallel view.
    #[must_use]
    pub fn class_members(&self, name: &str) -> Option<crate::ty::TableTy> {
        self.env.class_shape(name)
    }

    /// [`Self::class_members`] with `name`'s own type parameters bound to
    /// `args`, positionally — the shape a reference that wrote them means
    /// (round 3 review F48). `---@type Box<number>` binds `Box`'s `T` to
    /// `number`; `args` empty is exactly [`Self::class_members`] (every
    /// parameter free, matching a bare `Box`). See
    /// [`crate::env::TypeEnv::class_shape_bound`] for the substitution rule
    /// this delegates to — the same one every other consumer of a generic
    /// class's shape already goes through.
    // `pub(crate)`, not `pub` (round 4 review R16): no consumer outside this
    // module or its tests calls it directly — every external caller
    // (`luabox-lsp`, verified: it only ever reaches this through
    // `Self::class_members_of`) goes through `class_members_of` below, which
    // is the crate's real reference-site API for a bound generic lookup.
    #[must_use]
    pub(crate) fn class_members_bound(
        &self,
        name: &str,
        args: &[crate::ty::Ty],
    ) -> Option<crate::ty::TableTy> {
        self.env.class_shape_bound(name, args)
    }

    /// [`crate::env::TypeEnv::collect_class`]'s per-key winner for every
    /// field of `name`'s merged shape (#70): member name → the class whose
    /// own declaration claimed it, plus that declaration's site when a
    /// project file's `---@field` wrote it.
    ///
    /// The declaration-site half of what [`Self::class_members`] already
    /// answers for types. An editor surface that must point at *where* a
    /// member is declared — hover's description, goto-definition's target —
    /// otherwise has to re-derive the merge from the same annotations and
    /// hope its walk agrees with the checker's; on a diamond conflict and on
    /// a class whose `---@field`s are split across files, it does not
    /// ([`crate::FieldOrigin`] spells out both shapes). This is the merge's
    /// own answer, so there is nothing for the two to disagree about.
    ///
    /// `None` for a name this layer does not declare as a class, matching
    /// [`Self::class_members`]. A field with no recorded site is present with
    /// [`FieldOrigin::site`] `None` rather than absent — see
    /// [`crate::FieldDeclSite`] for which contributions have no site to record.
    #[must_use]
    pub fn class_field_origins(
        &self,
        name: &str,
    ) -> Option<std::collections::BTreeMap<String, crate::FieldOrigin>> {
        self.env.class_field_origins(name)
    }

    /// Whether `name`'s ancestor chain was too deep for the most recent
    /// [`Self::class_members`]/[`Self::class_members_bound`]/
    /// [`Self::class_members_of`] call to resolve fully (`LB0317`) — a class
    /// this layer declares but whose shape above the depth cap is missing
    /// from what those calls returned.
    ///
    /// This layer's `env` is long-lived (one per workspace revision, cached
    /// behind `luabox-lsp`'s `MergedAmbient`), unlike the per-file `TypeEnv`
    /// `crate::check::run` builds and drains on every check — nothing else
    /// ever drains this one, so every hit since the layer was built stays
    /// visible here for as long as the layer lives, not just the one that
    /// just happened. Exposed so an LSP surface reading a class's members
    /// through this layer (hover, completion, goto-definition, signature
    /// help — none of which go through `check::run`) can tell the user their
    /// class shape is incomplete instead of silently showing fewer members
    /// than the class declares while the Problems panel, checking the same
    /// file through its own `TypeEnv`, already says so (production readiness
    /// review, finding 2 — this module's own doc above promises "one
    /// environment, not a parallel view").
    #[must_use]
    pub fn class_ancestry_truncated(&self, name: &str) -> bool {
        self.env.class_ancestry_truncated(name)
    }

    /// [`Self::class_members_bound`] from a LuaCATS type expression directly
    /// — the reference-site half of round 3 review F48 (#56's acceptance
    /// criterion: "no divergence between what the editor accepts and what CI
    /// rejects"). `Self::class_members(name)` alone cannot answer this
    /// correctly for a generic reference: extracting just the name and
    /// asking for its (necessarily unbound) shape is exactly the shape of
    /// the divergence this closes — hover/completion answering `T` where the
    /// checker, which resolves the *whole* reference, answers `number`.
    ///
    /// `ty` is expected to be a `Named` reference (`Box`, `Box<number>`,
    /// `mylib.Point`); anything else (`T?`, `A|B`, an array, ...) returns
    /// `None` rather than guessing which member of a compound type the
    /// caller meant — unwrap to the `Named` case yourself first if that is
    /// what you have. `None` also covers a `Named` reference whose name is
    /// not a class at all (an alias, an enum, an unresolvable name).
    ///
    /// The arguments are lowered against this ambient layer's own declared
    /// names — the workspace scope every project file's own annotations
    /// already resolve against — so a class name, alias, or nested generic
    /// reference used as an argument (`Box<Pair<number>>`) resolves exactly
    /// as it would inside a checked file. No `---@generic` scope applies: an
    /// argument names a concrete type, not a template placeholder.
    #[must_use]
    pub fn class_members_of(&self, ty: &luacats::TypeExpr) -> Option<crate::ty::TableTy> {
        let luacats::TypeExprKind::Named { name, args } = &ty.kind else {
            return None;
        };
        let args = self.env.lower_bound_args(args, &self.aliases);
        self.class_members_bound(name, &args)
    }

    /// A new ambient layer: this one plus the workspace-global
    /// `---@class`/`---@enum`/`---@alias` declarations collected from every
    /// checked project source file (luals parity: classes, enums, and
    /// aliases are workspace-global, so a name declared in any project file —
    /// including a class's `function Class:method` member attachments — is
    /// nameable and resolvable from every other file).
    ///
    /// Merge semantics (luals merges duplicate class declarations' fields):
    /// a class this layer does not declare is inserted whole; a class both
    /// declare merges member-wise — parents union, `---@field`s and
    /// attachments union, with this layer (defs — the published surface,
    /// #108 winner-first) winning any same-name member collision. A member
    /// attached by a carrier never shadows a `---@field` declaration.
    /// Enums and aliases merge first-wins: this layer's defs win a name
    /// collision, and among project files the first to declare a name wins
    /// (deterministic — `files` is iterated in the caller's stable file
    /// order). luals treats a duplicate alias definition as a warning and
    /// keeps one; luabox keeps the first deterministically, mirroring the
    /// enum rule (#110). Global names are untouched, so the
    /// `undefined-global` lint baseline is unchanged.
    ///
    /// Project aliases are carried into the new layer's alias map raw and
    /// expanded lazily by each consuming file's lowerer (see
    /// [`crate::env::FileTypes`]), so a cross-file alias body referencing
    /// another workspace-global alias or class resolves at the use site and a
    /// cyclic alias terminates via the lowerer's cycle guard.
    #[must_use]
    pub fn with_project_types<'a>(
        &self,
        files: impl IntoIterator<Item = &'a crate::env::FileTypes>,
    ) -> Ambient {
        let mut env = self.env.clone_surface();
        let mut aliases = self.aliases.clone();
        for file in files {
            env.merge_file_types(file);
            for (name, alias) in file.aliases() {
                // Defs (already seeded) win; among project files the first
                // declarer of a name wins — first-wins, mirroring enums.
                aliases.entry(name.clone()).or_insert_with(|| alias.clone());
            }
        }
        Ambient {
            env,
            aliases,
            global_names: self.global_names.clone(),
        }
    }

    /// A new ambient layer: this one plus the type surfaces harvested from a
    /// vendored luarocks tree (#30, [`crate::rocks::harvest`]).
    ///
    /// Consumes `self` so a caller that has just built the project-wide layer
    /// ([`Self::with_project_types`]) extends it in place rather than cloning
    /// the whole surface a second time; a project with no rock tree passes an
    /// empty iterator and pays nothing.
    ///
    /// **Explicit beats implicit.** Where [`Self::with_project_types`] unions a
    /// class's members across declarations (luals parity — two declarations in
    /// code you wrote are one intent), a harvested rock surface only fills names
    /// nothing else has claimed: a class, enum or alias already declared by the
    /// stdlib, by `[types] defs`, or by any project file is left exactly as it
    /// is. That is what makes `[types] defs` a real escape hatch — your
    /// declaration replaces the rock's rather than merging with it — and it is
    /// why this must be applied *after* [`Self::with_project_types`].
    #[must_use]
    pub fn with_rock_types<'a>(
        mut self,
        rocks: impl IntoIterator<Item = &'a crate::env::FileTypes>,
    ) -> Ambient {
        for types in rocks {
            self.env.insert_unclaimed_types(types);
            for (name, alias) in types.aliases() {
                self.aliases
                    .entry(name.clone())
                    .or_insert_with(|| alias.clone());
            }
        }
        self
    }

    /// Undeclared type names referenced by the definition files themselves —
    /// a self-consistency check for the shipped packages (should be empty).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn unknown_names(&self) -> &[(String, luacats::Span)] {
        &self.env.unknown_names
    }

    /// Whether the ambient layer declares a callable of this (possibly
    /// dotted) name — the test/introspection surface.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn has_function(&self, name: &str) -> bool {
        self.env.function(name).is_some()
    }

    /// Whether the ambient layer binds this global value (module table or
    /// scalar).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn has_global(&self, name: &str) -> bool {
        self.env.global_type(name).is_some()
    }
}

/// Every top-level global name a `.d.lua` source declares: the base name of
/// every assignment target that resolves to a global, anywhere in the file
/// (mirroring `luabox-lint`'s `global-write` detection). A dotted target —
/// `function math.abs(x) end` desugars to an assignment to `math.abs` —
/// contributes only its first segment (`math`), the name that must already
/// exist as a global for the declaration to make sense.
fn declared_global_names(parse: &lua::Parse) -> HashSet<String> {
    let lowered = luabox_hir::lower(parse);
    let mut names = HashSet::new();
    for (body_id, body) in lowered.bodies() {
        for (_, stmt) in body.stmts() {
            let Stmt::Assign { targets, .. } = stmt else {
                continue;
            };
            for &target in targets {
                let base = base_expr(body, target);
                let hir = HirId::expr(body_id, base);
                if let Some(Resolution::Global(name)) = lowered.resolution(hir) {
                    names.insert(name.clone());
                }
            }
        }
    }
    names
}

/// Walk an assignment target's `Expr::Index` chain (`a.b.c` desugars to
/// nested `Index` nodes) down to its innermost base expression.
fn base_expr(body: &luabox_hir::Body, mut id: ExprId) -> ExprId {
    while let Expr::Index { base, .. } = body.expr(id) {
        id = *base;
    }
    id
}

/// The embedded `.d.lua` sources for one dialect, in a stable order.
fn sources(dialect: Dialect) -> &'static [&'static str] {
    match dialect {
        Dialect::Lua51 => &[
            include_str!("../../../assets/defs/lua51/basic.d.lua"),
            include_str!("../../../assets/defs/lua51/string.d.lua"),
            include_str!("../../../assets/defs/lua51/table.d.lua"),
            include_str!("../../../assets/defs/lua51/math.d.lua"),
            include_str!("../../../assets/defs/lua51/io.d.lua"),
            include_str!("../../../assets/defs/lua51/os.d.lua"),
            include_str!("../../../assets/defs/lua51/coroutine.d.lua"),
            include_str!("../../../assets/defs/lua51/debug.d.lua"),
            include_str!("../../../assets/defs/lua51/package.d.lua"),
        ],
        Dialect::Lua52 => &[
            include_str!("../../../assets/defs/lua52/basic.d.lua"),
            include_str!("../../../assets/defs/lua52/string.d.lua"),
            include_str!("../../../assets/defs/lua52/table.d.lua"),
            include_str!("../../../assets/defs/lua52/math.d.lua"),
            include_str!("../../../assets/defs/lua52/io.d.lua"),
            include_str!("../../../assets/defs/lua52/os.d.lua"),
            include_str!("../../../assets/defs/lua52/coroutine.d.lua"),
            include_str!("../../../assets/defs/lua52/debug.d.lua"),
            include_str!("../../../assets/defs/lua52/bit32.d.lua"),
            include_str!("../../../assets/defs/lua52/package.d.lua"),
        ],
        Dialect::Lua53 => &[
            include_str!("../../../assets/defs/lua53/basic.d.lua"),
            include_str!("../../../assets/defs/lua53/string.d.lua"),
            include_str!("../../../assets/defs/lua53/table.d.lua"),
            include_str!("../../../assets/defs/lua53/math.d.lua"),
            include_str!("../../../assets/defs/lua53/io.d.lua"),
            include_str!("../../../assets/defs/lua53/os.d.lua"),
            include_str!("../../../assets/defs/lua53/coroutine.d.lua"),
            include_str!("../../../assets/defs/lua53/debug.d.lua"),
            include_str!("../../../assets/defs/lua53/utf8.d.lua"),
            include_str!("../../../assets/defs/lua53/package.d.lua"),
        ],
        Dialect::Lua54 => &[
            include_str!("../../../assets/defs/lua54/basic.d.lua"),
            include_str!("../../../assets/defs/lua54/string.d.lua"),
            include_str!("../../../assets/defs/lua54/table.d.lua"),
            include_str!("../../../assets/defs/lua54/math.d.lua"),
            include_str!("../../../assets/defs/lua54/io.d.lua"),
            include_str!("../../../assets/defs/lua54/os.d.lua"),
            include_str!("../../../assets/defs/lua54/coroutine.d.lua"),
            include_str!("../../../assets/defs/lua54/debug.d.lua"),
            include_str!("../../../assets/defs/lua54/utf8.d.lua"),
            include_str!("../../../assets/defs/lua54/package.d.lua"),
        ],
        Dialect::LuaJit => &[
            include_str!("../../../assets/defs/luajit/basic.d.lua"),
            include_str!("../../../assets/defs/luajit/string.d.lua"),
            include_str!("../../../assets/defs/luajit/table.d.lua"),
            include_str!("../../../assets/defs/luajit/math.d.lua"),
            include_str!("../../../assets/defs/luajit/io.d.lua"),
            include_str!("../../../assets/defs/luajit/os.d.lua"),
            include_str!("../../../assets/defs/luajit/coroutine.d.lua"),
            include_str!("../../../assets/defs/luajit/debug.d.lua"),
            include_str!("../../../assets/defs/luajit/package.d.lua"),
            include_str!("../../../assets/defs/luajit/bit.d.lua"),
            include_str!("../../../assets/defs/luajit/jit.d.lua"),
        ],
    }
}

/// The stdlib ambient layer for a dialect, built once and cached for the
/// process lifetime (definition files never change at runtime — the perf
/// gate depends on this being paid only once).
#[must_use]
pub fn stdlib(dialect: Dialect) -> &'static Ambient {
    // One slot per `Dialect` variant, in `Dialect::ALL` order.
    static CACHE: [OnceLock<Ambient>; Dialect::ALL.len()] =
        [const { OnceLock::new() }; Dialect::ALL.len()];
    let index = match dialect {
        Dialect::Lua51 => 0,
        Dialect::Lua52 => 1,
        Dialect::Lua53 => 2,
        Dialect::Lua54 => 3,
        Dialect::LuaJit => 4,
    };
    CACHE[index].get_or_init(|| Ambient::build(sources(dialect)))
}

/// Build an ambient layer combining the dialect stdlib with extra
/// project-local definition sources (`[types] defs`). Not cached — the extra
/// sources vary per project; the stdlib portion is small and shared by
/// value here.
#[must_use]
pub fn build_ambient(dialect: Dialect, extra: &[String]) -> Ambient {
    if extra.is_empty() {
        // Common case: reuse the cached stdlib set by cloning nothing —
        // callers hold `&Ambient`, so hand back a fresh build only when
        // extra defs exist. Here we still must return owned; clone-free
        // path is `stdlib` (used directly by the frontend).
        return Ambient::build(sources(dialect));
    }
    let mut all: Vec<&str> = sources(dialect).to_vec();
    for src in extra {
        all.push(src.as_str());
    }
    Ambient::build(&all)
}

/// Build the ambient layer combining the dialect stdlib with attributed
/// project-local and dependency definition files (`[types] defs`), and report
/// cross-package `---@class` name collisions (`LB0307`, #108).
///
/// This is the manifest-native form of luals's `workspace.library` model: a
/// dependency's own `[types] defs` files join the consumer's ambient scope, so
/// their `---@class` declarations become referenceable and checkable across
/// the package boundary. Class names are a single global namespace (as in
/// luals), but where luals silently merges duplicate declarations, luabox
/// emits a warning at every declaration after the first.
///
/// **Order is precedence.** Callers pass `defs` in winner-first order —
/// project-local defs first (the consumer wins), then each direct dependency
/// alphabetically. The first source to declare a class name wins: its fields
/// are the ones [`TypeEnv`] resolves, and every later declaration of that name
/// yields an `LB0307` warning naming the file that already declared it.
#[must_use]
pub fn build_ambient_checked(dialect: Dialect, defs: &[DefFile]) -> (Ambient, Vec<Diagnostic>) {
    let mut all: Vec<&str> = sources(dialect).to_vec();
    for def in defs {
        all.push(def.text.as_str());
    }
    let ambient = Ambient::build(&all);
    (ambient, class_collisions(defs))
}

/// Detect `---@class` names declared by more than one `.d.lua` in `defs`,
/// producing an `LB0307` warning at each declaration after the first. The
/// first declarer (earliest in `defs`, which is winner-first order) wins and
/// is never reported; each later duplicate points at the file that already
/// owns the name. Only cross-*file* duplicates are reported — a file that
/// declares a class once (the norm) and the trusted stdlib layer are never
/// involved.
fn class_collisions(defs: &[DefFile]) -> Vec<Diagnostic> {
    let mut owner: BTreeMap<String, String> = BTreeMap::new();
    let mut diags = Vec::new();
    for def in defs {
        let parse = lua::parse(&def.text, Dialect::Lua54);
        let items = luacats::harvest(&parse);
        // A class name may only be *claimed* once per file even if the same
        // file repeats it; dedup within-file so an intra-file repeat does not
        // masquerade as a cross-package collision.
        let mut claimed_here: HashSet<String> = HashSet::new();
        for item in &items {
            for tag in &item.block.tags {
                let Tag::Class(class) = tag else { continue };
                if class.name.is_empty() || !claimed_here.insert(class.name.clone()) {
                    continue;
                }
                if let Some(first) = owner.get(&class.name) {
                    diags.push(
                        Diagnostic::warning(
                            CLASS_COLLISION,
                            format!(
                                "class `{}` is declared by more than one definition package",
                                class.name
                            ),
                        )
                        .with_label(Label::primary(
                            Span::new(def.file.clone(), class.span.start..class.span.end),
                            "duplicate declaration here",
                        ))
                        .with_note(format!(
                            "first declared in `{first}`; that declaration wins"
                        )),
                    );
                } else {
                    owner.insert(class.name.clone(), def.file.clone());
                }
            }
        }
    }
    diags
}

/// Detect `---@alias` names declared by more than one source across the
/// project — luals `duplicate-doc-alias` (`LB0310`, #113).
///
/// This is the alias counterpart of [`class_collisions`], but reaches project
/// *source* files too: after #110 a same-name `---@alias` in two project files
/// resolves silently first-wins, and this restores luals's warning at the
/// losing site. The winner order matches the runtime resolution
/// ([`Ambient::with_project_types`]): every `[types] defs` source in `defs`
/// (winner-first) is considered first — an ambient alias always wins — then the
/// project files in `project`'s stable order. The first source to declare a
/// name owns it and is never reported; each later declaration yields an
/// `LB0310` warning naming the file that already declared it.
///
/// Deliberately excludes the dialect stdlib layer (like `class_collisions`):
/// only the explicit `[types] defs` and project files participate, so a project
/// alias never collides with a hidden stdlib name.
#[must_use]
pub fn alias_collisions(
    defs: &[DefFile],
    project: &[(String, &crate::env::FileTypes)],
) -> Vec<Diagnostic> {
    let mut owner: BTreeMap<String, String> = BTreeMap::new();
    let mut diags = Vec::new();
    // `[types] defs` sources first — winner-first, so an ambient alias owns the
    // name and any project redeclaration below loses to it.
    for def in defs {
        let parse = lua::parse(&def.text, Dialect::Lua54);
        let items = luacats::harvest(&parse);
        let mut claimed_here: HashSet<String> = HashSet::new();
        for item in &items {
            for tag in &item.block.tags {
                let Tag::Alias(alias) = tag else { continue };
                if alias.name.is_empty() || !claimed_here.insert(alias.name.clone()) {
                    continue;
                }
                report_alias(
                    &mut owner,
                    &mut diags,
                    &alias.name,
                    &def.file,
                    alias.span.start..alias.span.end,
                );
            }
        }
    }
    // Project files, in the caller's stable order: the first to declare a name
    // (that no def already owns) wins; the rest lose.
    for (label, types) in project {
        for (name, alias) in types.aliases() {
            report_alias(
                &mut owner,
                &mut diags,
                name,
                label,
                alias.span.start..alias.span.end,
            );
        }
    }
    diags
}

/// Claim `name` for `file` if unclaimed, else emit an `LB0310` at `span`
/// pointing back at the owner.
fn report_alias(
    owner: &mut BTreeMap<String, String>,
    diags: &mut Vec<Diagnostic>,
    name: &str,
    file: &str,
    span: std::ops::Range<usize>,
) {
    if let Some(first) = owner.get(name) {
        diags.push(
            Diagnostic::warning(
                ALIAS_COLLISION,
                format!("alias `{name}` is declared more than once"),
            )
            .with_label(Label::primary(
                Span::new(file.to_string(), span),
                "duplicate declaration here",
            ))
            .with_note(format!(
                "first declared in `{first}`; that declaration wins"
            )),
        );
    } else {
        owner.insert(name.to_string(), file.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ty::Ty;

    /// A generic class's shape as harvested from a project file — the
    /// merged ambient a real hover/completion query would hold.
    fn box_ambient() -> Ambient {
        let src = "---@class Box<T>\n---@field item T\nlocal B = {}\nreturn B\n";
        let parsed = lua::parse(src, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        let types = crate::module_surface(&parsed, "box.lua", None).types;
        stdlib(Dialect::Lua54).with_project_types([&types])
    }

    /// The [`luacats::TypeExpr`] a `---@type <src>` annotation parses to —
    /// the shape a real LuaCATS caller (hover/completion) already holds.
    fn type_expr(src: &str) -> luacats::TypeExpr {
        let parsed = lua::parse(&format!("---@type {src}\nlocal b\n"), Dialect::Lua54);
        assert_eq!(
            parsed.errors(),
            &[],
            "annotation fixture must parse cleanly"
        );
        let items = luacats::harvest(&parsed);
        for item in &items {
            for tag in &item.block.tags {
                if let Tag::Type(t) = tag {
                    return t.types[0].clone();
                }
            }
        }
        panic!("no ---@type tag harvested from {src:?}");
    }

    #[test]
    fn class_members_of_a_bound_reference_resolves_the_bound_type() {
        // Round 3 review F48: `Ambient::class_members(name)` alone always
        // resolves through `class_shape_bound(name, &[])` — every parameter
        // free — so a caller that extracted just the name from `Box<number>`
        // and asked for its members got `item: T`, the exact "editor accepts
        // what CI rejects" shape #56's acceptance criterion forbids: the
        // checker resolves the same reference to `item: number`.
        let ambient = box_ambient();
        let ty = type_expr("Box<number>");
        let shape = ambient
            .class_members_of(&ty)
            .expect("Box<number> is a class reference");
        assert_eq!(
            shape.fields["item"].ty,
            Ty::Number,
            "the bound reference's member must resolve to the type it was bound to"
        );

        // The rejecting probe beside the accepting one: `class_members_bound`
        // called directly with the SAME argument must agree — proving
        // `class_members_of` is not doing anything `class_members_bound`
        // itself would not, and that a genuinely wrong argument is
        // distinguishable from the right one (not just "some concrete type").
        let bound_directly = ambient
            .class_members_bound("Box", &[Ty::String])
            .expect("Box is a class");
        assert_eq!(bound_directly.fields["item"].ty, Ty::String);
        assert_ne!(bound_directly.fields["item"].ty, shape.fields["item"].ty);
    }

    #[test]
    fn class_members_of_monomorphises_a_nested_generic_argument() {
        // The argument list is lowered against the env's OWN generic classes
        // (`TypeEnv::generic_classes`), which is what lets an argument that is
        // itself a generic reference resolve: without those templates in
        // scope, `Pair<number>` lowers to the bare `Ty::Named("Pair")` — a
        // name carrying none of its binding — and `Box<Pair<number>>`'s member
        // silently becomes an unresolvable reference rather than the pair's
        // shape. Measured: three surviving mutants returned an empty or
        // garbage template map here and no test noticed.
        let src = "\
---@class Pair<P>
---@field left P
---@field right P
---@class Boxed<T>
---@field item T
local B = {}
return B
";
        let parsed = lua::parse(src, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        let types = crate::module_surface(&parsed, "boxed.lua", None).types;
        let ambient = stdlib(Dialect::Lua54).with_project_types([&types]);

        let shape = ambient
            .class_members_of(&type_expr("Boxed<Pair<number>>"))
            .expect("Boxed<...> is a class reference");
        let Ty::Table(inner) = &shape.fields["item"].ty else {
            panic!(
                "the nested argument must resolve to Pair's shape, got {:?}",
                shape.fields["item"].ty
            );
        };
        assert_eq!(inner.fields["left"].ty, Ty::Number);
        assert_eq!(inner.fields["right"].ty, Ty::Number);

        // Rejecting probe: a different nested binding is a different shape,
        // so the argument is genuinely carried rather than erased to a
        // catch-all table both spellings would satisfy.
        let other = ambient
            .class_members_of(&type_expr("Boxed<Pair<string>>"))
            .expect("Boxed<...> is a class reference");
        assert_ne!(other.fields["item"].ty, shape.fields["item"].ty);
    }

    #[test]
    fn class_members_of_an_unbound_reference_stays_lenient() {
        // The one-variable control: the identical class, referenced bare —
        // no arguments to bind, so the member stays the free `T` (unknown
        // downstream), exactly as `Ambient::class_members` already behaves
        // and exactly as a bare `Box` written by hand means (#84).
        let ambient = box_ambient();
        let ty = type_expr("Box");
        let shape = ambient
            .class_members_of(&ty)
            .expect("Box is a class reference");
        assert_eq!(shape.fields["item"].ty, Ty::Named("T".to_string()));

        // Consistent with the plain by-name lookup, which this must not
        // diverge from for the unbound case.
        assert_eq!(shape, ambient.class_members("Box").expect("Box is a class"));
    }

    #[test]
    fn class_members_of_a_non_named_type_expression_is_none() {
        // The documented boundary: `class_members_of` resolves a `Named`
        // reference only, and only when the name is a class. A caller
        // holding a compound type (`T?`, a union, an array, ...) — not
        // `TypeExprKind::Named` at the syntax level at all — gets `None`
        // rather than a guess at which member it meant; a real `Named`
        // reference to a non-class name (`string`) is `None` for the same
        // reason `class_members`/`class_members_bound` already are.
        let ambient = box_ambient();
        for src in ["Box?", "Box|nil", "Box[]", "string"] {
            assert!(
                ambient.class_members_of(&type_expr(src)).is_none(),
                "{src} must not resolve as a bound class reference"
            );
        }
    }

    // === class_ancestry_truncated (production readiness review, finding 2,
    // crate-level gap) =======================================================
    //
    // `luabox-lsp`'s `merged_ambient.rs` already pins this accessor through
    // `MergedAmbient`'s wrapper, but that suite is out of scope for this
    // crate's own mutation gate (`cargo mutants -p luabox-types` runs only
    // `luabox-types`' tests). Nothing at THIS crate's level ever called
    // `Ambient::class_ancestry_truncated` directly — every other test above
    // reads a class's shape (`class_members`/`class_members_of`), never
    // whether that shape was truncated — so a survivor here was a public API
    // this crate ships with no test of its own ever exercising it.

    /// A single-file `---@class C0`, `---@class C1 : C0`, ..., `Cn : C(n-1)`
    /// chain of length `n` — the same shape `env.rs`'s own
    /// `MAX_ANCESTRY_DEPTH` boundary tests and `merged_ambient.rs`'s
    /// `chain_source` use.
    fn chain_source(n: usize) -> String {
        use std::fmt::Write as _;
        let mut src = String::from("---@class C0\n---@field item number\n");
        for i in 1..=n {
            let _ = writeln!(src, "---@class C{i} : C{}", i - 1);
        }
        src
    }

    /// The ambient a project file contributing `chain_source(n)` would merge
    /// into — `class_members`/`class_ancestry_truncated`'s real read path
    /// (`with_project_types`), not the raw `TypeEnv` `env.rs`'s own tests
    /// build directly.
    fn chain_ambient(n: usize) -> Ambient {
        let src = chain_source(n);
        let parsed = lua::parse(&src, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        let types = crate::module_surface(&parsed, "chain.lua", None).types;
        stdlib(Dialect::Lua54).with_project_types([&types])
    }

    #[test]
    fn class_ancestry_truncated_is_false_for_a_class_within_the_depth_cap() {
        // The floor side: a chain of exactly `MAX_ANCESTRY_DEPTH` classes
        // resolves in full, so the deepest class's ancestry was never
        // truncated. Pins the `-> false` direction: a mutant that always
        // returns `false` would pass this alone, which is exactly why the
        // `-> true` sibling test below also has to exist.
        let n = crate::env::MAX_ANCESTRY_DEPTH - 1;
        let ambient = chain_ambient(n);
        let name = format!("C{n}");
        let shape = ambient.class_members(&name).expect("the class resolves");
        assert!(
            shape.fields.contains_key("item"),
            "a chain within the limit must still merge C0's field"
        );
        assert!(!ambient.class_ancestry_truncated(&name));
    }

    #[test]
    fn class_ancestry_truncated_is_true_for_a_class_past_the_depth_cap() {
        // The ceiling side: one class past `MAX_ANCESTRY_DEPTH` already loses
        // C0's field to the cap, so the accessor must say so. Pins the
        // `-> true` direction: a mutant that always returns `true` would
        // pass this alone, which is exactly why the sibling test above also
        // has to exist — only together do the two pin both directions.
        let n = crate::env::MAX_ANCESTRY_DEPTH;
        let ambient = chain_ambient(n);
        let name = format!("C{n}");
        let shape = ambient.class_members(&name).expect("the class resolves");
        assert!(
            !shape.fields.contains_key("item"),
            "one class past the limit must already truncate before reaching C0's field"
        );
        assert!(ambient.class_ancestry_truncated(&name));
    }

    #[test]
    fn class_ancestry_truncated_does_not_leak_across_class_names() {
        // The query must answer for the class just resolved, not "has this
        // layer ever seen a depth-limit hit anywhere" — a shallow class
        // looked up after a truncated one must not inherit the previous
        // call's `true`.
        let deep_n = crate::env::MAX_ANCESTRY_DEPTH;
        let src = format!(
            "{}\n---@class Shallow\n---@field x number\n",
            chain_source(deep_n)
        );
        let parsed = lua::parse(&src, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        let types = crate::module_surface(&parsed, "chain.lua", None).types;
        let ambient = stdlib(Dialect::Lua54).with_project_types([&types]);

        let deep_name = format!("C{deep_n}");
        ambient
            .class_members(&deep_name)
            .expect("the deep class resolves");
        assert!(ambient.class_ancestry_truncated(&deep_name));

        ambient
            .class_members("Shallow")
            .expect("the shallow class resolves");
        assert!(!ambient.class_ancestry_truncated("Shallow"));
    }

    #[test]
    fn every_dialect_builds_without_unknown_type_names() {
        for dialect in Dialect::ALL {
            let ambient = stdlib(dialect);
            assert!(
                ambient.unknown_names().is_empty(),
                "{dialect:?} defs reference undeclared type names: {:?}",
                ambient.unknown_names()
            );
        }
    }

    #[test]
    fn global_names_enumerates_top_level_declarations() {
        // Module tables and scalar globals, keyed by their own top-level
        // name — not the dotted stdlib functions they carry.
        let names = stdlib(Dialect::Lua54).global_names();
        for expected in [
            "print", "assert", "pairs", "math", "string", "_G", "_VERSION",
        ] {
            assert!(names.contains(expected), "missing `{expected}`: {names:?}");
        }
        // `math.abs` is a stdlib function, not itself a top-level global.
        assert!(!names.contains("math.abs"));
    }

    #[test]
    fn global_names_include_project_defs() {
        let extra = vec!["---@meta\nlove = {}\nfunction love.load() end\n".to_string()];
        let ambient = build_ambient(Dialect::Lua54, &extra);
        assert!(ambient.global_names().contains("love"));
        // The stdlib set is still layered in underneath.
        assert!(ambient.global_names().contains("print"));
    }

    #[test]
    fn basic_globals_present_everywhere() {
        for dialect in Dialect::ALL {
            let a = stdlib(dialect);
            for name in [
                "print",
                "type",
                "pairs",
                "ipairs",
                "tostring",
                "tonumber",
                "pcall",
                "assert",
                "setmetatable",
                "require",
                "string.format",
                "string.rep",
                "table.insert",
                "math.floor",
                "os.time",
            ] {
                assert!(a.has_function(name), "{dialect:?} missing `{name}`");
            }
            assert!(a.has_global("_G"), "{dialect:?} missing `_G`");
            assert!(a.has_global("math"), "{dialect:?} missing `math` table");
        }
    }

    #[test]
    fn version_gated_availability() {
        // bit32 exists in 5.2 only.
        assert!(stdlib(Dialect::Lua52).has_function("bit32.band"));
        assert!(!stdlib(Dialect::Lua51).has_function("bit32.band"));
        assert!(!stdlib(Dialect::Lua54).has_function("bit32.band"));

        // utf8 is 5.3+.
        assert!(!stdlib(Dialect::Lua51).has_function("utf8.char"));
        assert!(!stdlib(Dialect::Lua52).has_function("utf8.char"));
        assert!(stdlib(Dialect::Lua53).has_function("utf8.char"));
        assert!(stdlib(Dialect::Lua54).has_function("utf8.char"));

        // table.pack/unpack are 5.2+; 5.1 has the `unpack` global instead.
        assert!(!stdlib(Dialect::Lua51).has_function("table.pack"));
        assert!(stdlib(Dialect::Lua51).has_function("unpack"));
        assert!(stdlib(Dialect::Lua52).has_function("table.pack"));

        // string.pack is 5.3+.
        assert!(!stdlib(Dialect::Lua52).has_function("string.pack"));
        assert!(stdlib(Dialect::Lua53).has_function("string.pack"));

        // table.move is 5.3+.
        assert!(!stdlib(Dialect::Lua52).has_function("table.move"));
        assert!(stdlib(Dialect::Lua54).has_function("table.move"));

        // jit/bit only on LuaJIT.
        assert!(stdlib(Dialect::LuaJit).has_function("bit.band"));
        assert!(stdlib(Dialect::LuaJit).has_global("jit"));
        assert!(!stdlib(Dialect::Lua54).has_function("bit.band"));

        // warn is 5.4 only.
        assert!(stdlib(Dialect::Lua54).has_function("warn"));
        assert!(!stdlib(Dialect::Lua53).has_function("warn"));

        // math.type is 5.3+.
        assert!(!stdlib(Dialect::Lua51).has_function("math.type"));
        assert!(stdlib(Dialect::Lua53).has_function("math.type"));
    }

    // --- ambient globals flowing into check + inference ------------------

    fn codes(dialect: Dialect, src: &str, strictness: crate::Strictness) -> Vec<String> {
        let ambient = stdlib(dialect);
        let parse = lua::parse(src, dialect);
        assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
        crate::check_file_with_ambient(&parse, "test.lua", strictness, dialect, Some(ambient))
            .iter()
            .map(|d| d.code.to_string())
            .collect()
    }

    fn strict(dialect: Dialect, src: &str) -> Vec<String> {
        codes(dialect, src, crate::Strictness::Strict)
    }

    #[test]
    fn stdlib_call_argument_is_typechecked() {
        // `string.rep(s, n)` wants an integer count.
        assert_eq!(
            strict(Dialect::Lua54, "string.rep(\"x\", \"y\")\n"),
            vec!["LB0300"]
        );
        assert_eq!(
            strict(Dialect::Lua54, "string.rep(\"x\", 3)\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn print_accepts_anything() {
        assert_eq!(
            strict(Dialect::Lua54, "print(1, \"two\", true, nil, {})\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn stdlib_result_type_flows_into_calls() {
        // `string.rep` returns `string`, so it satisfies a string parameter
        // and violates a number one.
        let ok = "\
---@param s string
local function f(s) end
f(string.rep(\"x\", 3))
";
        assert_eq!(strict(Dialect::Lua54, ok), Vec::<String>::new());
        let bad = "\
---@param n number
local function f(n) end
f(string.rep(\"x\", 3))
";
        assert_eq!(strict(Dialect::Lua54, bad), vec!["LB0300"]);
    }

    #[test]
    fn tonumber_overloads() {
        // 1-arg primary form.
        assert_eq!(
            strict(Dialect::Lua54, "local n = tonumber(\"3\")\n"),
            Vec::<String>::new()
        );
        // 2-arg (base) overload form.
        assert_eq!(
            strict(Dialect::Lua54, "local n = tonumber(\"ff\", 16)\n"),
            Vec::<String>::new()
        );
        // Neither form takes zero arguments.
        assert_eq!(strict(Dialect::Lua54, "tonumber()\n"), vec!["LB0301"]);
        // The base must be an integer — no form accepts a string base.
        assert_eq!(
            strict(Dialect::Lua54, "tonumber(\"ff\", \"x\")\n"),
            vec!["LB0300"]
        );
    }

    #[test]
    fn table_insert_two_and_three_arg_forms() {
        let src = "\
local t = {}
table.insert(t, 1)
table.insert(t, 2, 3)
";
        assert_eq!(strict(Dialect::Lua54, src), Vec::<String>::new());
    }

    #[test]
    fn local_shadows_ambient_global() {
        // Ambient `tostring` takes exactly one argument, so a 2-arg call
        // errors — unless a local of the same name shadows it.
        assert_eq!(strict(Dialect::Lua54, "tostring(1, 2)\n"), vec!["LB0301"]);
        let shadowed = "\
local function tostring(...) end
tostring(1, 2)
";
        assert_eq!(strict(Dialect::Lua54, shadowed), Vec::<String>::new());
    }

    #[test]
    fn module_constant_field_reads_typed() {
        // `math.pi` is a number; passing it to a string parameter errors.
        let src = "\
---@param s string
local function f(s) end
f(math.pi)
";
        assert_eq!(strict(Dialect::Lua54, src), vec!["LB0300"]);
    }

    #[test]
    fn version_gated_stdlib_call() {
        // string.pack is 5.3+: a wrong first argument errors in 5.4 (the
        // format must be a string)...
        assert_eq!(
            strict(Dialect::Lua54, "string.pack(123, 1)\n"),
            vec!["LB0300"]
        );
        // ...while in 5.1 `string.pack` is undeclared: an unknown receiver,
        // never checked, no crash, no diagnostic.
        assert_eq!(
            strict(Dialect::Lua51, "string.pack(123, 1)\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn bit32_present_only_in_52() {
        // In 5.2 bit32 is a real module; the call typechecks clean.
        assert_eq!(
            strict(Dialect::Lua52, "local x = bit32.band(1, 2)\n"),
            Vec::<String>::new()
        );
        // In 5.4 bit32 is absent: unknown receiver, no diagnostic (an absent
        // global read is not itself an error).
        assert_eq!(
            strict(Dialect::Lua54, "local x = bit32.band(1, 2)\n"),
            Vec::<String>::new()
        );
    }

    // --- cross-package class collisions (#108) ---------------------------

    #[test]
    fn build_ambient_checked_reports_collision_and_first_wins() {
        let defs = vec![
            DefFile {
                file: "defs/a.d.lua".to_string(),
                text: "---@meta\n---@class Widget\n---@field a number\n".to_string(),
            },
            DefFile {
                file: "dep/defs/b.d.lua".to_string(),
                text: "---@meta\n---@class Widget\n---@field b number\n".to_string(),
            },
        ];
        let (ambient, diags) = build_ambient_checked(Dialect::Lua54, &defs);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].code.to_string(), "LB0307");
        assert_eq!(diags[0].severity, luabox_diag::Severity::Warning);
        // Loser at the span, winner named in the note.
        let label = diags[0].primary_label().expect("primary label");
        assert_eq!(label.span.file, "dep/defs/b.d.lua");
        assert!(
            diags[0].notes.iter().any(|n| n.contains("defs/a.d.lua")),
            "note names the winner: {:?}",
            diags[0].notes
        );
        // Deterministic winner: the first (project-local) `Widget` — field `a`.
        let parse = lua::parse("---@type Widget\nlocal w = { a = 1 }\n", Dialect::Lua54);
        let clean = crate::check_file_with_ambient(
            &parse,
            "t.lua",
            crate::Strictness::Strict,
            Dialect::Lua54,
            Some(&ambient),
        );
        assert!(
            clean.is_empty(),
            "project decl (field `a`) must win: {clean:?}"
        );
        let parse = lua::parse("---@type Widget\nlocal w = { b = 1 }\n", Dialect::Lua54);
        let loser = crate::check_file_with_ambient(
            &parse,
            "t.lua",
            crate::Strictness::Strict,
            Dialect::Lua54,
            Some(&ambient),
        );
        assert!(
            !loser.is_empty(),
            "dependency decl (field `b`) must have lost"
        );
    }

    // --- duplicate-doc-alias (#113) -------------------------------------

    fn file_types(src: &str) -> crate::env::FileTypes {
        crate::module_surface(&lua::parse(src, Dialect::Lua54), "x.lua", None).types
    }

    #[test]
    fn alias_collision_across_project_files_first_wins() {
        let a = file_types("---@alias Id integer\n");
        let b = file_types("---@alias Id string\n");
        let diags = alias_collisions(&[], &[("a.lua".to_string(), &a), ("b.lua".to_string(), &b)]);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].code.to_string(), "LB0310");
        assert_eq!(diags[0].severity, luabox_diag::Severity::Warning);
        // The second file (stable order) loses; the first is named in the note.
        let label = diags[0].primary_label().expect("primary label");
        assert_eq!(label.span.file, "b.lua");
        assert!(
            diags[0].notes.iter().any(|n| n.contains("a.lua")),
            "note names the winner: {:?}",
            diags[0].notes
        );
    }

    #[test]
    fn alias_defs_win_over_project() {
        let defs = vec![DefFile {
            file: "defs/ids.d.lua".to_string(),
            text: "---@meta\n---@alias Id integer\n".to_string(),
        }];
        let proj = file_types("---@alias Id string\n");
        let diags = alias_collisions(&defs, &[("p.lua".to_string(), &proj)]);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].code.to_string(), "LB0310");
        let label = diags[0].primary_label().expect("primary label");
        assert_eq!(label.span.file, "p.lua", "the project decl loses to defs");
        assert!(diags[0].notes.iter().any(|n| n.contains("defs/ids.d.lua")));
    }

    #[test]
    fn distinct_aliases_no_collision() {
        let a = file_types("---@alias Id integer\n");
        let b = file_types("---@alias Name string\n");
        let diags = alias_collisions(&[], &[("a.lua".to_string(), &a), ("b.lua".to_string(), &b)]);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn an_intra_file_class_repeat_is_one_collision_not_two() {
        // The within-file dedup's whole job (#58 mutation audit): a package
        // whose own file repeats a class collides with the OTHER package
        // once, not once per repetition — the repeat is a duplicate
        // declaration (the union rule's business, #49), not two claims.
        let defs = vec![
            DefFile {
                file: "defs/a.d.lua".to_string(),
                text: "---@meta\n---@class Widget\n---@field a number\n".to_string(),
            },
            DefFile {
                file: "dep/defs/b.d.lua".to_string(),
                text: "---@meta\n---@class Widget\n---@field b number\n\
                       ---@class Widget\n---@field c number\n"
                    .to_string(),
            },
        ];
        let (_ambient, diags) = build_ambient_checked(Dialect::Lua54, &defs);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].code.to_string(), "LB0307");
    }

    #[test]
    fn an_intra_file_alias_repeat_is_one_collision_not_two() {
        // The alias twin of the class dedup above (#113 dedups per file the
        // same way; the duplicate-alias diagnostic within one file is
        // LB0310's own separate report, not a package collision).
        let defs = vec![
            DefFile {
                file: "a.d.lua".to_string(),
                text: "---@meta\n---@alias Id integer\n".to_string(),
            },
            DefFile {
                file: "b.d.lua".to_string(),
                text: "---@meta\n---@alias Id string\n---@alias Id boolean\n".to_string(),
            },
        ];
        let diags = alias_collisions(&defs, &[]);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].code.to_string(), "LB0310");
    }

    #[test]
    fn build_ambient_checked_distinct_classes_no_collision() {
        let defs = vec![
            DefFile {
                file: "a.d.lua".to_string(),
                text: "---@meta\n---@class Alpha\n---@field a number\n".to_string(),
            },
            DefFile {
                file: "b.d.lua".to_string(),
                text: "---@meta\n---@class Beta\n---@field b number\n".to_string(),
            },
        ];
        let (_ambient, diags) = build_ambient_checked(Dialect::Lua54, &defs);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn project_defs_layer_over_stdlib() {
        // A project-local package adds a global alongside the stdlib set.
        let extra = vec![
            "---@meta\n---@param name string\n---@return boolean\nfunction love_setup(name) end\n"
                .to_string(),
        ];
        let ambient = build_ambient(Dialect::Lua54, &extra);
        let parse = lua::parse("love_setup(1)\nprint(\"still stdlib\")\n", Dialect::Lua54);
        let diags = crate::check_file_with_ambient(
            &parse,
            "test.lua",
            crate::Strictness::Strict,
            Dialect::Lua54,
            Some(&ambient),
        );
        let codes: Vec<String> = diags.iter().map(|d| d.code.to_string()).collect();
        // `love_setup` wants a string; `print` (stdlib) still resolves.
        assert_eq!(codes, vec!["LB0300"]);
    }
}
