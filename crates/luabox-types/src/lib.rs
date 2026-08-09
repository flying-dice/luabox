//! Unified type IR and checker — **Semantics** bounded context
//! (SPEC.md §3, §16).
//!
//! One internal type IR fed by the LuaCATS annotation front-end (`---@class`
//! etc., full compatibility non-negotiable) — the one and only type format
//! (DIRECTION.md). One checker, no parallel type system.
//!
//! **P0 scope (this crate today):** the annotation-driven subset behind
//! `luabox check` — types come from LuaCATS annotations and literals only.
//! The load-bearing design decision is the *structural* table
//! representation ([`ty::TableTy`]): field map + indexers + array part,
//! never an opaque `table` primitive, so checking a table literal against
//! a `---@class` parameter produces field-level diagnostics and P1's rich
//! table inference (SPEC.md §3 hard requirement) extends this IR instead
//! of replacing it.
//!
//! **Cross-file `require` + workspace-global classes:** [`module_surface`]
//! reifies a file's chunk `return` type (annotations authoritative, no
//! call-site seeding, own requires left unresolved so the graph stays
//! acyclic under cycles) together with the file's workspace-global
//! `---@class`/`---@enum` declarations — luals parity: a class declared in
//! any checked file, including its `function Class:method` member
//! attachments, is nameable and resolvable from every other file
//! ([`Ambient::with_project_types`]). [`check_file_with_requires`] threads
//! a `require`-string → export-type registry into checking, so
//! `local M = require("mod")` types `M` from the required module's
//! annotations — conformance assertions work in consumer files, not just
//! the defining file (#85). The CLI (`check_cmd`) and LSP
//! (`diagnostics`) each build the registry + merged ambient from the
//! project source set, reusing the bundler's / salsa DB's `require`
//! path-mapping.
//!
//! **Types from a bare luarocks tree (#30):** [`rocks::harvest`] reads the
//! LuaCATS annotations a rock's *installed sources* already carry
//! (`lua_modules/share/lua/<X.Y>/**.lua`) for their class/enum/alias
//! declarations and `require`-export types, so `luarocks install --tree
//! lua_modules <rock>` yields visible, enforced signatures with no manifest
//! declaration at all. Ambient-relaxed and surface-only: vendored bodies are
//! never checked, and a rock file that does not parse is skipped silently.
//!
//! **Bidirectional / contextual typing (#120):** a function-literal
//! parameter takes its type from the expected `fun(...)` at a call-argument
//! (`---@param cb fun(...)`) or `---@type fun(...)` position, so the lambda
//! body checks with no per-parameter annotation. Conservative: no expected
//! function type (unannotated callee, `unknown`/`any`/non-function expected)
//! leaves the parameter `unknown` exactly as before, and an explicit
//! `---@param` on the lambda wins. The expected type also now propagates
//! transitively: into a table literal (a function-valued field's lambda takes
//! the field's `fun(...)` type; a nested table field takes its declared
//! class), through `return` position (a `---@return` type contextually types
//! the returned literal), and across nested callback layers
//! (`fun(a): fun(b)` types both). Still deferred: overload-driven or generic
//! callback inference (a generic callback is skipped, never guessed).
//!
//! **P1 (TODO):** overload/generic-driven contextual typing, generics as
//! real type variables, function subtyping.
//!
//! Diagnostics carry `LB03xx` codes registered in `luabox-diag` (this
//! crate depends on it the way rustc crates depend on `rustc_errors`).

mod assign;
mod check;
mod codes;
mod defs;
mod directive;
mod env;
mod generics;
mod infer;
mod lower;
pub mod rocks;
pub mod ty;
mod version;

pub use assign::{Exactness, assignable};
pub use codes::{CLASS_COST_LIMIT, CLASS_DEPTH_LIMIT, CYCLIC_CLASS};
pub use defs::{
    Ambient, DefFile, alias_collisions, build_ambient, build_ambient_checked, stdlib as stdlib_defs,
};
pub use directive::{
    DirectiveScan, RULE_CLASS_ANCESTRY_TOO_COSTLY, RULE_CLASS_ANCESTRY_TOO_DEEP,
    RULE_CYCLIC_CLASS_ANCESTRY, parse_directive_body,
};
pub use env::{FileTypes, MAX_ANCESTRY_DEPTH, TypeEnv};
pub use infer::{ExternalTypes, InferredBinding, InferredReturn};
pub use rocks::{RockFile, RockModule, RockSurfaces};
pub use version::VersionReq;

use std::collections::HashMap;

use luabox_diag::Diagnostic;
use luabox_syntax::{LineIndex, lua, luacats};

