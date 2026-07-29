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
pub mod ty;
mod version;

pub use assign::{Exactness, assignable};
pub use defs::{
    Ambient, DefFile, alias_collisions, build_ambient, build_ambient_checked, stdlib as stdlib_defs,
};
pub use env::{FileTypes, TypeEnv};
pub use infer::{ExternalTypes, InferredBinding, InferredReturn};
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
    let items = &artifacts.items;
    let env = TypeEnv::build_from_items(parse, items, ambient);
    let outcome = infer::run(
        &artifacts.lowered,
        &env,
        file,
        Exactness::Strict,
        InferMode::Check,
        None,
    );
    let types = FileTypes::collect(items, &env, &outcome.carrier_class_final);
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

    let env = TypeEnv::build_from_items(parse, items, ambient);
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
            &env,
            file,
            Exactness::from_strict(strictness == Strictness::Strict),
            InferMode::Check,
            externals.as_ref(),
        );
        diags.extend(check::run(
            parse,
            &env,
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
        diags.extend(check::duplicate_doc_fields(items, file));
    }

    // Honor luals' `---@diagnostic disable*: <rule>` for the checker
    // diagnostics that carry a luals rule name (`undefined-field` → LB0306,
    // `deprecated` → LB0308, `discard-returns` → LB0309, `duplicate-doc-field`
    // → LB0311). One scan serves them all; see `directive.rs` for why it does
    // not reuse the linter's engine.
    if diags
        .iter()
        .any(|d| directive::rule_for_code(d.code).is_some())
    {
        let source = parse.syntax().text().to_string();
        let sup = directive::DirectiveScan::scan(&source);
        if sup.any() {
            // One table for the file: a newline count from byte 0 per
            // diagnostic is O(diagnostics x file size).
            let lines = LineIndex::new(&source);
            diags.retain(|d| {
                let Some(rule) = directive::rule_for_code(d.code) else {
                    return true;
                };
                let line = d
                    .primary_label()
                    .map_or(0, |l| lines.line_of(l.span.range.start));
                !sup.suppresses(rule, line)
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
