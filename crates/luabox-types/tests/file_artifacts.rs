// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]
//! [`FileArtifacts`] — the harvest + lower a parse is turned into once and
//! then threaded through several entry points (CC-M1).
//!
//! The contract these pin is *equivalence*: an entry point fed pre-derived
//! artifacts must produce byte-for-byte what the plain entry point produces
//! deriving them itself. `luabox check` runs the artifact-threaded pair over
//! every project file, so any divergence here would be a silent behaviour
//! change in the CLI's diagnostics.

use std::collections::HashMap;

use luabox_diag::Diagnostic;
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::ty::Ty;
use luabox_types::{
    FileArtifacts, Strictness, check_file_with_artifacts, check_file_with_requires,
    module_requires, module_surface, module_surface_with_artifacts, stdlib_defs,
};

/// A module file worth checking twice: annotations, a class with a method, a
/// `require`, an exported table, and a genuine mismatch to diagnose.
const FIXTURE: &str = "\
local dep = require(\"pkg.dep\")

---@class Greeter
---@field greeting string
local Greeter = {}

---@param name string
---@return string
function Greeter:hello(name)
  return self.greeting .. name
end

---@param n number
local function double(n)
  return n * 2
end

double(\"nope\")

return Greeter
";

fn parsed() -> luabox_syntax::lua::Parse {
    let parse = parse(FIXTURE, Dialect::Lua54);
    assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
    parse
}

fn codes(diags: &[Diagnostic]) -> Vec<String> {
    diags.iter().map(|d| d.code.to_string()).collect()
}

#[test]
fn artifact_requires_match_the_standalone_require_inventory() {
    let parse = parsed();
    let artifacts = FileArtifacts::new(&parse);
    assert_eq!(artifacts.requires(), vec!["pkg.dep"]);
    assert_eq!(artifacts.requires(), module_requires(&parse));
}

#[test]
fn artifact_requires_are_empty_for_a_file_without_requires() {
    let parse = parse("local x = 1\n", Dialect::Lua54);
    assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
    assert_eq!(FileArtifacts::new(&parse).requires(), Vec::<String>::new());
}

#[test]
fn a_surface_from_artifacts_equals_the_surface_derived_in_place() {
    let parse = parsed();
    let ambient = stdlib_defs(Dialect::Lua54);
    let artifacts = FileArtifacts::new(&parse);
    assert_eq!(
        module_surface_with_artifacts(&parse, "mod.lua", Some(ambient), &artifacts),
        module_surface(&parse, "mod.lua", Some(ambient)),
    );
}

#[test]
fn a_check_from_artifacts_equals_the_check_derived_in_place() {
    let parse = parsed();
    let ambient = stdlib_defs(Dialect::Lua54);
    let artifacts = FileArtifacts::new(&parse);
    let requires: HashMap<String, Ty> = HashMap::from([("pkg.dep".to_owned(), Ty::Number)]);

    for strictness in [Strictness::None, Strictness::Warn, Strictness::Strict] {
        let threaded = check_file_with_artifacts(
            &parse,
            "mod.lua",
            strictness,
            Dialect::Lua54,
            Some(ambient),
            &requires,
            &artifacts,
        );
        let plain = check_file_with_requires(
            &parse,
            "mod.lua",
            strictness,
            Dialect::Lua54,
            Some(ambient),
            &requires,
        );
        assert_eq!(codes(&threaded), codes(&plain), "at {strictness:?}");
        assert_eq!(threaded, plain, "at {strictness:?}");
    }
}

#[test]
fn the_shared_artifacts_still_produce_the_fixture_s_own_diagnostic() {
    // Equivalence would be vacuous if both sides found nothing: the strict
    // check of this fixture must actually diagnose `double("nope")`.
    let parse = parsed();
    let artifacts = FileArtifacts::new(&parse);
    let diags = check_file_with_artifacts(
        &parse,
        "mod.lua",
        Strictness::Strict,
        Dialect::Lua54,
        Some(stdlib_defs(Dialect::Lua54)),
        &HashMap::<String, Ty>::new(),
        &artifacts,
    );
    assert_eq!(codes(&diags), vec!["LB0300"]);
}

#[test]
fn artifacts_can_be_reused_across_every_entry_point_for_one_file() {
    // The `check` pipeline's actual usage: one derivation feeding the surface
    // pass, the require inventory, and the check — in that order, on the same
    // borrow.
    let parse = parsed();
    let ambient = stdlib_defs(Dialect::Lua54);
    let artifacts = FileArtifacts::new(&parse);

    let surface = module_surface_with_artifacts(&parse, "mod.lua", Some(ambient), &artifacts);
    assert!(surface.export.is_some(), "the fixture returns Greeter");
    assert!(
        !surface.types.is_empty(),
        "the fixture declares a workspace-global class"
    );

    let requires: HashMap<String, Ty> = artifacts
        .requires()
        .into_iter()
        .map(|module| (module, Ty::Number))
        .collect();
    let diags = check_file_with_artifacts(
        &parse,
        "mod.lua",
        Strictness::Strict,
        Dialect::Lua54,
        Some(ambient),
        &requires,
        &artifacts,
    );
    assert_eq!(codes(&diags), vec!["LB0300"]);
}