use codes::{FIELD_NOT_FOUND, TYPE_MISMATCH};
use infer::InferMode;
use ty::Ty;

/// The strictness ladder (SPEC.md §3): `none` → `warn` → `strict`.
///
/// - `None`: type diagnostics are suppressed entirely.
/// - `Warn`: mismatches are warnings; `unknown` is assignable both ways.
/// - `Strict`: mismatches are errors; `unknown -> T` is itself a mismatch
///   (untyped = `unknown`, not `any`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strictness {
    /// No type diagnostics.
    None,
    /// Warning severity, permissive `unknown`.
    Warn,
    /// Error severity, strict `unknown`.
    Strict,
}

impl Strictness {
    /// Map the manifest's `[types] strict` boolean: `true` → strict,
    /// `false` → warn. TODO: surface the full three-level ladder (plus
    /// per-file overrides) in the manifest; `None` is currently only
    /// reachable programmatically.
    #[must_use]
    pub fn from_manifest_flag(strict: bool) -> Self {
        if strict {
            Strictness::Strict
        } else {
            Strictness::Warn
        }
    }
}

/// The display-inference surface behind editor inlay hints: every named
/// binding's final inferred type, every unannotated function's inferred
/// return types, and the cross-file exchange surface (the module's export
/// type + observed outgoing call arguments).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DisplayTypes {
    /// Every binding's reified type at its declaration range.
    pub bindings: Vec<InferredBinding>,
    /// Inferred returns per unannotated function, keyed by source range.
    pub returns: Vec<InferredReturn>,
    /// The inferred type of the chunk's `return` value — what a dependent
    /// file's `require` of this module evaluates to.
    pub module_export: Option<ty::Ty>,
    /// Argument types observed at calls of functions this file does not
    /// define, keyed by terminal callee name — parameter seeds for the
    /// files this one requires.
    pub outgoing_calls: std::collections::HashMap<String, Vec<ty::Ty>>,
}

/// Run the rich table inference in *display mode* over one parsed file —
/// the editor inlay-hint surface.
///
/// Same inference as [`check_file`] (annotations stay authoritative;
/// `ambient` merges definition-package globals beneath the file's own
/// declarations, exactly as in [`check_file_with_ambient`]) with two
/// additions:
///
/// - **Call-site parameter seeding** — an unannotated parameter takes the
///   union of the argument types observed at the function's call sites, so
///   bodies of unannotated functions type through.
/// - **Cross-file inputs** (`externals`) — `require("mod")` evaluates to
///   the target module's export type, and exported functions' parameters
///   seed from dependent files' observed call arguments.
///
/// Display-only — the checker never sees seeded types, so no diagnostic
/// can arise from them.
#[must_use]
pub fn infer_display_types(
    parse: &lua::Parse,
    file: &str,
    ambient: Option<&Ambient>,
    externals: Option<&ExternalTypes>,
) -> DisplayTypes {
    let items = luacats::harvest(parse);
    let env = TypeEnv::build_from_items(parse, &items, ambient);
    let lowered = luabox_hir::lower(parse);
    let outcome = infer::run(
        &lowered,
        &env,
        file,
        Exactness::Loose,
        InferMode::Display,
        externals,
    );
    DisplayTypes {
        bindings: outcome.binding_types,
        returns: outcome.fn_returns,
        module_export: outcome.module_export,
        outgoing_calls: outcome.outgoing_calls,
    }
}

/// One project file's cross-file type surface, in **check mode** (#85).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ModuleSurface {
    /// The type a `require` of this file evaluates to: the reified type of
    /// the chunk's top-level `return` expression. `None` when the chunk has
    /// no `return` value.
    pub export: Option<Ty>,
    /// The workspace-global `---@class`/`---@enum` declarations this file
    /// contributes (luals parity: a class declared in any checked project
    /// file — including its `function Class:method` member attachments — is
    /// nameable and resolvable from every other file). Merged into every
    /// file's ambient scope via [`Ambient::with_project_types`].
    pub types: FileTypes,
}

/// The two derived artifacts every per-file entry point in this crate needs
/// from a parse: the harvested LuaCATS annotation blocks and the lowered HIR.
///
/// Both are pure functions of the parse, so a caller that runs several entry
/// points over the *same* file — as `luabox check` does, taking a module's
/// surface, its `require` inventory, and then checking it — derives them once
/// and threads them through the `_with_artifacts` entry points instead of
/// paying for `harvest` + `lower` again per call (CC-M1). The plain entry
/// points ([`module_surface`], [`check_file_with_requires`],
/// [`module_requires`]) still exist and simply derive what they need
/// themselves, so a one-shot caller need not know this type exists.
#[derive(Debug, Clone)]
pub struct FileArtifacts {
    items: Vec<luacats::AnnotatedItem>,
    lowered: luabox_hir::LoweredFile,
}

