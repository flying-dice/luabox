// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]
//! Cross-module argument checking (#46): a call through a `require` binding
//! consults the resolved function signature and reports `---@param`
//! mismatches exactly like a same-file call.
//!
//! The export *type* already crossed the boundary before this
//! ([`cross_file_require`](../cross_file_require.rs) proves it — a required
//! function's `---@return` flows into the consumer). What did not cross was
//! *parameter enforcement*: the checker resolved a callee's signature only
//! through its local-binding / dotted-name registries, neither of which knows
//! anything about a required module's members, so a call reached through
//! `require` was argument-checked by nobody.
//!
//! The fix routes those call sites through the *same*
//! `Checker::check_arg_slots` path a same-file call takes, so LB0300
//! (type mismatch) and LB0301 (arity) read identically on both sides of the
//! boundary. Its conservatism is inherited wholesale, and that is the point
//! of the negative tests below: an **unannotated** exported function is not
//! argument-checked at all, because an unannotated *same-file* function is
//! not either (measured — see `unannotated_export_is_not_checked`).

use std::collections::HashMap;

use luabox_diag::Diagnostic;
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::ty::Ty;
use luabox_types::{
    Ambient, FileTypes, Strictness, check_file_with_requires, module_surface, stdlib_defs,
};

/// The stdlib-only ambient for Lua 5.4.
fn stdlib() -> &'static Ambient {
    stdlib_defs(Dialect::Lua54)
}

/// Compute a module file's check-mode surface: its export type plus the
/// workspace-global class/enum declarations it contributes.
fn surface(src: &str) -> (Ty, FileTypes) {
    let parsed = parse(src, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "module fixture must parse cleanly");
    let surface = module_surface(&parsed, "mod.lua", Some(stdlib()));
    (
        surface.export.expect("module returns a value"),
        surface.types,
    )
}

/// Strict-check a consumer file with a one-entry require registry.
fn check_with(module: &str, consumer: &str) -> Vec<Diagnostic> {
    check_with_all(&[("mod", module)], consumer)
}

/// Strict-check a consumer file against several required modules, mirroring
/// what the CLI's cross-file pipeline assembles: each module is surfaced
/// independently (its own requires stay unresolved), its export is keyed by
/// the require string, and the classes it declares merge into the consumer's
/// ambient — workspace-global, luals-style.
fn check_with_all(modules: &[(&str, &str)], consumer: &str) -> Vec<Diagnostic> {
    let surfaces: Vec<(String, Ty, FileTypes)> = modules
        .iter()
        .map(|(name, src)| {
            let (export, types) = surface(src);
            ((*name).to_string(), export, types)
        })
        .collect();
    let ambient = stdlib().with_project_types(surfaces.iter().map(|(_, _, types)| types));
    let requires: HashMap<String, Ty> = surfaces
        .iter()
        .map(|(name, export, _)| (name.clone(), export.clone()))
        .collect();
    let parsed = parse(consumer, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "consumer fixture must parse cleanly");
    check_file_with_requires(
        &parsed,
        "consumer.lua",
        Strictness::Strict,
        Dialect::Lua54,
        Some(&ambient),
        &requires,
    )
}

fn codes(diags: &[Diagnostic]) -> Vec<String> {
    diags.iter().map(|d| d.code.to_string()).collect()
}

/// A `return M` module table of annotated functions — the ordinary shape.
const GEOM: &str = r#"
local M = {}
---@param w number
---@param h number
---@return number
function M.area(w, h)
  return w * h
end
return M
"#;

// --- the issue's measured table: the two rows that were `no` -------------

/// Row 4 of #46: cross-module, through a table field. The headline defect.
#[test]
fn cross_module_table_field_call_is_argument_checked() {
    let diags = check_with(
        GEOM,
        r#"
local geom = require("mod")
local x = geom.area("nope", 4)
"#,
    );
    assert_eq!(codes(&diags), ["LB0300"], "{diags:#?}");
}

/// Row 3 of #46: cross-module, direct — the module *is* the function.
#[test]
fn cross_module_direct_function_export_is_argument_checked() {
    let diags = check_with(
        r#"
---@param n number
---@return number
return function(n)
  return n
end
"#,
        r#"
local f = require("mod")
local x = f("nope")
"#,
    );
    assert_eq!(codes(&diags), ["LB0300"], "{diags:#?}");
}

// --- the rows that already passed, pinned as regression anchors -----------

