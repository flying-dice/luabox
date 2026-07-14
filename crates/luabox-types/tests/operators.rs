//! `---@operator` overload application across the workspace surface (#114).
//!
//! Operators declared on a `---@class` are workspace-global exactly like
//! `---@field`s: an operator declared on a class in one project file, or in a
//! `[types] defs` package, applies wherever that class is used. These tests
//! prove the operator result types through at a *consumer* file's use site —
//! a correct use checks clean under strict (the result is the declared class,
//! not `unknown`, which would itself error `unknown -> T` under strict), and a
//! misuse of the result surfaces `LB0300`.

use std::collections::HashMap;

use luabox_syntax::lua::{Dialect, parse};
use luabox_types::ty::Ty;
use luabox_types::{
    Ambient, FileTypes, Strictness, check_file_with_requires, combined_defs, module_surface,
    stdlib_defs,
};

fn stdlib() -> &'static Ambient {
    stdlib_defs(Dialect::Lua54)
}

/// A module file's workspace-global type surface (its `---@class`
/// declarations, including their operators).
fn surface(src: &str, ambient: &Ambient) -> FileTypes {
    let parsed = parse(src, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "module fixture must parse cleanly");
    module_surface(&parsed, "mod.lua", Some(ambient)).types
}

/// Strict-check a consumer file against `ambient` (no require registry needed:
/// the class is named through the workspace-global surface).
fn check(src: &str, ambient: &Ambient) -> Vec<String> {
    let parsed = parse(src, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "consumer fixture must parse cleanly");
    check_file_with_requires(
        &parsed,
        "consumer.lua",
        Strictness::Strict,
        Dialect::Lua54,
        Some(ambient),
        &HashMap::<String, Ty>::new(),
    )
    .iter()
    .map(|d| d.code.to_string())
    .collect()
}

// --- cross-file (workspace-global class) ----------------------------------

const VEC_MODULE: &str = "\
---@class Vec
---@operator add(Vec): Vec
---@operator mul(number): Vec
local Vec = {}
return Vec
";

#[test]
fn operator_applies_cross_file_when_use_is_correct() {
    // `Vec` and its operators are declared in the module file; the consumer
    // names `Vec` through the workspace-global surface and uses `a + b`,
    // binding the result to a `Vec`. If the operator did not apply the result
    // would be `unknown` and `unknown -> Vec` would error under strict — so a
    // clean check proves the declared `Vec` result flowed cross-file.
    let base = stdlib();
    let ambient = base.with_project_types([&surface(VEC_MODULE, base)]);
    let consumer = "\
---@type Vec
local a
---@type Vec
local b
---@type Vec
local c = a + b
";
    assert_eq!(check(consumer, &ambient), Vec::<String>::new());
}

#[test]
fn operator_result_misuse_flagged_cross_file() {
    // Same setup; the result is fed to a `string` slot — the mismatch is
    // reported at the consumer, proving the operator typed the result as `Vec`
    // rather than degrading to `unknown`.
    let base = stdlib();
    let ambient = base.with_project_types([&surface(VEC_MODULE, base)]);
    let consumer = "\
---@type Vec
local a
---@type Vec
local b
---@type string
local c = a + b
";
    assert_eq!(check(consumer, &ambient), vec!["LB0300"]);
}

#[test]
fn reversed_operand_dispatch_cross_file() {
    // `2 * v` resolves through `Vec`'s `mul(number): Vec` (right-operand
    // dispatch) across the workspace surface.
    let base = stdlib();
    let ambient = base.with_project_types([&surface(VEC_MODULE, base)]);
    let consumer = "\
---@type Vec
local v
---@type Vec
local scaled = 2 * v
";
    assert_eq!(check(consumer, &ambient), Vec::<String>::new());
    let bad = "\
---@type Vec
local v
---@type string
local scaled = 2 * v
";
    assert_eq!(check(bad, &ambient), vec!["LB0300"]);
}

// --- defs package ---------------------------------------------------------

const VEC_DEF: &str = "\
---@meta
---@class Vec
---@operator add(Vec): Vec
";

#[test]
fn operator_applies_through_defs_package() {
    // `Vec` and its `add` operator come from a `[types] defs` package. A
    // consumer using `a + b` and binding the result to `Vec` checks clean.
    let ambient = combined_defs(Dialect::Lua54, &[VEC_DEF.to_string()]);
    let consumer = "\
---@type Vec
local a
---@type Vec
local b
---@type Vec
local c = a + b
";
    assert_eq!(check(consumer, &ambient), Vec::<String>::new());
}

#[test]
fn operator_result_misuse_flagged_through_defs_package() {
    let ambient = combined_defs(Dialect::Lua54, &[VEC_DEF.to_string()]);
    let consumer = "\
---@type Vec
local a
---@type Vec
local b
---@type string
local c = a + b
";
    assert_eq!(check(consumer, &ambient), vec!["LB0300"]);
}