impl FileArtifacts {
    /// Harvest the annotations and lower the HIR of one parsed file.
    #[must_use]
    pub fn new(parse: &lua::Parse) -> Self {
        Self {
            items: luacats::harvest(parse),
            lowered: luabox_hir::lower(parse),
        }
    }

    /// This file's static `require` module strings, in source order —
    /// [`module_requires`] without re-lowering.
    #[must_use]
    pub fn requires(&self) -> Vec<String> {
        requires_of(&self.lowered)
    }

    /// This file's harvested LuaCATS annotation blocks — [`Self::new`]'s own
    /// `luacats::harvest` call, exposed so a caller that already built
    /// `FileArtifacts` for other reasons (module surface, checking) can
    /// reuse the same harvest for its own annotation-only walk instead of
    /// harvesting the file a second time (round 6 review M19:
    /// `luabox-cli`'s `check_cmd::deep_class_chain_diagnostics` used to call
    /// `luacats::harvest` again, serially, for every project file, even
    /// though this is the exact harvest `FileArtifacts::new` already built
    /// for the same file).
    #[must_use]
    pub fn items(&self) -> &[luacats::AnnotatedItem] {
        &self.items
    }
}

/// Compute one file's [`ModuleSurface`]: the reified `require`-export type
/// plus the workspace-global class/enum declarations.
///
/// Mirrors what luals resolves a `require("mod")` call to — the module
/// file's `return` type — but computed for the *checker*, so:
///
/// - **annotations are authoritative** and there is **no call-site
///   parameter seeding** (unlike the display-mode export behind inlay
///   hints): an unannotated exported function's parameters stay `unknown`,
///   never a guessed type, so a consumer's checks never rest on inference
///   about *other* files' call sites; and
/// - the file's **own** `require`s are left unresolved (`unknown`), which is
///   what keeps the cross-file registry acyclic and cycle-tolerant — a
///   `require` cycle resolves each participant against a partner computed
///   without following back.
///
/// `ambient` is the same definition-package layer
/// [`check_file_with_ambient`] uses (needed to model e.g. `setmetatable`
/// inside the module); named types the surface mentions are carried by name
/// and resolved in the *consumer's* environment.
#[must_use]
pub fn module_surface(parse: &lua::Parse, file: &str, ambient: Option<&Ambient>) -> ModuleSurface {
    module_surface_with_artifacts(parse, file, ambient, &FileArtifacts::new(parse))
}

/// [`module_surface`] over already-derived [`FileArtifacts`], for a caller
/// that also checks the same file and must not harvest and lower it twice.
#[must_use]
pub fn module_surface_with_artifacts(
    parse: &lua::Parse,
    file: &str,
    ambient: Option<&Ambient>,
    artifacts: &FileArtifacts,
) -> ModuleSurface {
    let env = build_file_env(parse, artifacts, ambient);
    module_surface_from_env(&env, file, artifacts)
}