/// Row 5 of #46: an explicit `---@type` at the call site restored checking
/// before this change. It must keep working — the new path must not shadow
/// the annotation, which stays authoritative (SPEC §3).
#[test]
fn explicit_type_annotation_at_call_site_still_checks() {
    let diags = check_with(
        GEOM,
        r#"
---@type fun(w: number, h: number): number
local area = require("mod").area
local x = area("nope", 4)
"#,
    );
    assert_eq!(codes(&diags), ["LB0300"], "{diags:#?}");
}

// --- arity, the sibling of type mismatch ---------------------------------

#[test]
fn cross_module_too_few_arguments_reports_arity() {
    let diags = check_with(
        GEOM,
        r#"
local geom = require("mod")
local x = geom.area(3)
"#,
    );
    assert_eq!(codes(&diags), ["LB0301"], "{diags:#?}");
}

#[test]
fn cross_module_too_many_arguments_reports_arity() {
    let diags = check_with(
        GEOM,
        r#"
local geom = require("mod")
local x = geom.area(3, 4, 5)
"#,
    );
    assert_eq!(codes(&diags), ["LB0301"], "{diags:#?}");
}

// --- the negative direction: correct code must stay silent ---------------

#[test]
fn cross_module_correct_arguments_report_nothing() {
    let diags = check_with(
        GEOM,
        r#"
local geom = require("mod")
local x = geom.area(3, 4)
"#,
    );
    assert_eq!(codes(&diags), Vec::<String>::new(), "{diags:#?}");
}

/// The conservatism gate. An exported function with **no** signature
/// annotation reifies to a `fun` whose parameters are `unknown` and whose
/// arity is whatever the body happens to declare. Checking a call against
/// *that* would manufacture arity errors about code the author never
/// annotated — and a same-file call to an equally unannotated function
/// reports nothing (measured on `develop`). So neither does this one.
#[test]
fn unannotated_export_is_not_checked() {
    let diags = check_with(
        r#"
local M = {}
function M.f(a, b)
  return a
end
return M
"#,
        r#"
local m = require("mod")
m.f("anything")
m.f(1, 2, 3)
"#,
    );
    assert_eq!(codes(&diags), Vec::<String>::new(), "{diags:#?}");
}

/// `---@param b? string` — an omitted optional argument is not an arity
/// error, cross-module exactly as same-file.
#[test]
fn cross_module_optional_parameter_may_be_omitted() {
    let diags = check_with(
        r#"
local M = {}
---@param a number
---@param b? string
function M.f(a, b) end
return M
"#,
        r#"
local m = require("mod")
m.f(1)
"#,
    );
    assert_eq!(codes(&diags), Vec::<String>::new(), "{diags:#?}");
}

/// ... but a supplied optional argument of the wrong type still reports.
#[test]
fn cross_module_optional_parameter_still_type_checked_when_supplied() {
    let diags = check_with(
        r#"
local M = {}
---@param a number
---@param b? string
function M.f(a, b) end
return M
"#,
        r#"
local m = require("mod")
m.f(1, 2)
"#,
    );
    assert_eq!(codes(&diags), ["LB0300"], "{diags:#?}");
}

/// `---@vararg` lifts the arity ceiling across the boundary too.
#[test]
fn cross_module_varargs_accept_extra_arguments() {
    let diags = check_with(
        r#"
local M = {}
---@param a number
---@vararg string
function M.f(a, ...) end
return M
"#,
        r#"
local m = require("mod")
m.f(1, "x", "y")
"#,
    );
    assert_eq!(codes(&diags), Vec::<String>::new(), "{diags:#?}");
}

/// ... and a vararg of the wrong element type still reports.
#[test]
fn cross_module_varargs_element_type_is_checked() {
    let diags = check_with(
        r#"
local M = {}
---@param a number
---@vararg string
function M.f(a, ...) end
return M
"#,
        r#"
local m = require("mod")
m.f(1, "x", 2)
"#,
    );
    assert_eq!(codes(&diags), ["LB0300"], "{diags:#?}");
}

// --- neighborhood: one step away from the fixed shape --------------------

/// The require binding renamed through a second local.
#[test]
fn aliased_require_binding_is_argument_checked() {
    let diags = check_with(
        GEOM,
        r#"
local g = require("mod")
local alias = g
local x = alias.area("nope", 4)
"#,
    );
    assert_eq!(codes(&diags), ["LB0300"], "{diags:#?}");
}

