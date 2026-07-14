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
//! (`lua_diagnostics`) each build the registry + merged ambient from the
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
// Back-compat alias: `luabox-lsp` (`server.rs`) and `luabox-cli`'s lint command
// (`lint_cmd.rs`) still call `combined_defs`. Kept as a thin re-export delegating
// to `build_ambient` so those crates compile unchanged; remove once they migrate.
// TODO(follow-up): migrate luabox-lsp + lint_cmd to `build_ambient` and drop this.
pub use defs::build_ambient as combined_defs;
pub use env::{FileTypes, TypeEnv};
pub use infer::{ExternalTypes, InferredBinding, InferredReturn};
pub use version::VersionReq;

use std::collections::HashMap;

use luabox_diag::Diagnostic;
use luabox_syntax::{lua, luacats};
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
    let outcome = infer::run(&lowered, &env, file, false, true, externals);
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
    let items = luacats::harvest(parse);
    let env = TypeEnv::build_from_items(parse, &items, ambient);
    let lowered = luabox_hir::lower(parse);
    let outcome = infer::run(&lowered, &env, file, true, false, None);
    let types = FileTypes::collect(&items, &env, &outcome.carrier_class_final);
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
    luabox_hir::lower(parse)
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
    let items = luacats::harvest(parse);
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

    let env = TypeEnv::build_from_items(parse, &items, ambient);
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
    let mut inferred_types = std::collections::HashMap::new();
    if strictness != Strictness::None {
        // Rich table inference (SPEC.md §3) runs first: the checker uses
        // its published types wherever annotations are absent (annotations
        // always win), and inference contributes its own diagnostics
        // (LB0306) at the same strictness-mapped severity.
        let lowered = luabox_hir::lower(parse);
        let inference = infer::run(
            &lowered,
            &env,
            file,
            strictness == Strictness::Strict,
            false,
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
        diags.extend(check::duplicate_doc_fields(&items, file));
        inferred_types = inference.expr_types;
    }
    let _ = inferred_types;

    // Honor luals' `---@diagnostic disable*: <rule>` for the checker
    // diagnostics that carry a luals rule name (`undefined-field` → LB0306,
    // `deprecated` → LB0308, `discard-returns` → LB0309, `duplicate-doc-field`
    // → LB0311). One scan serves them all; see `directive.rs` for why it does
    // not reuse the linter's engine.
    if diags
        .iter()
        .any(|d| directive::rule_for_code(&d.code.to_string()).is_some())
    {
        let source = parse.syntax().text().to_string();
        let sup = directive::DirectiveScan::scan(&source);
        if sup.any() {
            diags.retain(|d| {
                let code = d.code.to_string();
                let Some(rule) = directive::rule_for_code(&code) else {
                    return true;
                };
                let line = d
                    .primary_label()
                    .map_or(0, |l| directive::line_of(&source, l.span.range.start));
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
        .filter(|d| d.code.to_string() == "LB0306")
        .filter_map(|d| d.primary_label().map(|l| l.span.range.clone()))
        .collect();
    if !absent_ranges.is_empty() {
        diags.retain(|d| {
            if d.code.to_string() != "LB0300" {
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

#[cfg(test)]
// test code — panics document assumptions
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]
mod tests {
    use luabox_diag::Severity;
    use luabox_syntax::lua::{Dialect, parse};

    use super::*;

    fn check(source: &str, strictness: Strictness) -> Vec<Diagnostic> {
        let parse = parse(source, Dialect::Lua54);
        assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
        check_file(&parse, "test.lua", strictness, Dialect::Lua54)
    }

    fn codes(source: &str, strictness: Strictness) -> Vec<String> {
        check(source, strictness)
            .iter()
            .map(|d| d.code.to_string())
            .collect()
    }

    fn strict_codes(source: &str) -> Vec<String> {
        codes(source, Strictness::Strict)
    }

    // --- rule a: call sites -------------------------------------------

    #[test]
    fn call_argument_type_mismatch() {
        let src = "\
---@param n number
local function double(n)
  return n * 2
end
double(\"nope\")
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn call_with_matching_literal_is_clean() {
        let src = "\
---@param n number
---@param s string
local function f(n, s) end
f(2, \"ok\")
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn arity_too_few_and_too_many() {
        let src = "\
---@param a number
---@param b number
local function f(a, b) end
f(1)
f(1, 2, 3)
";
        assert_eq!(strict_codes(src), vec!["LB0301", "LB0301"]);
    }

    #[test]
    fn overload_return_type_flows_to_call_result() {
        // Locks the already-working #86 behaviour: the matching `---@overload`
        // governs the call's result type. `f("hi")` matches the string
        // overload => `string`, satisfying a `string` parameter cleanly; the
        // primary-matching `f(1)` yields `number` and misuses it.
        let ok = "\
---@overload fun(x: string): string
---@param x number
---@return number
local function f(x) end
---@param s string
local function want(s) end
want(f(\"hi\"))
";
        assert_eq!(strict_codes(ok), Vec::<String>::new());
        let bad = "\
---@overload fun(x: string): string
---@param x number
---@return number
local function f(x) end
---@param s string
local function want(s) end
want(f(1))
";
        assert_eq!(strict_codes(bad), vec!["LB0300"]);
    }

    #[test]
    fn call_matching_an_overload_is_clean() {
        // The 2-arg string overload accepts `f("x", "y")`; no diagnostic even
        // though the primary is a 1-arg number form.
        let src = "\
---@overload fun(a: string, b: string)
---@param a number
local function f(a) end
f(\"x\", \"y\")
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn no_match_reports_against_closest_overload() {
        // Primary `fun(a: number)`, overload `fun(a: string, b: string)`. The
        // 2-arg call `f("x", 42)` fits the overload's arity and its first arg,
        // so it is reported against the overload (`expected string` on the
        // second arg) rather than the primary (which would say "takes 1
        // argument").
        let src = "\
---@overload fun(a: string, b: string)
---@param a number
local function f(a) end
f(\"x\", 42)
";
        let diags = check(src, Strictness::Strict);
        assert_eq!(
            diags.iter().map(|d| d.code.to_string()).collect::<Vec<_>>(),
            vec!["LB0300"],
            "should be a type mismatch against the overload, not an arity error"
        );
        let diag = &diags[0];
        assert!(
            diag.message.contains("expected `string`"),
            "message should refer to the string overload's param: {}",
            diag.message
        );
        assert_eq!(diag.labels[0].message, "expected `string`");
    }

    #[test]
    fn optional_params_and_varargs_relax_arity() {
        let src = "\
---@param a number
---@param b? number
local function f(a, b) end
---@param a number
---@param ... string
local function g(a, ...) end
f(1)
f(1, 2)
g(1)
g(1, \"x\", \"y\")
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn vararg_arguments_are_typechecked() {
        let src = "\
---@param a number
---@param ... string
local function g(a, ...) end
g(1, \"x\", 2)
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn nil_satisfies_optional_param() {
        let src = "\
---@param a number
---@param b? string
local function f(a, b) end
f(1, nil)
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn annotated_local_flows_into_call() {
        let src = "\
---@param n number
local function f(n) end
---@type string
local s = \"x\"
f(s)
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn annotated_call_result_flows_into_call() {
        let src = "\
---@return string
local function name() return \"x\" end
---@param n number
local function f(n) end
f(name())
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn function_reference_argument() {
        let src = "\
---@param cb fun(x: number)
local function on(cb) end
---@param x number
local function handler(x) end
on(handler)
on(3)
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn multi_return_expansion_fills_arity() {
        let src = "\
---@return number, string
local function pair() return 1, \"x\" end
---@param a number
---@param b string
local function f(a, b) end
f(pair())
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn unknown_callee_is_never_checked() {
        assert_eq!(strict_codes("print(1, 2, 3)\n"), Vec::<String>::new());
    }

    #[test]
    fn dotted_function_names_resolve() {
        let src = "\
local M = {}
---@param n number
function M.double(n)
  return n * 2
end
M.double(\"no\")
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    // --- table literals against class shapes ----------------------------

    const POINT: &str = "\
---@class Point
---@field x number
---@field y number
---@field label? string

---@param p Point
local function use(p) end
";

    #[test]
    fn table_literal_missing_required_field() {
        let src = format!("{POINT}use({{ x = 1 }})\n");
        let diags = check(&src, Strictness::Strict);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].code.to_string(), "LB0302");
        assert!(diags[0].message.contains("`y`"), "{}", diags[0].message);
    }

    #[test]
    fn table_literal_unknown_field() {
        let src = format!("{POINT}use({{ x = 1, y = 2, z = 3 }})\n");
        let diags = check(&src, Strictness::Strict);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].code.to_string(), "LB0303");
        assert!(diags[0].message.contains("`z`"), "{}", diags[0].message);
    }

    #[test]
    fn table_literal_field_type_mismatch() {
        let src = format!("{POINT}use({{ x = 1, y = \"two\" }})\n");
        let diags = check(&src, Strictness::Strict);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].code.to_string(), "LB0300");
    }

    #[test]
    fn table_literal_optional_field_may_be_absent() {
        let src = format!("{POINT}use({{ x = 1, y = 2 }})\n");
        assert_eq!(strict_codes(&src), Vec::<String>::new());
    }

    #[test]
    fn each_field_problem_is_its_own_diagnostic() {
        let src = format!("{POINT}use({{ y = \"two\", z = 3 }})\n");
        let mut found: Vec<String> = strict_codes(&src);
        found.sort();
        assert_eq!(found, vec!["LB0300", "LB0302", "LB0303"]);
    }

    #[test]
    fn inherited_fields_count() {
        let src = "\
---@class Base
---@field id number

---@class Derived: Base
---@field name string

---@param d Derived
local function f(d) end
f({ name = \"x\" })
f({ id = 1, name = \"x\" })
";
        assert_eq!(strict_codes(src), vec!["LB0302"]);
    }

    #[test]
    fn indexer_keeps_class_open() {
        let src = "\
---@class Bag
---@field size number
---@field [string] boolean

---@param b Bag
local function f(b) end
f({ size = 1, extra = true })
f({ size = 1, extra = 3 })
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn array_items_checked_against_array_part() {
        let src = "\
---@param xs string[]
local function f(xs) end
f({ \"a\", \"b\" })
f({ \"a\", 2 })
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn nested_table_literal_checked_field_by_field() {
        let src = "\
---@class Inner
---@field n number

---@class Outer
---@field inner Inner

---@param o Outer
local function f(o) end
f({ inner = { n = \"no\" } })
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    // --- rule b: annotated locals ----------------------------------------

    #[test]
    fn typed_local_initializer_checked() {
        let src = "\
---@type number
local n = \"nope\"
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn typed_local_assignment_checked() {
        let src = "\
---@type number
local n = 1
n = \"nope\"
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn untyped_local_assignment_unchecked() {
        let src = "\
local n = 1
n = \"fine\"
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn typed_local_table_literal_field_level() {
        let src = "\
---@class Cfg
---@field port number

---@type Cfg
local cfg = { port = \"8080\" }
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    // --- rule c: returns ---------------------------------------------------

    #[test]
    fn return_type_mismatch() {
        let src = "\
---@return number
local function f()
  return \"nope\"
end
";
        assert_eq!(strict_codes(src), vec!["LB0304"]);
    }

    #[test]
    fn return_count_mismatch() {
        let src = "\
---@return number, string
local function f()
  return 1
end
---@return number
local function g()
  return 1, 2
end
";
        assert_eq!(strict_codes(src), vec!["LB0304", "LB0304"]);
    }

    #[test]
    fn nilable_missing_returns_are_fine() {
        let src = "\
---@return number, string?
local function f()
  return 1
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn nested_unannotated_function_returns_unchecked() {
        let src = "\
---@return number
local function f()
  local g = function()
    return \"inner is fine\"
  end
  g()
  return 1
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn annotated_param_flows_into_return_check() {
        let src = "\
---@param s string
---@return number
local function f(s)
  return s
end
";
        assert_eq!(strict_codes(src), vec!["LB0304"]);
    }

    // --- name-based `@param` binding (#74) --------------------------------

    #[test]
    fn partial_param_annotations_bind_by_name() {
        // Annotating only params 2..5 of a 6-param function must not shift
        // the tags onto the wrong positions (#74's exact repro).
        let src = "\
---@param b string
---@param c boolean
---@param d integer
---@param e string
local function f(a, b, c, d, e, g)
  return a, b, c, d, e, g
end
f(1, \"s\", true, 2, \"t\", 3)
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
        // And a call violating the *named* params errors per slot.
        let bad = src.replace(
            "f(1, \"s\", true, 2, \"t\", 3)",
            "f(1, true, \"s\", 2.5, 2, 3)",
        );
        assert_eq!(
            strict_codes(&bad),
            vec!["LB0300", "LB0300", "LB0300", "LB0300"]
        );
    }

    #[test]
    fn out_of_order_param_tags_bind_by_name() {
        let src = "\
---@param b string
---@param a number
local function f(a, b) end
f(1, \"s\")
f(\"s\", 1)
";
        assert_eq!(strict_codes(src), vec!["LB0300", "LB0300"]);
    }

    #[test]
    fn vararg_param_tag_binds_regardless_of_position() {
        let src = "\
---@param ... string
---@param a number
local function f(a, ...) end
f(1, \"x\", \"y\")
f(1, \"x\", 2)
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    // --- legacy `---@vararg Type` (deprecated EmmyLua spelling of
    // `---@param ... Type`) ---------------------------------------------

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
    fn legacy_vararg_tag_alone_types_dots() {
        // `---@vararg string` alone must type `...` exactly like
        // `---@param ... string` would: extra call args are checked against
        // it, so a non-string extra arg is a type mismatch.
        let src = "\
---@vararg string
local function f(...) end
f(\"x\", \"y\")
f(\"x\", 2)
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn legacy_vararg_tag_with_fixed_params() {
        // `---@vararg` combines with ordinary `---@param` tags on fixed
        // (non-vararg) parameters just like the modern `---@param ...` form
        // does in `vararg_param_tag_binds_regardless_of_position` above.
        let src = "\
---@param a number
---@vararg string
local function f(a, ...) end
f(1, \"x\", \"y\")
f(1, \"x\", 2)
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn legacy_vararg_tag_and_param_dots_union_per_luals() {
        // luals precedent (lua-language-server, `script/vm/compiler.lua`,
        // the `'...'`-source case): it loops over every doc bound to the AST
        // vararg node — `script/parser/luadoc.lua`'s `bindDoc` binds *both* a
        // `doc.vararg` tag and a `doc.param` tag whose `param[1] == '...'` to
        // that same source — and calls `vm.setNode(source, ...)` for each
        // one. `vm.setNode` without a `cover` argument *merges* (unions) into
        // the existing node rather than overwriting it (`script/vm/node.lua`,
        // `vm.setNode`). So when a block carries both a legacy `---@vararg`
        // and a modern `---@param ...`, luals's effective vararg type is the
        // union of the two — not "last tag wins" and not "param form wins".
        // We match that: string|number here, so a `boolean` extra arg is the
        // only misuse.
        let src = "\
---@vararg string
---@param ... number
local function f(...) end
f(\"x\")
f(1)
f(true)
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn duplicate_param_tag_names_first_wins() {
        let src = "\
---@param a number
---@param a string
local function f(a) end
f(1)
f(\"x\")
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn param_tag_naming_no_parameter_is_unbound() {
        // TODO(P2): LuaLS warns here; today the tag is silently unbound and
        // the parameter stays permissive `unknown`.
        let src = "\
---@param sied number
local function f(side)
  return side
end
f(1)
f(\"anything\")
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    // --- constructor returns through `setmetatable` (#73) ------------------

    /// Strict check with the Lua 5.4 stdlib ambient layer (its
    /// `setmetatable` signature must not mask inference).
    fn strict_codes_ambient(src: &str) -> Vec<String> {
        let parse = lua::parse(src, lua::Dialect::Lua54);
        assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
        check_file_with_ambient(
            &parse,
            "test.lua",
            Strictness::Strict,
            lua::Dialect::Lua54,
            Some(stdlib_defs(lua::Dialect::Lua54)),
        )
        .iter()
        .map(|d| d.code.to_string())
        .collect()
    }

    const CIRCLE_CLASS: &str = "\
---@class Circle
---@field radius number
local Circle = {}
Circle.__index = Circle

function Circle:area()
  return self.radius * self.radius
end
";

    #[test]
    fn constructor_setmetatable_satisfies_declared_class_return() {
        let src = format!(
            "{CIRCLE_CLASS}
---@param radius number
---@return Circle
function Circle.new(radius)
  return setmetatable({{ radius = radius }}, Circle)
end
"
        );
        assert_eq!(strict_codes_ambient(&src), Vec::<String>::new());
    }

    #[test]
    fn wrong_constructor_return_still_errors() {
        // Returning something that is NOT the declared class must stay an
        // error: a plain number...
        let src = format!(
            "{CIRCLE_CLASS}
---@return Circle
function Circle.new()
  return 42
end
"
        );
        assert_eq!(strict_codes_ambient(&src), vec!["LB0304"]);
        // ...and an instance of an unrelated class missing the fields.
        let src = format!(
            "{CIRCLE_CLASS}
---@class Empty
local Empty = {{}}
Empty.__index = Empty

---@return Circle
function Circle.new()
  return setmetatable({{}}, Empty)
end
"
        );
        assert_eq!(strict_codes_ambient(&src), vec!["LB0304"]);
    }

    #[test]
    fn annotated_instance_resolves_declared_fields_and_methods() {
        let src = format!(
            "{CIRCLE_CLASS}
---@param radius number
---@return Circle
function Circle.new(radius)
  return setmetatable({{ radius = radius }}, Circle)
end

---@param n number
local function wantn(n) end

local c = Circle.new(2)
wantn(c.radius)
wantn(c:area())
"
        );
        assert_eq!(strict_codes_ambient(&src), Vec::<String>::new());
    }

    #[test]
    fn self_in_class_methods_uses_declared_field_types() {
        // The declaration wins over the inferred constructor value: `count`
        // is declared `integer`, so `string.rep` (integer count) accepts it
        // even though the constructor stored a plain `number` param.
        let src = "\
---@class Banner
---@field count integer
local Banner = {}
Banner.__index = Banner

function Banner:draw()
  return string.rep(\"#\", self.count)
end

---@param count integer
---@return Banner
function Banner.new(count)
  return setmetatable({ count = count }, Banner)
end
";
        assert_eq!(strict_codes_ambient(src), Vec::<String>::new());
        // Negative: a declared `string` field stays a string — arithmetic
        // consumers of it error.
        let bad = "\
---@param n number
local function wantn(n) end

---@class Tag
---@field label string
local Tag = {}
Tag.__index = Tag

function Tag:size()
  wantn(self.label)
end
";
        assert_eq!(strict_codes_ambient(bad), vec!["LB0300"]);
    }

    // --- #118 `:` method-call checking through class shapes ------------------

    /// A class carrying a typed method and a `---@deprecated` method, plus a
    /// `---@return Class` constructor — the shared fixture for the `:`-call
    /// resolution tests.
    const WIDGET_CLASS: &str = "\
---@class Widget
---@field size number
local Widget = {}
Widget.__index = Widget

---@param w number
---@return number
function Widget:resize(w)
  return w
end

---@deprecated
function Widget:legacy() end

---@param n number
---@return Widget
function Widget.new(n)
  return setmetatable({ size = n }, Widget)
end
";

    #[test]
    fn method_call_wrong_arg_type_flagged() {
        let src = format!(
            "{WIDGET_CLASS}
local w = Widget.new(1)
w:resize(\"nope\")
"
        );
        assert_eq!(strict_codes_ambient(&src), vec!["LB0300"]);
    }

    #[test]
    fn method_call_correct_arg_is_clean() {
        let src = format!(
            "{WIDGET_CLASS}
local w = Widget.new(1)
w:resize(2)
"
        );
        assert_eq!(strict_codes_ambient(&src), Vec::<String>::new());
    }

    #[test]
    fn method_call_wrong_arity_flagged() {
        // The implicit `self` is not counted: `resize` takes one explicit
        // argument, so zero and two both mis-arity (LB0301).
        let src = format!(
            "{WIDGET_CLASS}
local w = Widget.new(1)
w:resize()
w:resize(2, 3)
"
        );
        assert_eq!(strict_codes_ambient(&src), vec!["LB0301", "LB0301"]);
    }

    #[test]
    fn deprecated_method_call_flagged() {
        let src = format!(
            "{WIDGET_CLASS}
local w = Widget.new(1)
w:legacy()
"
        );
        assert_eq!(strict_codes_ambient(&src), vec!["LB0308"]);
    }

    #[test]
    fn deprecated_method_call_is_warning_even_under_strict() {
        let src = format!(
            "{WIDGET_CLASS}
local w = Widget.new(1)
w:legacy()
"
        );
        let parse = lua::parse(&src, lua::Dialect::Lua54);
        assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
        let diags = check_file_with_ambient(
            &parse,
            "test.lua",
            Strictness::Strict,
            lua::Dialect::Lua54,
            Some(stdlib_defs(lua::Dialect::Lua54)),
        );
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code.to_string(), "LB0308");
        assert_eq!(diags[0].severity, Severity::Warning);
    }

    #[test]
    fn method_call_via_self_resolves_and_checks() {
        // Inside a method, `self` resolves to the declared class: a wrong-typed
        // `self:resize(...)` is checked, and `self:legacy()` flags deprecation.
        let src = format!(
            "{WIDGET_CLASS}
function Widget:grow()
  self:resize(\"bad\")
  self:legacy()
end
"
        );
        assert_eq!(strict_codes_ambient(&src), vec!["LB0300", "LB0308"]);
    }

    #[test]
    fn method_call_via_typed_local_resolves() {
        let src = format!(
            "{WIDGET_CLASS}
---@type Widget
local w
w:resize(\"bad\")
"
        );
        assert_eq!(strict_codes_ambient(&src), vec!["LB0300"]);
    }

    #[test]
    fn method_call_inherited_through_parent_resolves() {
        // A subclass instance reaches the parent's typed method through the
        // `__index` chain: `Derived` indexes into `Base`, so `d:resize(...)`
        // resolves to `Base.resize` and its argument is checked.
        let src = "\
---@class Base
local Base = {}
Base.__index = Base

---@param w number
function Base:resize(w) end

---@class Derived : Base
local Derived = setmetatable({}, { __index = Base })
Derived.__index = Derived

---@return Derived
function Derived.new()
  return setmetatable({}, Derived)
end

local d = Derived.new()
d:resize(\"bad\")
";
        assert_eq!(strict_codes_ambient(src), vec!["LB0300"]);
    }

    #[test]
    fn method_call_unknown_receiver_is_silent() {
        // An `any`/unknown receiver resolves to no class: nothing is reported,
        // even with an obviously wrong argument (mandatory conservatism).
        let src = format!(
            "{WIDGET_CLASS}
---@param x any
local function use(x)
  x:resize(\"whatever\")
end
"
        );
        assert_eq!(strict_codes_ambient(&src), Vec::<String>::new());
    }

    #[test]
    fn method_call_plain_table_receiver_is_silent() {
        // A plain inferred table with no declared class carries no method
        // signature: the `:` call is not checked.
        let src = "\
local t = {}
function t.thing(a, b) return a end
t:thing(1)
";
        assert_eq!(strict_codes_ambient(src), Vec::<String>::new());
    }

    #[test]
    fn method_call_unknown_method_adds_no_arg_finding() {
        // A method the class does not declare resolves to no signature, so the
        // argument checker stays silent — it manufactures no arity/type
        // finding. Inference's own `undefined-field` rule (#90, LB0306) owns
        // the "no such method" finding; the `:`-call arg check adds nothing.
        let src = format!(
            "{WIDGET_CLASS}
local w = Widget.new(1)
w:nonexistent(1, 2, 3)
"
        );
        let codes = strict_codes_ambient(&src);
        assert_eq!(codes, vec!["LB0306"]);
        assert!(!codes.iter().any(|c| c == "LB0300" || c == "LB0301"));
    }

    // --- enums, aliases, LB0305 -----------------------------------------

    #[test]
    fn enum_member_satisfies_enum_param() {
        let src = "\
---@enum Color
local Color = {
  red = 1,
  green = 2,
}
---@param c Color
local function paint(c) end
paint(Color.red)
paint(2)
paint(9)
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn alias_literal_union() {
        let src = "\
---@alias Mode \"fast\"|\"slow\"
---@param m Mode
local function run(m) end
run(\"fast\")
run(\"medium\")
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn alias_multiline_literal_quote_forms() {
        // #116: the multiline `---|` member forms `'"x"'`, `"x"`, and `'x'`
        // all denote the literal value `x` — the wrapping quotes are syntax,
        // not content — so every valid call is accepted and only the odd one
        // out is rejected.
        let src = "\
---@alias Level
---| '\"debug\"' # verbose
---| \"info\"
---| 'warn'

---@param l Level
local function set_level(l) end
set_level(\"debug\")
set_level(\"info\")
set_level(\"warn\")
set_level(\"nope\")
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    // --- LB0315: union/enum exhaustiveness ------------------------------

    #[test]
    fn nonexhaustive_union_if_is_flagged() {
        // `s : "a"|"b"|"c"`, only "a"/"b" handled, no else -> LB0315 naming "c".
        let src = "\
---@alias Color \"a\"|\"b\"|\"c\"
---@param s Color
local function f(s)
  if s == \"a\" then
    return 1
  elseif s == \"b\" then
    return 2
  end
end
";
        let diags = check(src, Strictness::Strict);
        let codes: Vec<String> = diags.iter().map(|d| d.code.to_string()).collect();
        assert_eq!(codes, vec!["LB0315"]);
        assert!(
            diags[0].message.contains("`\"c\"`"),
            "message should name the uncovered member: {}",
            diags[0].message
        );
    }

    #[test]
    fn exhaustive_union_if_is_clean() {
        let src = "\
---@alias Color \"a\"|\"b\"|\"c\"
---@param s Color
local function f(s)
  if s == \"a\" then
    return 1
  elseif s == \"b\" then
    return 2
  elseif s == \"c\" then
    return 3
  end
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn union_if_with_else_is_clean() {
        let src = "\
---@alias Color \"a\"|\"b\"|\"c\"
---@param s Color
local function f(s)
  if s == \"a\" then
    return 1
  elseif s == \"b\" then
    return 2
  else
    return 0
  end
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn open_type_discriminant_is_never_flagged() {
        // A `string` discriminant is open-ended — never an exhaustiveness site.
        let src = "\
---@param s string
local function f(s)
  if s == \"a\" then
    return 1
  elseif s == \"b\" then
    return 2
  end
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn union_containing_a_nonliteral_is_never_flagged() {
        // `"a"|"b"|string` is not finite: the `string` member opens it up.
        let src = "\
---@alias Loose \"a\"|\"b\"|string
---@param s Loose
local function f(s)
  if s == \"a\" then
    return 1
  elseif s == \"b\" then
    return 2
  end
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn reversed_operand_order_is_analysed() {
        // `"a" == s` reads the same as `s == "a"`.
        let src = "\
---@alias Color \"a\"|\"b\"|\"c\"
---@param s Color
local function f(s)
  if \"a\" == s then
    return 1
  elseif \"b\" == s then
    return 2
  end
end
";
        assert_eq!(strict_codes(src), vec!["LB0315"]);
    }

    #[test]
    fn compound_branch_condition_bails() {
        // A non-`==`-literal branch aborts the analysis — no false positive.
        let src = "\
---@alias Color \"a\"|\"b\"|\"c\"
---@param s Color
---@param ready boolean
local function f(s, ready)
  if s == \"a\" then
    return 1
  elseif s == \"b\" and ready then
    return 2
  end
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn nonexhaustive_enum_if_is_flagged() {
        let src = "\
---@enum Dir
local Dir = { up = 1, down = 2, left = 3 }
---@param d Dir
local function step(d)
  if d == Dir.up then
    return 1
  elseif d == Dir.down then
    return 2
  end
end
";
        let diags = check(src, Strictness::Strict);
        let codes: Vec<String> = diags.iter().map(|d| d.code.to_string()).collect();
        assert_eq!(codes, vec!["LB0315"]);
        assert!(
            diags[0].message.contains("`Dir.left`"),
            "message should name the uncovered enum member: {}",
            diags[0].message
        );
    }

    #[test]
    fn exhaustive_enum_if_is_clean() {
        let src = "\
---@enum Dir
local Dir = { up = 1, down = 2 }
---@param d Dir
local function step(d)
  if d == Dir.up then
    return 1
  elseif d == Dir.down then
    return 2
  end
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn nonexhaustive_if_warns_in_warn_mode() {
        // Rides the strictness ladder: a warning in warn mode.
        let src = "\
---@alias Color \"a\"|\"b\"|\"c\"
---@param s Color
local function f(s)
  if s == \"a\" then
    return 1
  elseif s == \"b\" then
    return 2
  end
end
";
        let diags = check(src, Strictness::Warn);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code.to_string(), "LB0315");
        assert_eq!(diags[0].severity, Severity::Warning);
    }

    #[test]
    fn generic_alias_substitutes() {
        // #117: `Pair<number>` monomorphises `{ first: T, second: T }`, so the
        // string `second` is caught against `number`.
        let src = "\
---@alias Pair<T> { first: T, second: T }
---@type Pair<number>
local bad = { first = 1, second = \"two\" }
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn generic_alias_valid_instantiation() {
        let src = "\
---@alias Pair<T> { first: T, second: T }
---@type Pair<number>
local ok = { first = 1, second = 2 }
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn generic_alias_bare_reference_is_lenient() {
        // A bare `Pair` (no `<...>`) resolves its params to `unknown` — no
        // arity error, no field checking — matching luals and generic classes.
        let src = "\
---@alias Pair<T> { first: T, second: T }
---@type Pair
local anything = { first = 1, second = \"two\" }
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn generic_alias_arity_mismatch_reported() {
        // #117: an explicit `<...>` list of the wrong length is LB0313.
        let src = "\
---@alias Pair<T> { first: T, second: T }
---@type Pair<number, string>
local x = { first = 1, second = 2 }
";
        assert_eq!(strict_codes(src), vec!["LB0313"]);
    }

    #[test]
    fn generic_class_arity_mismatch_reported() {
        // #124: LB0313 extends to generic `---@class Name<T>` references — an
        // explicit `<...>` list of the wrong length is flagged. The initializer
        // matches the substituted shape so only the arity finding remains.
        let src = "\
---@class Box<T>
---@field value T

---@type Box<number, string>
local b = { value = 1 }
";
        assert_eq!(strict_codes(src), vec!["LB0313"]);
    }

    #[test]
    fn generic_class_bare_and_correct_arity_are_clean() {
        // A correct-arity reference and a bare reference (no `<...>`) both pass.
        let src = "\
---@class Box<T>
---@field value T

---@type Box<number>
local ok = { value = 1 }

---@type Box
local bare = { value = 2 }
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    // --- #123 cyclic `---@alias` (LB0314) ------------------------------------

    #[test]
    fn direct_cyclic_alias_reported_once() {
        // `---@alias Loop Loop` can never terminate expanding into something
        // real (`expand_alias`'s cycle guard collapses it to `unknown`), so
        // it is LB0314 — reported once even though two annotations reference
        // it (never once per reference site).
        let src = "\
---@alias Loop Loop
---@param a Loop
---@param b Loop
local function f(a, b) end
";
        assert_eq!(strict_codes(src), vec!["LB0314"]);
    }

    #[test]
    fn mutual_cyclic_alias_reported() {
        // `---@alias A B` + `---@alias B A`: neither alias ever bottoms out,
        // so expanding the referenced one (`A`) trips the cycle guard.
        let src = "\
---@alias A B
---@alias B A
---@param x A
local function f(x) end
";
        assert_eq!(strict_codes(src), vec!["LB0314"]);
    }

    #[test]
    fn non_cyclic_alias_is_clean() {
        // The safe-termination and reporting changes must not touch an
        // ordinary alias used in a `---@param`.
        let src = "\
---@alias Status \"on\"|\"off\"
---@param s Status
local function f(s) end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
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
    fn unknown_type_name_reported() {
        let src = "\
---@param x Wibble
local function f(x) end
";
        assert_eq!(strict_codes(src), vec!["LB0305"]);
    }

    #[test]
    fn generic_params_do_not_report_lb0305() {
        let src = "\
---@generic T
---@param x T
---@return T
local function id(x)
  return x
end
id(1)
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    // --- strictness ladder -----------------------------------------------

    #[test]
    fn warn_mode_downgrades_severity() {
        let src = "\
---@param n number
local function f(n) end
f(\"no\")
";
        let warn = check(src, Strictness::Warn);
        assert_eq!(warn.len(), 1);
        assert_eq!(warn[0].severity, Severity::Warning);
        let strict = check(src, Strictness::Strict);
        assert_eq!(strict[0].severity, Severity::Error);
    }

    #[test]
    fn unknown_argument_strict_vs_warn() {
        let src = "\
---@param n number
local function f(n) end
local x = tonumber(\"3\")
f(x)
";
        // Warn: `unknown` flows freely. Strict: unknown -> number errors.
        assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
        assert_eq!(codes(src, Strictness::Strict), vec!["LB0300"]);
    }

    #[test]
    fn none_suppresses_everything() {
        let src = "\
---@param n number
local function f(n) end
f(\"no\")
";
        assert_eq!(codes(src, Strictness::None), Vec::<String>::new());
    }

    #[test]
    fn strictness_from_manifest_flag() {
        assert_eq!(Strictness::from_manifest_flag(true), Strictness::Strict);
        assert_eq!(Strictness::from_manifest_flag(false), Strictness::Warn);
    }

    // --- end to end ---------------------------------------------------------

    #[test]
    fn clean_annotated_file_has_no_diagnostics() {
        let src = "\
---@class Greeter
---@field name string
---@field excited? boolean

---@param g Greeter
---@return string
local function greet(g)
  return g.name
end

---@type Greeter
local g = { name = \"lua\" }
greet(g)
greet({ name = \"world\", excited = true })
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn diagnostics_carry_file_and_span() {
        let src = "\
---@param n number
local function f(n) end
f(\"no\")
";
        let diags = check(src, Strictness::Strict);
        let label = diags[0].primary_label().expect("primary label");
        assert_eq!(label.span.file, "test.lua");
        assert_eq!(&src[label.span.range.clone()], "\"no\"");
    }

    // --- #111 `---@deprecated` (LB0308) --------------------------------------

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
    fn deprecated_local_function_call_flagged() {
        let src = "\
---@deprecated
local function old() end
old()
";
        assert_eq!(codes(src, Strictness::Warn), vec!["LB0308"]);
    }

    #[test]
    fn deprecated_dotted_function_call_flagged() {
        let src = "\
local M = {}
---@deprecated
function M.legacy() end
M.legacy()
";
        assert_eq!(codes(src, Strictness::Warn), vec!["LB0308"]);
    }

    #[test]
    fn deprecated_value_reference_flagged() {
        let src = "\
---@deprecated
local function old() end
local alias = old
";
        // Referencing (not just calling) a deprecated function is a use.
        assert_eq!(codes(src, Strictness::Warn), vec!["LB0308"]);
    }

    #[test]
    fn deprecated_declaration_site_not_flagged() {
        let src = "\
---@deprecated
local function old() end
";
        // The declaration itself is never flagged — only uses are.
        assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
    }

    #[test]
    fn deprecated_is_warning_even_under_strict() {
        let src = "\
---@deprecated
local function old() end
old()
";
        let diags = check(src, Strictness::Strict);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Warning);
        assert_eq!(diags[0].code.to_string(), "LB0308");
    }

    #[test]
    fn deprecated_does_not_add_arity_errors() {
        // A bare `---@deprecated` block must not enable arity checking on an
        // otherwise unannotated function.
        let src = "\
---@deprecated
local function old(a, b) end
old(1, 2, 3, 4)
";
        assert_eq!(codes(src, Strictness::Warn), vec!["LB0308"]);
    }

    #[test]
    fn deprecated_suppressed_by_directive() {
        let src = "\
---@deprecated
local function old() end
---@diagnostic disable-next-line: deprecated
old()
";
        assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
    }

    #[test]
    fn deprecated_suppressed_file_wide() {
        let src = "\
---@diagnostic disable: deprecated
---@deprecated
local function old() end
old()
old()
";
        assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
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
    fn non_deprecated_call_is_clean() {
        let src = "\
local function ok() end
ok()
";
        assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
    }

    #[test]
    fn deprecated_cross_file_via_require() {
        // `Api.old` is deprecated in the required module; the flag rides the
        // module's export type into the consumer and flags the use site.
        let api = parse(
            "local Api = {}\n---@deprecated\nfunction Api.old() end\nreturn Api\n",
            Dialect::Lua54,
        );
        let export = module_surface(&api, "api.lua", None)
            .export
            .expect("module exports a table");
        let mut requires = HashMap::new();
        requires.insert("api".to_string(), export);
        let main = parse("local Api = require(\"api\")\nApi.old()\n", Dialect::Lua54);
        let diags = check_file_with_requires(
            &main,
            "main.lua",
            Strictness::Warn,
            Dialect::Lua54,
            None,
            &requires,
        );
        let codes: Vec<String> = diags.iter().map(|d| d.code.to_string()).collect();
        assert_eq!(codes, vec!["LB0308"]);
    }

    #[test]
    fn async_cross_file_via_require() {
        // `Api.fetch` is `---@async` in the required module; the flag rides
        // the module's export type into the consumer, so calling it from a
        // sync function flags LB0316 (`callee_sig` cannot resolve required
        // members — the `check_call` fallback through the resolved
        // expression type covers this, like `deprecated`/`version` do).
        let api = parse(
            "local Api = {}\n---@async\nfunction Api.fetch() end\nreturn Api\n",
            Dialect::Lua54,
        );
        let export = module_surface(&api, "api.lua", None)
            .export
            .expect("module exports a table");
        let mut requires = HashMap::new();
        requires.insert("api".to_string(), export);
        let main = parse(
            "local Api = require(\"api\")\nlocal function sync()\n  Api.fetch()\nend\n",
            Dialect::Lua54,
        );
        let diags = check_file_with_requires(
            &main,
            "main.lua",
            Strictness::Warn,
            Dialect::Lua54,
            None,
            &requires,
        );
        let codes: Vec<String> = diags.iter().map(|d| d.code.to_string()).collect();
        assert_eq!(codes, vec!["LB0316"]);
    }

    #[test]
    fn multiple_vararg_tags_union_per_luals() {
        // Two legacy `---@vararg` tags on one block are two bound docs in
        // luals, and every bound doc merges into the `...` node — so their
        // types union, exactly like a `---@vararg` next to a `---@param ...`.
        let src = "\
---@vararg number
---@vararg string
local function f(...) end
f(1)
f(\"x\")
f(true)
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    // --- #112 `---@nodiscard` (LB0309) ---------------------------------------

    #[test]
    fn nodiscard_bare_call_flagged() {
        let src = "\
---@nodiscard
---@return boolean
local function save() return true end
save()
";
        assert_eq!(codes(src, Strictness::Warn), vec!["LB0309"]);
    }

    #[test]
    fn nodiscard_bound_result_is_clean() {
        let src = "\
---@nodiscard
---@return boolean
local function save() return true end
local ok = save()
";
        assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
    }

    #[test]
    fn nodiscard_used_in_expression_is_clean() {
        let src = "\
---@nodiscard
---@return boolean
local function save() return true end
if save() then end
";
        assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
    }

    #[test]
    fn nodiscard_suppressed_by_directive() {
        let src = "\
---@nodiscard
---@return boolean
local function save() return true end
---@diagnostic disable-next-line: discard-returns
save()
";
        assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
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

    /// The `require`-export registry for a single module, for the cross-file
    /// nodiscard tests (mirrors [`deprecated_cross_file_via_require`]).
    fn require_registry(module: &str, src: &str) -> HashMap<String, Ty> {
        let parse = parse(src, Dialect::Lua54);
        assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
        let export = module_surface(&parse, "mod.lua", None)
            .export
            .expect("module exports a table");
        let mut requires = HashMap::new();
        requires.insert(module.to_string(), export);
        requires
    }

    #[test]
    fn nodiscard_cross_file_via_require() {
        // `old.important` is nodiscard in the required module; the flag rides
        // the module's export type into the consumer, so a bare call statement
        // in the consumer is a discard (#112 parity with LB0308's reach).
        let requires = require_registry(
            "old",
            "local M = {}\n---@nodiscard\n---@return number\nfunction M.important() return 2 end\nreturn M\n",
        );
        let main = parse(
            "local old = require(\"old\")\nold.important()\n",
            Dialect::Lua54,
        );
        let diags = check_file_with_requires(
            &main,
            "main.lua",
            Strictness::Warn,
            Dialect::Lua54,
            None,
            &requires,
        );
        let codes: Vec<String> = diags.iter().map(|d| d.code.to_string()).collect();
        assert_eq!(codes, vec!["LB0309"]);
    }

    #[test]
    fn nodiscard_cross_file_via_require_bound_is_clean() {
        // Binding the result accepts it — no discard, exactly as same-file.
        let requires = require_registry(
            "old",
            "local M = {}\n---@nodiscard\n---@return number\nfunction M.important() return 2 end\nreturn M\n",
        );
        let main = parse(
            "local old = require(\"old\")\nlocal n = old.important()\nprint(n)\n",
            Dialect::Lua54,
        );
        let diags = check_file_with_requires(
            &main,
            "main.lua",
            Strictness::Warn,
            Dialect::Lua54,
            None,
            &requires,
        );
        let codes: Vec<String> = diags.iter().map(|d| d.code.to_string()).collect();
        assert_eq!(codes, Vec::<String>::new());
    }

    // --- `---@async` await-in-sync (LB0316) ----------------------------------
    //
    // luals precedent: `script/core/diagnostics/await-in-sync.lua` flags a call
    // to an async function (`vm.isAsyncCall`) whose enclosing function
    // (`guide.getParentFunction`) is not async (`vm.isAsync`). A function is
    // async iff a `bindDocs` entry is `doc.async` (`script/vm/doc.lua`). The
    // main chunk is async (`vm.isAsync` returns true for `type == 'main'`), so
    // a top-level async call is not flagged. Default severity Warning, and
    // status None (disabled by default) in `script/proto/diagnostic.lua` —
    // luabox surfaces it at a fixed Warning like its siblings LB0308/LB0309.

    #[test]
    fn async_call_in_sync_function_flagged() {
        let src = "\
---@async
local function fetch() end
local function sync()
  fetch()
end
";
        assert_eq!(codes(src, Strictness::Warn), vec!["LB0316"]);
    }

    #[test]
    fn async_call_in_async_function_is_clean() {
        let src = "\
---@async
local function fetch() end
---@async
local function loadAll()
  fetch()
end
";
        assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
    }

    #[test]
    fn async_call_at_top_level_is_clean() {
        // The main chunk is an async context in luals, so a top-level call to
        // an async function is not flagged.
        let src = "\
---@async
local function fetch() end
fetch()
";
        assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
    }

    #[test]
    fn async_call_in_nested_sync_closure_flagged() {
        // A closure nested inside an async function is itself sync (unannotated),
        // so an async call within it is flagged — the *nearest* enclosing
        // function governs, matching `guide.getParentFunction`.
        let src = "\
---@async
local function fetch() end
---@async
local function loadAll()
  local cb = function()
    fetch()
  end
  cb()
end
";
        assert_eq!(codes(src, Strictness::Warn), vec!["LB0316"]);
    }

    #[test]
    fn async_dotted_call_flagged() {
        let src = "\
local M = {}
---@async
function M.fetch() end
local function sync()
  M.fetch()
end
";
        assert_eq!(codes(src, Strictness::Warn), vec!["LB0316"]);
    }

    #[test]
    fn async_method_call_flagged() {
        // A `:`-method resolved to an `---@async` signature, called from a
        // non-async function.
        let src = "\
---@class Client
local Client = {}
Client.__index = Client

---@async
function Client:fetch() end

---@return Client
function Client.new()
  return setmetatable({}, Client)
end

local function sync()
  local c = Client.new()
  c:fetch()
end
";
        assert_eq!(strict_codes_ambient(src), vec!["LB0316"]);
    }

    #[test]
    fn non_async_call_is_clean() {
        // A callee with no `---@async` tag is never flagged, in any context.
        let src = "\
local function plain() end
local function sync()
  plain()
end
";
        assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
    }

    #[test]
    fn async_in_sync_is_warning_even_under_strict() {
        // luals-Warning parity: LB0316 stays a Warning under strict mode, never
        // escalating on the strictness ladder (like LB0308/LB0309).
        let src = "\
---@async
local function fetch() end
local function sync()
  fetch()
end
";
        let diags = check(src, Strictness::Strict);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code.to_string(), "LB0316");
        assert_eq!(diags[0].severity, Severity::Warning);
    }

    #[test]
    fn async_in_sync_suppressed_by_directive() {
        let src = "\
---@async
local function fetch() end
local function sync()
  ---@diagnostic disable-next-line: await-in-sync
  fetch()
end
";
        assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
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
    fn async_bare_tag_adds_no_arity_error() {
        // A bare `---@async` block (no `---@param`/`---@return`) must not enable
        // arity checking on the function it annotates.
        let src = "\
---@async
local function fetch(a, b) end
fetch(1)
";
        assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
    }

    // --- #113 duplicate-doc-field (LB0311) -----------------------------------

    #[test]
    fn duplicate_doc_field_flagged() {
        let src = "\
---@class Point
---@field x number
---@field y number
---@field x integer
local Point = {}
";
        assert_eq!(codes(src, Strictness::Warn), vec!["LB0311"]);
    }

    #[test]
    fn distinct_fields_are_clean() {
        let src = "\
---@class Point
---@field x number
---@field y number
local Point = {}
";
        assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
    }

    #[test]
    fn duplicate_doc_field_suppressed_by_directive() {
        let src = "\
---@class Point
---@field x number
---@diagnostic disable-next-line: duplicate-doc-field
---@field x integer
local Point = {}
";
        assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
    }

    #[test]
    fn duplicate_doc_field_not_under_none() {
        let src = "\
---@class Point
---@field x number
---@field x integer
local Point = {}
";
        assert_eq!(codes(src, Strictness::None), Vec::<String>::new());
    }

    // --- #115 field visibility (LB0312 invisible) ----------------------------

    #[test]
    fn private_field_clean_inside_own_method() {
        // A `---@field private` read from the owning class's own method is
        // allowed — the receiver is `self`, whose class is the enclosing one.
        let src = "\
---@class Account
---@field private balance number
local Account = {}
Account.__index = Account

function Account:total()
  return self.balance
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn private_field_blocked_outside() {
        // The same read from a plain function is `invisible` — and it is
        // LB0312, not LB0306: the member exists, it is just not visible.
        let src = "\
---@class Account
---@field private balance number
local Account = {}
Account.__index = Account

---@param a Account
local function show(a)
  return a.balance
end
";
        assert_eq!(strict_codes(src), vec!["LB0312"]);
    }

    #[test]
    fn private_is_warning_in_warn_mode() {
        let src = "\
---@class Account
---@field private balance number
local Account = {}

---@param a Account
local function show(a)
  return a.balance
end
";
        let diags = check(src, Strictness::Warn);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code.to_string(), "LB0312");
        assert_eq!(diags[0].severity, Severity::Warning);
        // ...and an error under strict (stricter than luals, like LB0306).
        assert_eq!(check(src, Strictness::Strict)[0].severity, Severity::Error);
    }

    #[test]
    fn protected_clean_in_subclass_blocked_elsewhere() {
        let src = "\
---@class Base
---@field protected token? string
local Base = {}
Base.__index = Base

---@class Child : Base
local Child = {}
Child.__index = Child

function Child:reveal()
  return self.token
end

---@param b Base
local function leak(b)
  return b.token
end
";
        assert_eq!(strict_codes(src), vec!["LB0312"]);
    }

    #[test]
    fn private_not_visible_in_subclass() {
        // luals: private is same-class-only — a subclass method cannot read a
        // private parent member (protected would).
        let src = "\
---@class Base
---@field private secret? string
local Base = {}
Base.__index = Base

---@class Child : Base
local Child = {}
Child.__index = Child

function Child:peek()
  return self.secret
end
";
        assert_eq!(strict_codes(src), vec!["LB0312"]);
    }

    #[test]
    fn package_clean_same_file() {
        let src = "\
---@class Config
---@field package secret string
local Config = {}
Config.__index = Config

---@param c Config
local function read(c)
  return c.secret
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
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
    fn private_suppressed_by_directive() {
        let src = "\
---@class Account
---@field private balance number
local Account = {}

---@param a Account
local function show(a)
  ---@diagnostic disable-next-line: invisible
  return a.balance
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn private_suppressed_file_wide() {
        let src = "\
---@diagnostic disable: invisible
---@class Account
---@field private balance number
local Account = {}

---@param a Account
local function show(a)
  return a.balance
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn standalone_private_on_method() {
        // The tag-above-a-method form: `---@private function Class:m()`. The
        // method is reachable from the class's own methods, invisible outside.
        let src = "\
---@class Widget
local Widget = {}
Widget.__index = Widget

---@private
function Widget:_init() end

function Widget:render()
  self:_init()
end

---@param w Widget
local function use(w)
  w:_init()
end
";
        assert_eq!(strict_codes(src), vec!["LB0312"]);
    }

    #[test]
    fn visible_public_sibling_is_clean() {
        // A public member alongside a private one is never flagged.
        let src = "\
---@class Account
---@field private balance number
---@field owner string
local Account = {}

---@param a Account
local function show(a)
  return a.owner
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn public_redeclaration_in_subclass_reopens_member() {
        // A subclass re-declaring an inherited private member as a plain
        // `---@field` opens it back to public (nearest declaration wins).
        let src = "\
---@class Base
---@field private tag? string
local Base = {}

---@class Child : Base
---@field tag? string
local Child = {}

---@param c Child
local function read(c)
  return c.tag
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
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

    #[test]
    fn private_blocked_on_inferred_instance_receiver() {
        // The idiomatic case: a constructor result (an inference *instance*
        // shape, not an annotated `---@type`) still resolves its class, so a
        // private read on it from module scope is `invisible`.
        let src = "\
---@class Circle
---@field private radius number
local Circle = {}
Circle.__index = Circle

---@return Circle
function Circle.new()
  return setmetatable({ radius = 1 }, Circle)
end

local c = Circle.new()
print(c.radius)
";
        assert_eq!(strict_codes_ambient(src), vec!["LB0312"]);
    }

    #[test]
    fn standalone_private_on_bare_assign_carrier_blocked_outside() {
        // The bare-assignment carrier form: `---@private Carrier.method =
        // function() end`. luals resolves the class from the assignment base
        // (`vm.getDefinedClass` on the `setfield` node), so the member is
        // private to the class and invisible outside its methods.
        let src = "\
---@class Widget
local Widget = {}
Widget.__index = Widget

---@private
Widget.reset = function() end

---@param w Widget
local function use(w)
  return w.reset
end
";
        assert_eq!(strict_codes(src), vec!["LB0312"]);
    }

    #[test]
    fn standalone_private_on_bare_assign_carrier_clean_inside() {
        // Read from inside a `:` method of the same class — the enclosing class
        // context makes the private member visible (clean).
        let src = "\
---@class Widget
local Widget = {}
Widget.__index = Widget

---@private
Widget.reset = function() end

function Widget:render()
  return self.reset
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn standalone_private_from_inside_bare_assigned_method() {
        // Item 2 (access side): a method itself *defined* via the bare-assign
        // carrier form (`Widget.render = function(self) ... end`) that reads a
        // private member of its own class is judged in-class (clean). Both the
        // `function Widget:m()` and `Widget.m = function()` forms desugar to the
        // same HIR assignment, so the enclosing-class judgment is symmetric.
        let src = "\
---@class Widget
local Widget = {}
Widget.__index = Widget

---@private
Widget.reset = function() end

---@param self Widget
Widget.render = function(self)
  return self.reset
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn standalone_protected_on_bare_assign_carrier() {
        // `---@protected` variant: readable in the owning class's own method,
        // invisible from an unrelated function.
        let src = "\
---@class Widget
local Widget = {}
Widget.__index = Widget

---@protected
Widget.token = function() end

function Widget:reveal()
  return self.token
end

---@param w Widget
local function leak(w)
  return w.token
end
";
        assert_eq!(strict_codes(src), vec!["LB0312"]);
    }

    #[test]
    fn standalone_package_on_bare_assign_carrier() {
        // `---@package` variant: accessible anywhere in the file that declares
        // the class (clean same-file).
        let src = "\
---@class Config
local Config = {}
Config.__index = Config

---@package
Config.secret = function() end

---@param c Config
local function read(c)
  return c.secret
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn standalone_visibility_multi_target_assign_not_wired() {
        // Conservative: a visibility tag above a multi-target assignment
        // (`a.x, b.y = f, g`) is *not* wired — the base of a member does not
        // resolve to a single class, so nothing is marked private and the
        // outside read stays clean (no false `invisible`).
        let src = "\
---@class Widget
local Widget = {}
Widget.__index = Widget

local other = {}

---@private
Widget.reset, other.y = function() end, 1

---@param w Widget
local function use(w)
  return w.reset
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn standalone_visibility_on_non_carrier_base_unaffected() {
        // A bare assignment whose base is not a declared class carrier: the tag
        // finds no class to attach to, so nothing is marked private and the read
        // stays clean (no false `invisible`).
        let src = "\
---@class Widget
local Widget = {}
Widget.__index = Widget

local plain = {}

---@private
plain.reset = function() end

local x = plain.reset
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    // --- #122 callable classes (`---@operator call`) -------------------------

    const CALLABLE: &str = "\
---@class Callable
---@operator call(number): string
local M = {}
---@type Callable
local obj = M
";

    #[test]
    fn call_operator_makes_value_callable_result_flows() {
        // `obj(42)` yields `string`: assigning it to a `string` is clean, and
        // the arg (`number`) is satisfied by `42`.
        let src = format!(
            "{CALLABLE}\
---@type string
local s = obj(42)
"
        );
        assert_eq!(strict_codes(&src), Vec::<String>::new());
    }

    #[test]
    fn call_operator_result_misuse_is_flagged() {
        // The result is `string`, so assigning it to a `number` is LB0300 —
        // proving the operator's declared result type reaches the assignment.
        let src = format!(
            "{CALLABLE}\
---@type number
local n = obj(42)
"
        );
        assert_eq!(strict_codes(&src), vec!["LB0300"]);
    }

    #[test]
    fn call_operator_wrong_argument_type_is_flagged() {
        // The operator's input is `number`; a `string` argument is LB0300.
        let src = format!(
            "{CALLABLE}\
local x = obj(\"wrong\")
"
        );
        assert_eq!(strict_codes(&src), vec!["LB0300"]);
    }

    #[test]
    fn call_operator_result_flows_to_return() {
        // The result type also flows into a `---@return string` context.
        let src = format!(
            "{CALLABLE}\
---@return string
local function make()
  return obj(42)
end
"
        );
        assert_eq!(strict_codes(&src), Vec::<String>::new());
    }

    #[test]
    fn no_input_call_operator_accepts_any_arguments() {
        // A no-paren `call: T` operator is a permissive callable: any argument
        // list is accepted and the result is `T`.
        let src = "\
---@class NoIn
---@operator call: boolean
local N = {}
---@type NoIn
local n = N
---@type boolean
local a = n()
---@type boolean
local b = n(1, \"two\", {})
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn inherited_call_operator_type_checks_arguments() {
        // A subclass inherits its parent's `call` operator; a wrong-typed
        // argument is still flagged (LB0300).
        let src = "\
---@class Base
---@operator call(string): integer
local B = {}
---@class Derived : Base
local D = {}
---@type Derived
local d = D
local bad = d(99)
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn unresolved_callee_is_unchanged_no_new_diagnostic() {
        // An `any`/unknown callee never manufactures a call diagnostic: it is
        // not a declared class with a `call` operator (conservatism, AC #3).
        let src = "\
---@type any
local x = nil
local y = x(\"anything\")
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn class_without_call_operator_is_not_callable() {
        // A plain class value has no `call` operator: calling it stays exactly
        // as today (no synthesized signature, no new diagnostic).
        let src = "\
---@class Plain
local P = {}
---@type Plain
local p = P
local r = p(1, 2, 3)
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    // --- rule: contextual / bidirectional typing (#120) ----------------

    #[test]
    fn contextual_param_from_callback_flags_bad_field() {
        // A function-literal argument matched to a `---@param cb fun(w: Widget)`
        // types `w` as `Widget` with no annotation, so a bad field read inside
        // the lambda is flagged (LB0306) — the canonical bidirectional win.
        let src = "\
---@class Widget
---@field name string

---@param cb fun(w: Widget)
local function higher(cb) end

higher(function(w) return w.nofield end)
";
        assert_eq!(strict_codes(src), vec!["LB0306"]);
    }

    #[test]
    fn contextual_param_from_callback_correct_field_is_clean() {
        // The same shape, accessing a real field: `w` is `Widget`, `w.name`
        // resolves, nothing is flagged.
        let src = "\
---@class Widget
---@field name string

---@param cb fun(w: Widget)
local function higher(cb) end

higher(function(w) return w.name end)
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn contextual_param_unannotated_callee_is_unchanged() {
        // An UNANNOTATED higher-order function's callback parameter has no
        // expected type: `w` stays `unknown`, so the bad field read raises
        // nothing (conservatism, AC #3 — behavior exactly as before).
        let src = "\
---@class Widget
---@field name string

local function higher(cb) end

higher(function(w) return w.nofield end)
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn contextual_param_from_typed_local_seeds_lambda() {
        // A `---@type fun(w: Widget)` on a local seeds its function-literal
        // initializer's parameter, so the lambda body checks against Widget.
        let bad = "\
---@class Widget
---@field name string

---@type fun(w: Widget)
local f = function(w) return w.nofield end
";
        assert_eq!(strict_codes(bad), vec!["LB0306"]);

        let good = "\
---@class Widget
---@field name string

---@type fun(w: Widget)
local f = function(w) return w.name end
";
        assert_eq!(strict_codes(good), Vec::<String>::new());
    }

    #[test]
    fn contextual_return_typed_param_is_usable() {
        // `---@type fun(x: number): number` seeds `x` as `number`; a numeric
        // use inside the body checks, and misusing the result where a string
        // is expected is flagged — the param and return carry their types.
        let src = "\
---@type fun(x: number): number
local f = function(x) return x + 1 end

---@param s string
local function needs_string(s) end
needs_string(f(2))
";
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn explicit_lambda_annotation_overrides_contextual() {
        // An explicit `---@param` on the lambda is authoritative (SPEC §3):
        // it wins over the contextual `fun(w: Widget)`, so `w` is `any` here
        // and the field read is NOT flagged. Were the contextual type to leak
        // through, `w.nofield` on `Widget` would raise LB0306.
        let src = "\
---@class Widget
---@field name string

---@type fun(w: Widget)
---@param w any
local f = function(w) return w.nofield end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn contextual_param_non_function_expected_is_unchanged() {
        // A `---@type` whose target is not a function type never seeds a
        // lambda parameter (there is no expected function type to draw from).
        let src = "\
---@class Widget
---@field name string

---@type Widget
local w = { name = \"x\" }
local n = w.nofield
";
        // The only diagnostic is the direct bad-field read on `w`, proving no
        // contextual machinery misfired.
        assert_eq!(strict_codes(src), vec!["LB0306"]);
    }

    // --- follow-ups: expected type INTO literals, return position, nesting ---
    //
    // luals reference: `script/vm/compiler.lua` compiles a node lazily against
    // its expected (`infer`) type — flowing an expected class into a table
    // constructor and an expected `fun(...)` into a returned closure, recursing
    // through nested callback and table layers. luabox mirrors this in
    // `infer::seed_contextual` (+ `seed_returns`), keeping the checker's
    // field-by-field literal diagnostics (LB0302/LB0303/LB0304) intact.

    #[test]
    fn contextual_class_into_typed_local_function_field() {
        // (A) An expected `---@class` flows into a `---@type` table literal: a
        // function-valued field's lambda takes the field's declared `fun(...)`
        // parameter types, so a bad field read inside it is flagged.
        let bad = "\
---@class Event
---@field x number
---@class Widget
---@field onclick fun(e: Event)

---@type Widget
local w = { onclick = function(e) return e.nofield end }
";
        assert_eq!(strict_codes(bad), vec!["LB0306"]);

        let good = "\
---@class Event
---@field x number
---@class Widget
---@field onclick fun(e: Event)

---@type Widget
local w = { onclick = function(e) return e.x end }
";
        assert_eq!(strict_codes(good), Vec::<String>::new());
    }

    #[test]
    fn contextual_class_into_param_literal_keeps_field_checks() {
        // (A) The same flow at a `---@param p Widget` call argument, AND the
        // literal's own field-compatibility diagnostics still fire: `name` is
        // missing (LB0302) while the seeded `e` inside `onclick` flags the bad
        // read (LB0306) — the contextual type does not weaken the field check.
        let src = "\
---@class Event
---@field x number
---@class Widget
---@field name string
---@field onclick fun(e: Event)

---@param p Widget
local function f(p) end

f({ onclick = function(e) return e.nofield end })
";
        assert_eq!(strict_codes(src), vec!["LB0302", "LB0306"]);
    }

    #[test]
    fn contextual_bad_literal_field_still_diagnosed() {
        // (A) A structurally bad literal at a class parameter is still flagged
        // field-by-field (missing required + undeclared key), unchanged.
        let src = "\
---@class Widget
---@field name string

---@param p Widget
local function f(p) end

f({ nope = 1 })
";
        assert_eq!(strict_codes(src), vec!["LB0302", "LB0303"]);
    }

    #[test]
    fn contextual_non_single_class_expected_seeds_nothing() {
        // (A) An `any` expected type — and a two-member union with no single
        // expected shape — seed nothing: the lambda parameter stays `unknown`
        // and the bad read is NOT flagged (conservatism, unchanged behavior).
        let any_expected = "\
---@class Event
---@field x number
---@class Widget
---@field onclick fun(e: Event)

---@type any
local w = { onclick = function(e) return e.nofield end }
";
        assert_eq!(strict_codes(any_expected), Vec::<String>::new());

        let union_expected = "\
---@class Event
---@field x number
---@class W1
---@field onclick fun(e: Event)
---@class W2
---@field onclick fun(e: Event)

---@param p W1|W2
local function f(p) end

f({ onclick = function(e) return e.nofield end })
";
        assert_eq!(strict_codes(union_expected), Vec::<String>::new());
    }

    #[test]
    fn contextual_return_seeds_returned_lambda() {
        // (B) A `---@return fun(w: Widget)` contextually types the returned
        // function literal's parameter, so a bad field read inside it is
        // flagged — the return-position analogue of the callback-argument win.
        let bad = "\
---@class Widget
---@field name string

---@return fun(w: Widget)
local function make()
  return function(w) return w.nofield end
end
";
        assert_eq!(strict_codes(bad), vec!["LB0306"]);

        let good = "\
---@class Widget
---@field name string

---@return fun(w: Widget)
local function make()
  return function(w) return w.name end
end
";
        assert_eq!(strict_codes(good), Vec::<String>::new());
    }

    #[test]
    fn contextual_return_seeds_returned_table_function_field() {
        // (B, composes with A) A `---@return Widget` flows the class into the
        // returned table literal, typing its `onclick` field's lambda param.
        let src = "\
---@class Event
---@field x number
---@class Widget
---@field name string
---@field onclick fun(e: Event)

---@return Widget
local function make()
  return { name = \"x\", onclick = function(e) return e.nofield end }
end
";
        assert_eq!(strict_codes(src), vec!["LB0306"]);
    }

    #[test]
    fn contextual_return_bad_literal_still_diagnosed() {
        // (B) A returned literal that does not satisfy the declared class is
        // still flagged by the checker's return field check (missing field).
        let src = "\
---@class Widget
---@field name string
---@field size number

---@return Widget
local function make()
  return { name = \"x\" }
end
";
        assert_eq!(strict_codes(src), vec!["LB0302"]);
    }

    #[test]
    fn contextual_nested_lambda_two_levels() {
        // (C) `outer(function(a) return function(b) ... end end)` against
        // `---@param cb fun(x: A): fun(y: B)` types BOTH layers: the inner
        // `y` takes `B` transitively through the expected return type.
        let inner = "\
---@class A
---@field a number
---@class B
---@field b number

---@param cb fun(x: A): fun(y: B)
local function outer(cb) end

outer(function(x) return function(y) return y.nofield end end)
";
        assert_eq!(strict_codes(inner), vec!["LB0306"]);

        // The outer layer is still typed too (one-level win preserved).
        let outer_layer = "\
---@class A
---@field a number
---@class B
---@field b number

---@param cb fun(x: A): fun(y: B)
local function outer(cb) end

outer(function(x) return function(y) return x.nofield end end)
";
        assert_eq!(strict_codes(outer_layer), vec!["LB0306"]);
    }

    #[test]
    fn contextual_nested_table_literal_field() {
        // (C, composes with A) A table literal nested inside another
        // contextually-typed table literal takes the outer field's declared
        // class type, so the innermost function field's lambda is typed.
        let src = "\
---@class Event
---@field x number
---@class Panel
---@field onclick fun(e: Event)
---@class Widget
---@field panel Panel

---@type Widget
local w = { panel = { onclick = function(e) return e.nofield end } }
";
        assert_eq!(strict_codes(src), vec!["LB0306"]);
    }

    #[test]
    fn contextual_unannotated_intermediary_stays_untyped() {
        // (C) A layer luals would NOT type — a lambda passed to an UNANNOTATED
        // parameter `g` — carries no expected type, so its own `x` stays
        // `unknown` and the bad read is not flagged. Propagation follows the
        // expected-type structure only; it never invents one.
        let src = "\
---@class A
---@field a number

---@param cb fun(x: A)
local function outer(cb) end

local function passthru(g)
  g(function(x) return x.nofield end)
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }
}