/// The environment [`module_surface_from_env`]/[`check_file_from_env`] share
/// — split out for a caller needing both against the *same* `ambient` that
/// has a place to hold `env` across both calls.
///
/// [`module_surface_with_artifacts`] and [`check_file_with_artifacts`] each
/// build their own `env` this way and discard it after one use, which is
/// **deliberate**, not merely "the simple option" (round 4 review finding 6
/// reverted round 4 review R14's attempt at sharing one build across both
/// halves in the CLI batch path, `check_cmd.rs`'s `run_passes` — see that
/// function's doc comment for the measurement): the check half needs every
/// file's export already collected into the project-wide `require` registry
/// before any file can be checked, so a caller cannot fold this and
/// [`check_file_from_env`] into one per-file call — it has to build the
/// export half for every file, THEN check every file. R14 kept each file's
/// built `TypeEnv` alive in a `Vec` across that gap instead of building a
/// second one; that traded CPU time for peak memory that scales with
/// (file count × ambient size) instead of (rayon parallelism × ambient
/// size) — an O(N²)-ish regression measured at ~29x peak RSS on a 500-file
/// synthetic project for a ~1.76x CPU win, the wrong side of that trade at
/// realistic project sizes.
///
/// **Do not thread one `env` from this function across a `rayon` pass over
/// a project's files** — that is the CLI batch path
/// (`check_cmd.rs`'s `run_passes`) above all; it is exactly the shape R14
/// had and reverted. Re-measured independently for the gate below (N-file
/// project, two `---@class` declarations per file, a linear `require`
/// chain, release build, peak RSS via `scripts/peak-rss.py`, three runs
/// each):
///
///   N      one build/file (this function, per call)   one retained `Vec<TypeEnv>` (R14)
///   100    11 MiB                                       51 MiB   (4.6x)
///   200    15 MiB                                      145 MiB   (9.3x)
///   300    20 MiB                                      290 MiB  (14.5x)
///   500    29 MiB                                      734 MiB  (25.3x)
///
/// `scripts/perf-gate.sh`'s "RETAINED-TYPEENV REGRESSION GATE" leg
/// reproduces the N=500 row on every CI run (budget 100 MiB — the
/// transient column has ~3x headroom under it, the retained column misses
/// it by ~7x) specifically so this regression fails loudly instead of
/// shipping again. `build_file_env` still exists for a caller with a good
/// reason to hold `env` across two calls of its own on ONE file — just
/// budget the same tradeoff before reaching for it across many.
// Round 6 review M47: this used to be `pub fn` with zero callers outside
// this file — round 5 raised the same finding (N51) and it was answered
// with a longer doc comment instead of a visibility change. Grepped the
// whole workspace immediately before this edit: every reference to
// `build_file_env` outside this file is a doc comment or intra-doc link
// (`check_cmd.rs:366`, and the `[`build_file_env`]` links in this file's own
// docs below), never a call. Private — the doc comment above still stands as
// the record of why the function exists and what NOT to do with it.
#[must_use]
fn build_file_env(
    parse: &lua::Parse,
    artifacts: &FileArtifacts,
    ambient: Option<&Ambient>,
) -> TypeEnv {
    TypeEnv::build_from_items(parse, &artifacts.items, ambient)
}

/// [`module_surface_with_artifacts`] over an already-built [`TypeEnv`]
/// ([`build_file_env`]), for a caller that also runs [`check_file_from_env`]
/// against the identical `env` and must not pay
/// `TypeEnv::build_from_items`'s ambient-cloning cost twice — see
/// [`build_file_env`]'s doc comment for why the CLI batch path builds its
/// own `env` per call instead (round 4 review finding 6).
///
/// The CLI batch path (`check_cmd.rs`'s `run_passes`) must NOT call this
/// with an `env` held alive across its `rayon` pass over every project
/// file — that reintroduces R14's retained `Vec<TypeEnv>`, measured at up
/// to ~25x peak RSS over the transient one-build-per-call shape at N=500
/// files (see [`build_file_env`]'s doc comment for the full table), and is
/// now caught by `scripts/perf-gate.sh`'s "RETAINED-TYPEENV REGRESSION
/// GATE" leg (500-file corpus, budget 100 MiB).
// M47: same as `build_file_env` above — zero external callers, verified by
// the same workspace grep.
#[must_use]
fn module_surface_from_env(env: &TypeEnv, file: &str, artifacts: &FileArtifacts) -> ModuleSurface {
    let items = &artifacts.items;
    let outcome = infer::run(
        &artifacts.lowered,
        env,
        file,
        Exactness::Strict,
        InferMode::Check,
        None,
    );
    let types = FileTypes::collect(items, env, &outcome.carrier_class_final, file);
    ModuleSurface {
        export: outcome.module_export,
        types,
    }
}

/// The static `require` module strings this file names, in source order —
/// the keys a cross-file export registry is built over. Dynamic
/// (non-literal) requires are excluded (they are unresolvable and the
/// bundler hard-errors on them at build time).
#[must_use]
pub fn module_requires(parse: &lua::Parse) -> Vec<String> {
    requires_of(&luabox_hir::lower(parse))
}

/// The shared body of [`module_requires`] and [`FileArtifacts::requires`].
fn requires_of(lowered: &luabox_hir::LoweredFile) -> Vec<String> {
    lowered
        .requires()
        .iter()
        .map(|edge| edge.module.clone())
        .collect()
}

/// Typecheck one parsed file against its own annotations.
///
/// `file` names the file in diagnostic spans. Cross-file `require`
/// resolution is available through [`check_file_with_requires`]; this
/// entry point resolves no requires.
///
/// `edition` is the project Lua version (manifest `edition`) — the version a
/// `---@version` predicate is matched against ([`VersionReq`]).
#[must_use]
pub fn check_file(
    parse: &lua::Parse,
    file: &str,
    strictness: Strictness,
    edition: lua::Dialect,
) -> Vec<Diagnostic> {
    check_file_with_ambient(parse, file, strictness, edition, None)
}