/// The function pulled out of the module table into a local, then called.
#[test]
fn function_stored_in_a_local_then_called_is_argument_checked() {
    let diags = check_with(
        GEOM,
        r#"
local geom = require("mod")
local area = geom.area
local x = area("nope", 4)
"#,
    );
    assert_eq!(codes(&diags), ["LB0300"], "{diags:#?}");
}

/// `require("mod").f(...)` with no intervening binding.
#[test]
fn inline_require_call_is_argument_checked() {
    let diags = check_with(
        GEOM,
        r#"
local x = require("mod").area("nope", 4)
"#,
    );
    assert_eq!(codes(&diags), ["LB0300"], "{diags:#?}");
}

/// A module table nesting a second table of functions.
#[test]
fn nested_table_export_is_argument_checked() {
    let diags = check_with(
        r#"
local M = { util = {} }
---@param s string
---@return string
function M.util.fmt(s)
  return s
end
return M
"#,
        r#"
local m = require("mod")
local x = m.util.fmt(42)
"#,
    );
    assert_eq!(codes(&diags), ["LB0300"], "{diags:#?}");
}

/// One module re-exporting another's function through a fresh table.
#[test]
fn re_exported_function_is_argument_checked() {
    let diags = check_with_all(
        &[
            ("geom", GEOM),
            (
                "reexport",
                r#"
---@param w number
---@param h number
---@return number
local function area(w, h)
  return w * h
end
return { area = area }
"#,
            ),
        ],
        r#"
local r = require("reexport")
local x = r.area("nope", 4)
"#,
    );
    assert_eq!(codes(&diags), ["LB0300"], "{diags:#?}");
}

/// A `:` colon-method on a class the module exports. This already worked
/// (the method-call path publishes its resolved signature), and it is pinned
/// here so the new dotted path does not double-report it.
#[test]
fn colon_method_on_exported_class_is_argument_checked_once() {
    let diags = check_with(
        r#"
---@class Box
local Box = {}
Box.__index = Box

---@return Box
function Box.new()
  return setmetatable({}, Box)
end

---@param w number
---@return number
function Box:grow(w)
  return w
end
return Box
"#,
        r#"
local Box = require("mod")
local b = Box.new()
local x = b:grow("nope")
"#,
    );
    assert_eq!(codes(&diags), ["LB0300"], "{diags:#?}");
}

/// The same method reached with `.` and an explicit receiver.
#[test]
fn dot_call_of_an_exported_method_is_argument_checked() {
    let diags = check_with(
        r#"
---@class Box
local Box = {}
Box.__index = Box

---@param self Box
---@param w number
---@return number
function Box.grow(self, w)
  return w
end
return Box
"#,
        r#"
local Box = require("mod")
---@type Box
local b = nil
local x = Box.grow(b, "nope")
"#,
    );
    assert_eq!(codes(&diags), ["LB0300"], "{diags:#?}");
}

/// A `---@field f fun(...)` declared on the exported class, rather than a
/// `function M.f` definition.
#[test]
fn declared_field_function_is_argument_checked() {
    let diags = check_with(
        r#"
---@class Api
---@field send fun(payload: string): boolean
local Api = {}
return Api
"#,
        r#"
local api = require("mod")
local ok = api.send(42)
"#,
    );
    assert_eq!(codes(&diags), ["LB0300"], "{diags:#?}");
}

/// An exported `---@overload`ed function: a call matching any overload is
/// accepted, one matching none reports.
#[test]
fn cross_module_overload_is_accepted_when_it_matches() {
    let module = r#"
local M = {}
---@param a number
---@return number
---@overload fun(a: string): number
function M.f(a)
  return 1
end
return M
"#;
    let ok = check_with(
        module,
        r#"
local m = require("mod")
local x = m.f("text")
"#,
    );
    assert_eq!(codes(&ok), Vec::<String>::new(), "{ok:#?}");
    let bad = check_with(
        module,
        r#"
local m = require("mod")
local x = m.f(true)
"#,
    );
    assert_eq!(codes(&bad), ["LB0300"], "{bad:#?}");
}

/// A dynamic `require(name)` resolves to no module, so its result stays
/// `unknown` and nothing is checked — the documented remaining edge.
#[test]
fn dynamic_require_path_stays_unchecked() {
    let diags = check_with(
        GEOM,
        r#"
local name = "mod"
local geom = require(name)
local x = geom.area("nope", 4)
"#,
    );
    assert_eq!(codes(&diags), Vec::<String>::new(), "{diags:#?}");
}