/// Typecheck one parsed file with an ambient definition-package layer in
/// reach.
///
/// `ambient` is the definition-package layer selected by the project
/// `edition` ([`stdlib_defs`] / [`build_ambient`]): its stdlib globals and
/// module tables become visible to both the checker and inference, merged
/// beneath the file's own declarations (SPEC.md §3).
#[must_use]
pub fn check_file_with_ambient(
    parse: &lua::Parse,
    file: &str,
    strictness: Strictness,
    edition: lua::Dialect,
    ambient: Option<&Ambient>,
) -> Vec<Diagnostic> {
    check_file_with_requires(parse, file, strictness, edition, ambient, &HashMap::new())
}

/// Typecheck one parsed file with an ambient definition-package layer AND a
/// cross-file `require`-export registry in reach (#85).
///
/// `requires` maps each `require("mod")` module string this file names to
/// the resolved target module's [`module_export`] type. A `require` whose
/// string is absent from the map (unresolved — a file not in the project,
/// or an external/runtime module) evaluates to `unknown`, exactly as
/// before, and raises no diagnostic of its own (luals does not error on an
/// unresolved `require`; the bundler is where an unresolvable static
/// `require` becomes a hard error).
///
/// The registry only feeds `require` resolution: it never enables
/// call-site parameter seeding, so no diagnostic can arise from inference
/// about other files' call sites — only from the required module's own
/// annotations flowing into this file at its use sites.
#[must_use]
pub fn check_file_with_requires<S: std::hash::BuildHasher>(
    parse: &lua::Parse,
    file: &str,
    strictness: Strictness,
    edition: lua::Dialect,
    ambient: Option<&Ambient>,
    requires: &HashMap<String, Ty, S>,
) -> Vec<Diagnostic> {
    check_file_with_artifacts(
        parse,
        file,
        strictness,
        edition,
        ambient,
        requires,
        &FileArtifacts::new(parse),
    )
}

/// [`check_file_with_requires`] over already-derived [`FileArtifacts`], for a
/// caller that also took the file's [`module_surface`] and must not harvest
/// and lower it twice (CC-M1).
#[must_use]
pub fn check_file_with_artifacts<S: std::hash::BuildHasher>(
    parse: &lua::Parse,
    file: &str,
    strictness: Strictness,
    edition: lua::Dialect,
    ambient: Option<&Ambient>,
    requires: &HashMap<String, Ty, S>,
    artifacts: &FileArtifacts,
) -> Vec<Diagnostic> {
    check_file_with_artifacts_and_sources(
        parse, file, strictness, edition, ambient, requires, artifacts, None,
    )
}

/// A project file name → that file's source text lookup, called on demand by
/// [`check_file_with_artifacts_and_sources`]'s cross-file suppression scan.
/// Named so the signatures that carry it read as "a source resolver", not a
/// `dyn Fn` clippy flags as too complex to leave inline.
pub type SourceResolver<'a> = &'a dyn Fn(&str) -> Option<String>;

/// [`check_file_with_artifacts`] plus a cross-file source resolver — the fix
/// for the `---@diagnostic disable` escape hatch not working cross-file
/// (production readiness review G1).
///
/// A `---@class` cycle/depth-limit diagnostic (`LB0317`/`LB0318`) can carry a
/// primary label whose span belongs to a **different** project file than
/// `file` — the `cross_file_class_decl_span` attribution tier in
/// [`check::report_depth_limit_hits`]/[`check::report_cyclic_class_hits`]
/// fires whenever the offending class is declared in a file other than the
/// one whose check pass happened to trip the resolver's guard. The
/// suppression scan below must honor *that* file's own
/// `---@diagnostic disable` directives, at *that* file's own line numbers —
/// never this file's, which is what the plain [`check_file_with_artifacts`]
/// used to do unconditionally (reusing `file`'s `LineIndex` against another
/// file's byte offsets — silently wrong line numbers feeding suppression, so
/// a correctly-scoped `disable-line` could hit or miss arbitrarily).
///
/// `other_sources` resolves a project file name (a diagnostic's foreign
/// `Span::file`) to that file's source text, on demand — called at most once
/// per distinct foreign file a suppressible diagnostic actually names, never
/// for a file with nothing to suppress. `None` (what
/// [`check_file_with_artifacts`] passes) means "no cross-file lookup is
/// available": a cross-file diagnostic is then left **unsuppressed** rather
/// than checked against the wrong file — the safe default, and a strict
/// correctness improvement over the old always-wrong-when-foreign behaviour,
/// not merely "no worse". Only a caller that holds every project file's
/// source in reach — `luabox-cli`'s `check_cmd::check_one` — can supply a
/// resolver and get full cross-file suppression.
#[must_use]
#[allow(
    clippy::too_many_arguments,
    reason = "one more link in the check_file → check_file_with_ambient → \
              check_file_with_requires → check_file_with_artifacts entry-point \
              ladder, each threading exactly one more caller-supplied input \
              through to check_file_from_env; splitting these into a struct \
              would just move the same nine names one level down"
)]
pub fn check_file_with_artifacts_and_sources<S: std::hash::BuildHasher>(
    parse: &lua::Parse,
    file: &str,
    strictness: Strictness,
    edition: lua::Dialect,
    ambient: Option<&Ambient>,
    requires: &HashMap<String, Ty, S>,
    artifacts: &FileArtifacts,
    other_sources: Option<SourceResolver<'_>>,
) -> Vec<Diagnostic> {
    let env = build_file_env(parse, artifacts, ambient);
    check_file_from_env(
        &env,
        parse,
        file,
        strictness,
        edition,
        ambient,
        requires,
        artifacts,
        other_sources,
    )
}

/// [`check_file_with_artifacts`] over an already-built [`TypeEnv`]
/// ([`build_file_env`]) — see [`build_file_env`]'s doc comment for why the
/// CLI batch path does not currently hold one `env` across a call to this
/// and a call to [`module_surface_from_env`] (round 4 review finding 6).
///
/// Same rule as [`module_surface_from_env`]: the CLI batch path
/// (`check_cmd.rs`'s `run_passes`/`check_one`) must build and drop its own
/// `env` per file, never one held across its `rayon` pass over the whole
/// project — R14 did that and cost up to ~25x peak RSS over the transient
/// shape at N=500 files (measured table in [`build_file_env`]'s doc
/// comment), now a CI-blocking leg in `scripts/perf-gate.sh`
/// ("RETAINED-TYPEENV REGRESSION GATE", 500-file corpus, budget 100 MiB).
// M47: same as `build_file_env` above — zero external callers, verified by
// the same workspace grep.
#[must_use]
#[allow(
    clippy::too_many_arguments,
    reason = "the bottom of the check_file* entry-point ladder — see \
              check_file_with_artifacts_and_sources's identical allow"
)]
fn check_file_from_env<S: std::hash::BuildHasher>(
    env: &TypeEnv,
    parse: &lua::Parse,
    file: &str,
    strictness: Strictness,
    edition: lua::Dialect,
    ambient: Option<&Ambient>,
    requires: &HashMap<String, Ty, S>,
    artifacts: &FileArtifacts,
    other_sources: Option<SourceResolver<'_>>,
) -> Vec<Diagnostic> {
    let items = &artifacts.items;
    // A `---@meta` definition package: its `---@class` declarations are
    // contracts, not carriers, so no `: Interface` conformance runs inside it
    // (#107).
    let is_meta = items.iter().any(|it| {
        it.block
            .tags
            .iter()
            .any(|t| matches!(t, luacats::Tag::Meta(_)))
    });

    let mut diags: Vec<Diagnostic> = Vec::new();

    // A resolved `require`-export registry (#85) reaches inference through
    // the display-mode `externals` channel, but with call-site parameter
    // seeding OFF (`fn_param_seeds` empty, `seed_params` false below): only
    // `require("mod")` resolution is enabled, which is sound for checking
    // because a module's export type is annotation-authoritative. The empty
    // registry stays `None`, so a file with no resolved requires checks
    // byte-for-byte as before.
    let externals = if requires.is_empty() {
        None
    } else {
        Some(ExternalTypes {
            // Re-collect into the default-hasher map `ExternalTypes` holds
            // (the caller's map may use any `BuildHasher`).
            requires: requires
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            fn_param_seeds: HashMap::new(),
        })
    };
    if strictness != Strictness::None {
        // Rich table inference (SPEC.md §3) runs first: the checker uses
        // its published types wherever annotations are absent (annotations
        // always win), and inference contributes its own diagnostics
        // (LB0306) at the same strictness-mapped severity.
        let inference = infer::run(
            &artifacts.lowered,
            env,
            file,
            Exactness::from_strict(strictness == Strictness::Strict),
            InferMode::Check,
            externals.as_ref(),
        );
        diags.extend(check::run(
            parse,
            env,
            file,
            strictness == Strictness::Strict,
            is_meta,
            edition,
            inference.view(),
        ));
        diags.extend(inference.diags);
        // Duplicate `---@field` on one class (luals `duplicate-doc-field`,
        // LB0311) — a per-file doc-consistency finding, so it is emitted here
        // alongside the type diagnostics and suppressed under `None` like them.
        // `ambient` matters to the key identity: a defs-declared alias in an
        // indexer key must collide with its expansion, and only the ambient
        // layer can resolve it (`duplicate_doc_fields_with_ambient`'s doc).
        diags.extend(check::duplicate_doc_fields_with_ambient(
            items, file, ambient,
        ));
    }

    // Honor luals' `---@diagnostic disable*: <rule>` for the checker
    // diagnostics that carry a luals rule name (`undefined-field` → LB0306,
    // `deprecated` → LB0308, `discard-returns` → LB0309, `duplicate-doc-field`
    // → LB0311, `cyclic-class-ancestry` → LB0318, ...). One scan serves them
    // all; see `directive.rs` for why it does not reuse the linter's engine.
    if diags
        .iter()
        .any(|d| directive::rule_for_code(d.code).is_some())
    {
        let source = parse.syntax().text().to_string();
        let sup = directive::DirectiveScan::scan(&source);
        // Worth building the `LineIndex` + running the per-diagnostic scan
        // when either this file's own directives might suppress something,
        // OR a cross-file resolver is in reach: a diagnostic can carry a
        // primary label belonging to a file OTHER than `file` (G1 — see
        // `check_file_with_artifacts_and_sources`'s doc comment), which this
        // file's own (empty) `sup` would never see, but a foreign file's
        // directives still might suppress.
        if sup.any() || other_sources.is_some() {
            // One table for the file: a newline count from byte 0 per
            // diagnostic is O(diagnostics x file size).
            let lines = LineIndex::new(&source);
            // Lazily built per foreign file a diagnostic's primary label
            // actually names — most files never trip this at all, and a
            // project with many files must not pay for scanning every one
            // just because one of them has a suppressible diagnostic.
            let mut foreign: HashMap<String, (directive::DirectiveScan, LineIndex)> =
                HashMap::new();
            diags.retain(|d| {
                let Some(rule) = directive::rule_for_code(d.code) else {
                    return true;
                };
                let Some(label) = d.primary_label() else {
                    return true;
                };
                if label.span.file == file {
                    let line = lines.line_of(label.span.range.start);
                    return !sup.suppresses(rule, line);
                }
                // A cross-file primary label (`cross_file_class_decl_span`):
                // suppression-check it against ITS OWN declaring file's
                // directives and line index, never this file's — reusing
                // `lines` (built from `file`'s source) against another
                // file's byte offsets is exactly the bug this fixes.
                let Some(resolve) = other_sources else {
                    // No resolver: cannot determine correctly. Never
                    // suppress rather than guess with the wrong file's line
                    // index — the safe default (see the doc comment on
                    // `check_file_with_artifacts_and_sources`).
                    return true;
                };
                let (foreign_sup, foreign_lines) =
                    foreign.entry(label.span.file.clone()).or_insert_with(|| {
                        let text = resolve(&label.span.file).unwrap_or_default();
                        let scan = directive::DirectiveScan::scan(&text);
                        let idx = LineIndex::new(&text);
                        (scan, idx)
                    });
                let line = foreign_lines.line_of(label.span.range.start);
                !foreign_sup.suppresses(rule, line)
            });
        }
    }

    // Collapse cascades: an undefined-field read (LB0306) makes the
    // expression `unknown`, and that `unknown` then mismatches wherever the
    // value flows (LB0300 at an annotated boundary spanning the same
    // expression). One mistake, one diagnostic — keep the specific LB0306
    // and drop the LB0300 whose reported range contains it.
    let absent_ranges: Vec<std::ops::Range<usize>> = diags
        .iter()
        .filter(|d| d.code == FIELD_NOT_FOUND)
        .filter_map(|d| d.primary_label().map(|l| l.span.range.clone()))
        .collect();
    if !absent_ranges.is_empty() {
        diags.retain(|d| {
            if d.code != TYPE_MISMATCH {
                return true;
            }
            let Some(label) = d.primary_label() else {
                return true;
            };
            let range = &label.span.range;
            !absent_ranges
                .iter()
                .any(|a| range.start <= a.start && a.end <= range.end)
        });
    }

    diags.sort_by_key(|d| d.primary_label().map_or(0, |l| l.span.range.start));
    diags
}

// The unit tests that reach crate-internal API. Everything exercising only the
// public surface lives in `tests/` as integration files, split by topic; these
// cannot follow because they reach past it:
//
// - `defs::Ambient::build` — a bare ambient layer built from definition sources
//   *without* the dialect stdlib beneath it. The public `build_ambient` always
//   includes the stdlib, so these fixtures cannot be expressed through it.
// - `TypeEnv::function` — the lowered signature of one function by name, the
//   direct assertion on `---@vararg` lowering.
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

    fn ambient_codes(src: &str, defs: &[&str]) -> Vec<String> {
        let ambient = crate::defs::Ambient::build(defs);
        let parse = parse(src, Dialect::Lua54);
        assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
        check_file_with_ambient(
            &parse,
            "test.lua",
            Strictness::Warn,
            Dialect::Lua54,
            Some(&ambient),
        )
        .iter()
        .map(|d| d.code.to_string())
        .collect()
    }

    #[test]
    fn legacy_vararg_tag_lowers_into_func_varargs() {
        // Direct check of the wiring in `TypeEnv::attach_function` (env.rs):
        // a bare `---@vararg Type` must populate `FunctionTy::varargs`
        // exactly like `---@param ... Type` does.
        let src = "\
---@vararg number
local function f(...) end
";
        let parsed = parse(src, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        let env = TypeEnv::build(&parsed);
        let func = env.function("f").expect("f should have a signature");
        assert_eq!(func.varargs, Some(crate::ty::Ty::Number));
        assert!(func.params.is_empty());
    }

    #[test]
    fn cyclic_alias_across_files_reported() {
        // #110: `---@alias` names are workspace-global, so a cycle formed
        // between two project files' aliases is caught the same way as one
        // written in a single file — mirrors the `LB0312` cross-file test's
        // `module_surface` + `with_project_types` harness.
        let a = parse("---@alias A B\n", Dialect::Lua54);
        let a_surface = module_surface(&a, "a.lua", None);
        let b = parse("---@alias B A\n", Dialect::Lua54);
        let b_surface = module_surface(&b, "b.lua", None);
        let ambient = crate::defs::Ambient::build(&[])
            .with_project_types([&a_surface.types, &b_surface.types]);
        let consumer = parse("---@param x A\nlocal function f(x) end\n", Dialect::Lua54);
        let diags = check_file_with_ambient(
            &consumer,
            "consumer.lua",
            Strictness::Strict,
            Dialect::Lua54,
            Some(&ambient),
        );
        let codes: Vec<String> = diags.iter().map(|d| d.code.to_string()).collect();
        assert_eq!(codes, vec!["LB0314"]);
    }

    #[test]
    fn deprecated_cross_file_via_defs() {
        let defs = "\
---@meta
---@deprecated
function oldGlobal() end
";
        let src = "oldGlobal()\n";
        assert_eq!(ambient_codes(src, &[defs]), vec!["LB0308"]);
    }

    #[test]
    fn nodiscard_cross_file_via_defs() {
        let defs = "\
---@meta
---@nodiscard
---@return boolean
function mustUse() end
";
        let src = "mustUse()\n";
        assert_eq!(ambient_codes(src, &[defs]), vec!["LB0309"]);
    }

    #[test]
    fn async_cross_file_via_defs() {
        // The `---@async` flag rides the signature through a `[types] defs`
        // package, so a sync call to a def-declared async function is flagged.
        let defs = "\
---@meta
---@async
function fetchGlobal() end
";
        let src = "\
local function sync()
  fetchGlobal()
end
";
        assert_eq!(ambient_codes(src, &[defs]), vec!["LB0316"]);
    }

    #[test]
    fn package_blocked_cross_file_via_defs() {
        // The class (and its `package` member) is declared in a `---@meta` defs
        // package; the consumer file is a different file, so the member is
        // invisible there.
        let defs = "\
---@meta
---@class Config
---@field package secret string
";
        let src = "\
---@param c Config
local function read(c)
  return c.secret
end
";
        assert_eq!(ambient_codes(src, &[defs]), vec!["LB0312"]);
    }

    #[test]
    fn private_cross_file_via_workspace_global_class() {
        // A `---@class` (with a private member) declared in one project file is
        // workspace-global — nameable and enforced from another file. Its
        // visibility rides the shared class surface: the private member is
        // invisible in the consumer, which is not one of the class's methods.
        let producer = parse(
            "---@class Foo\n---@field private secret number\nlocal Foo = {}\nreturn Foo\n",
            Dialect::Lua54,
        );
        let surface = module_surface(&producer, "foo.lua", None);
        let ambient = crate::defs::Ambient::build(&[]).with_project_types([&surface.types]);
        let consumer = parse(
            "---@param f Foo\nlocal function read(f)\n  return f.secret\nend\n",
            Dialect::Lua54,
        );
        let diags = check_file_with_ambient(
            &consumer,
            "consumer.lua",
            Strictness::Strict,
            Dialect::Lua54,
            Some(&ambient),
        );
        let codes: Vec<String> = diags.iter().map(|d| d.code.to_string()).collect();
        assert_eq!(codes, vec!["LB0312"]);
    }
}
