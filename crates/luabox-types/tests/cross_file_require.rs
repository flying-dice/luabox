// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]
//! Cross-file `require` resolution (#85): a `require("mod")` result is typed
//! from the required module's annotations, so conformance-style usage works
//! in consumer/test files, not just the module's own file.
//!
//! Each behavior is proven end-to-end at the type layer: [`module_surface`]
//! reifies the module file's `return` type plus its workspace-global
//! class/enum declarations, the export is keyed by the require string, the
//! classes merge into the consumer's ambient
//! ([`Ambient::with_project_types`]), and [`check_file_with_requires`]
//! threads the registry into the consumer's check. Misuse surfaces as a
//! diagnostic **at the consumer's use site**.

use std::collections::HashMap;

use luabox_diag::Diagnostic;
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::ty::Ty;
use luabox_types::{
    Ambient, FileTypes, Strictness, build_ambient, check_file_with_requires, module_surface,
    stdlib_defs,
};

/// The stdlib-only ambient for Lua 5.4.
fn stdlib() -> &'static Ambient {
    stdlib_defs(Dialect::Lua54)
}

/// Compute a module file's full check-mode surface against `ambient`.
fn surface(src: &str, ambient: &Ambient) -> (Ty, FileTypes) {
    let parsed = parse(src, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "module fixture must parse cleanly");
    let surface = module_surface(&parsed, "mod.lua", Some(ambient));
    (
        surface.export.expect("module returns a value"),
        surface.types,
    )
}

/// Compute a module file's check-mode export against `ambient`.
fn export(src: &str, ambient: &Ambient) -> Ty {
    surface(src, ambient).0
}

/// Strict-check a consumer file against `ambient` with a require registry.
fn check(src: &str, ambient: &Ambient, requires: &HashMap<String, Ty>) -> Vec<Diagnostic> {
    let parsed = parse(src, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "consumer fixture must parse cleanly");
    check_file_with_requires(
        &parsed,
        "consumer.lua",
        Strictness::Strict,
        Dialect::Lua54,
        Some(ambient),
        requires,
    )
}

fn codes(diags: &[Diagnostic]) -> Vec<String> {
    diags.iter().map(|d| d.code.to_string()).collect()
}

// --- structural export: a plain module table of annotated functions --------

const GEOM_MODULE: &str = "\
local M = {}
---@param w number
---@param h number
---@return number
function M.area(w, h)
  return w * h
end
return M
";

#[test]
fn require_result_flows_the_module_export_type() {
    // The consumer requires the module, calls an exported function, and
    // feeds its (number) result to a `string` parameter — the mismatch is
    // reported at the consumer's call site, proving the require typed
    // through from the module's `---@return`.
    let ambient = stdlib();
    let mut requires = HashMap::new();
    requires.insert("geom".to_string(), export(GEOM_MODULE, ambient));

    let consumer = "\
---@param s string
local function want(s) end
local M = require(\"geom\")
want(M.area(3, 4))
";
    assert_eq!(codes(&check(consumer, ambient, &requires)), vec!["LB0300"]);
}

#[test]
fn require_result_valid_use_is_clean() {
    let ambient = stdlib();
    let mut requires = HashMap::new();
    requires.insert("geom".to_string(), export(GEOM_MODULE, ambient));

    let consumer = "\
---@param n number
local function want(n) end
local M = require(\"geom\")
want(M.area(3, 4))
";
    assert_eq!(
        codes(&check(consumer, ambient, &requires)),
        Vec::<String>::new()
    );
}

// --- class-returning module + shared ambient class -------------------------

/// The interface both files see, ambient via a shared `---@meta` def.
const SHAPE_DEF: &str = "\
---@meta
---@class Shape
---@field area fun(self): number
";

/// A carrier module that reopens `Shape`, implements it, and returns itself.
const SHAPE_MODULE: &str = "\
---@class Shape
local Shape = {}
Shape.__index = Shape

---@return number
function Shape:area()
  return 1
end

---@param n number
---@return Shape
function Shape.new(n)
  return setmetatable({}, Shape)
end

return Shape
";

fn shape_ambient() -> Ambient {
    build_ambient(Dialect::Lua54, &[SHAPE_DEF.to_string()])
}

#[test]
fn require_of_class_module_resolves_inherited_method() {
    let ambient = shape_ambient();
    let mut requires = HashMap::new();
    requires.insert("shape".to_string(), export(SHAPE_MODULE, &ambient));

    // `Shape.new(2)` types as the class; `:area()` resolves through the
    // ambient class declaration and produces `number`.
    let consumer = "\
---@param n number
local function want(n) end
local Shape = require(\"shape\")
local s = Shape.new(2)
want(s:area())
";
    assert_eq!(
        codes(&check(consumer, &ambient, &requires)),
        Vec::<String>::new()
    );
}

#[test]
fn method_misuse_on_required_class_errors_at_consumer_site() {
    let ambient = shape_ambient();
    let mut requires = HashMap::new();
    requires.insert("shape".to_string(), export(SHAPE_MODULE, &ambient));

    // Calling a method the class does not declare is an undefined-field
    // read (LB0306), reported in the consumer at the misuse site.
    let consumer = "\
local Shape = require(\"shape\")
local s = Shape.new(2)
local _ = s:bogus()
";
    assert_eq!(codes(&check(consumer, &ambient, &requires)), vec!["LB0306"]);
}

// --- unresolved requires and cycles ----------------------------------------

#[test]
fn unresolved_require_stays_unknown_with_no_new_diagnostic() {
    // A require string absent from the registry (external / not a project
    // file) evaluates to `unknown` and raises no diagnostic of its own.
    let ambient = stdlib();
    let requires = HashMap::new();
    let consumer = "\
local M = require(\"nonexistent\")
local x = M
";
    assert_eq!(
        codes(&check(consumer, ambient, &requires)),
        Vec::<String>::new()
    );
}

#[test]
fn module_export_ignores_own_requires_so_cycles_terminate() {
    // A module that requires a partner (even cyclically) still computes an
    // export — its own requires are left unresolved, so there is no
    // recursion to loop on. Two mutually-requiring modules each produce a
    // finite export type.
    let ambient = stdlib();
    let a_src = "\
local B = require(\"b\")
local A = {}
---@return number
function A.f()
  return 1
end
return A
";
    let b_src = "\
local A = require(\"a\")
local B = {}
---@return number
function B.g()
  return 2
end
return B
";
    let a_export = export(a_src, ambient);
    let b_export = export(b_src, ambient);

    // Cross-check each against the other's export: no hang, no crash.
    let mut a_requires = HashMap::new();
    a_requires.insert("b".to_string(), b_export);
    let mut b_requires = HashMap::new();
    b_requires.insert("a".to_string(), a_export);

    assert_eq!(
        codes(&check(a_src, ambient, &a_requires)),
        Vec::<String>::new()
    );
    assert_eq!(
        codes(&check(b_src, ambient, &b_requires)),
        Vec::<String>::new()
    );
}

#[test]
fn no_return_module_has_no_export() {
    let parsed = parse("local M = {}\n", Dialect::Lua54);
    let surface = module_surface(&parsed, "mod.lua", Some(stdlib()));
    assert_eq!(surface.export, None);
    assert!(surface.types.is_empty(), "no declarations to contribute");
}

// --- workspace-global classes: probe A (no defs at all) --------------------

/// A self-contained annotated class module — NO ambient defs declare
/// `Circle`; the class exists only in this file (the common case, and the
/// literal text of #85: "typed from the required module's annotations").
const INLINE_CIRCLE: &str = "\
---@class Circle
---@field r number
local Circle = {}
Circle.__index = Circle
---@param r number
---@return Circle
function Circle.new(r) return setmetatable({ r = r }, Circle) end
---@return number
function Circle:area() return 3.14159 * self.r * self.r end
return Circle
";

/// Probe A's exact consumer: field read through the constructor result and
/// a method call, both fed to `---@type number` locals.
const INLINE_CONSUMER_OK: &str = "\
local Circle = require(\"circle\")
---@type number
local a1 = Circle.new(2).r
local c = Circle.new(2)
---@type number
local a2 = c:area()
";

/// Assemble the merged ambient + registry for a single inline-class module
/// and strict-check `consumer` against them.
fn check_inline(consumer: &str) -> Vec<Diagnostic> {
    let base = stdlib();
    let (export, types) = surface(INLINE_CIRCLE, base);
    let ambient = base.with_project_types([&types]);
    let mut requires = HashMap::new();
    requires.insert("circle".to_string(), export);
    check(consumer, &ambient, &requires)
}

#[test]
fn inline_class_through_require_types_fields_and_methods() {
    // Probe A: no defs anywhere. `Circle.new(2).r` and `c:area()` both type
    // as `number` because the class (and its member attachments) is
    // workspace-global, luals-style.
    assert_eq!(
        codes(&check_inline(INLINE_CONSUMER_OK)),
        Vec::<String>::new()
    );
}

#[test]
fn inline_class_misuse_errors_at_consumer_site() {
    let consumer = "\
local Circle = require(\"circle\")
local c = Circle.new(2)
local _ = c:bogus()
";
    let diags = check_inline(consumer);
    assert_eq!(codes(&diags), vec!["LB0306"]);
    let label = diags[0].primary_label().expect("primary label");
    assert_eq!(label.span.file, "consumer.lua");
    assert_eq!(&consumer[label.span.range.clone()], "c:bogus()");
}

#[test]
fn undefined_member_cascade_collapses_to_one_diagnostic() {
    // The `unknown` produced by an undefined-member read would also
    // mismatch the `---@type number` annotation — one mistake must yield
    // exactly one diagnostic (the specific LB0306, not an LB0300 echo).
    let consumer = "\
local Circle = require(\"circle\")
local c = Circle.new(2)
---@type number
local a = c:bogus()
";
    assert_eq!(codes(&check_inline(consumer)), vec!["LB0306"]);
}

#[test]
fn inline_class_literal_not_required_to_provide_methods() {
    // Carrier member attachments (`new`, `area`) resolve on reads but are
    // no table-literal obligation (luals `missing-fields` parity): a
    // consumer literal typed as the class needs only the `---@field`s.
    let consumer = "\
local Circle = require(\"circle\")
---@type Circle
local fake = { r = 1 }
---@type number
local n = fake.r
";
    assert_eq!(codes(&check_inline(consumer)), Vec::<String>::new());
    // ...while a missing `---@field` member still errors.
    let missing = "\
local Circle = require(\"circle\")
---@type Circle
local fake = {}
local _ = fake
";
    assert_eq!(codes(&check_inline(missing)), vec!["LB0300"]);
}

// --- probe B: defs AND a project file declare the same class ---------------

/// The defs side of probe B: the same class name, declaring the data field
/// and `new`/`area` members as `---@field`s.
const CIRCLE_DEF: &str = "\
---@meta
---@class Circle
---@field r number
---@field new fun(r: number): Circle
---@field area fun(self): number
";

#[test]
fn defs_and_inline_class_merge_members_without_duplicates() {
    // Probe B: `Circle` is declared BOTH by an ambient def and by the
    // module file. Members must merge (luals merges duplicate class
    // declarations' fields) — nothing drops, nothing double-reports.
    let base = build_ambient(Dialect::Lua54, &[CIRCLE_DEF.to_string()]);
    let (export, types) = surface(INLINE_CIRCLE, &base);
    let ambient = base.with_project_types([&types]);
    let mut requires = HashMap::new();
    requires.insert("circle".to_string(), export);

    // Probe B's exact consumer: both the field read and the method call
    // type through cleanly.
    assert_eq!(
        codes(&check(INLINE_CONSUMER_OK, &ambient, &requires)),
        Vec::<String>::new()
    );

    // And a genuine misuse still reports exactly one diagnostic.
    let misuse = "\
local Circle = require(\"circle\")
local c = Circle.new(2)
---@type number
local a = c:bogus()
";
    assert_eq!(codes(&check(misuse, &ambient, &requires)), vec!["LB0306"]);
}

// --- cross-file `---@alias` naming (#110) ----------------------------------
//
// An `---@alias` declared in one project file is nameable from a consumer's
// annotation, exactly like a workspace-global `---@class`/`---@enum` (#85).
// The alias is carried raw into the merged ambient by
// [`Ambient::with_project_types`] and expanded lazily by each consumer's
// lowerer, so cross-file alias-of-alias and alias-of-class resolve at the use
// site and cyclic aliases terminate via the lowerer's cycle guard.

/// The workspace-global surface a module file contributes (no `require`
/// registry needed — alias names resolve through the merged ambient, not a
/// module return value). Unlike [`surface`], this does not demand an export,
/// so an alias-only file (which has no `return`) is fine.
fn file_types(src: &str, ambient: &Ambient) -> FileTypes {
    let parsed = parse(src, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "module fixture must parse cleanly");
    module_surface(&parsed, "mod.lua", Some(ambient)).types
}

#[test]
fn cross_file_alias_resolves_and_enforces() {
    let base = stdlib();
    // `Id` is declared ONLY in the other file; the consumer names it in a
    // `---@param` with no `require` of its own.
    let types = file_types("---@alias Id string\n", base);
    let ambient = base.with_project_types([&types]);

    let ok = "\
---@param x Id
local function use(x) end
use(\"hello\")
";
    assert_eq!(
        codes(&check(ok, &ambient, &HashMap::new())),
        Vec::<String>::new()
    );
    // Misuse per the alias's underlying type (`string`) is LB0300 at the
    // consumer's call site — the alias resolved AND enforces.
    let bad = "\
---@param x Id
local function use(x) end
use(42)
";
    assert_eq!(
        codes(&check(bad, &ambient, &HashMap::new())),
        vec!["LB0300"]
    );
}

#[test]
fn cross_file_alias_of_alias() {
    let base = stdlib();
    // File 1 declares the base alias; file 2 declares an alias *of* it. The
    // consumer names only the second — expansion chains across both files.
    let t1 = file_types("---@alias Name string\n", base);
    let t2 = file_types("---@alias Label Name\n", base);
    let ambient = base.with_project_types([&t1, &t2]);

    let ok = "\
---@param x Label
local function use(x) end
use(\"n\")
";
    assert_eq!(
        codes(&check(ok, &ambient, &HashMap::new())),
        Vec::<String>::new()
    );
    let bad = "\
---@param x Label
local function use(x) end
use(1)
";
    assert_eq!(
        codes(&check(bad, &ambient, &HashMap::new())),
        vec!["LB0300"]
    );
}

#[test]
fn cross_file_alias_referencing_workspace_class() {
    let base = stdlib();
    // The class lives in one file, the alias-of-the-class in another: both
    // are workspace-global, so the alias body resolves the class name.
    let t_class = file_types("---@class Widget\n---@field id number\n", base);
    let t_alias = file_types("---@alias Handle Widget\n", base);
    let ambient = base.with_project_types([&t_class, &t_alias]);

    // A well-formed literal in a `Handle` position satisfies the class.
    let ok = "\
---@param h Handle
local function use(h) end
use({ id = 1 })
";
    assert_eq!(
        codes(&check(ok, &ambient, &HashMap::new())),
        Vec::<String>::new()
    );
    // A literal missing the class's required field still errors — proof the
    // alias resolved to the class shape, not to `unknown`.
    let bad = "\
---@param h Handle
local function use(h) end
use({})
";
    assert_eq!(
        codes(&check(bad, &ambient, &HashMap::new())),
        vec!["LB0302"]
    );
}

#[test]
fn same_name_alias_in_two_files_deterministic_winner() {
    let base = stdlib();
    // Two files declare `Id` differently. `with_project_types` is first-wins
    // among project files (mirroring the enum rule): the FIRST in iteration
    // order wins, deterministically — no crash on the collision.
    let t_string = file_types("---@alias Id string\n", base);
    let t_number = file_types("---@alias Id number\n", base);

    let use_string = "\
---@param x Id
local function use(x) end
use(\"s\")
";
    let use_number = "\
---@param x Id
local function use(x) end
use(1)
";

    // string-file first → `Id` is `string` workspace-wide.
    let ambient = base.with_project_types([&t_string, &t_number]);
    assert_eq!(
        codes(&check(use_string, &ambient, &HashMap::new())),
        Vec::<String>::new()
    );
    assert_eq!(
        codes(&check(use_number, &ambient, &HashMap::new())),
        vec!["LB0300"]
    );

    // Reverse the order → the winner flips deterministically.
    let ambient2 = base.with_project_types([&t_number, &t_string]);
    assert_eq!(
        codes(&check(use_number, &ambient2, &HashMap::new())),
        Vec::<String>::new()
    );
    assert_eq!(
        codes(&check(use_string, &ambient2, &HashMap::new())),
        vec!["LB0300"]
    );
}

#[test]
fn cyclic_alias_across_files_terminates() {
    let base = stdlib();
    // `Cy1` references `Cy2` and `Cy2` references `Cy1` — a cross-file cycle.
    // The lowerer's cycle guard collapses the recursion to `unknown` (exactly
    // the same-file cyclic-alias behavior), so checking terminates with no
    // hang and no crash — neither name trips LB0305 (both are known aliases,
    // just self-referential) — and the cycle itself is now reported (LB0314,
    // #123) once, at `Cy1`'s own declaration, not at the `use(1)` call site.
    let ta = file_types("---@alias Cy1 Cy2|number\n", base);
    let tb = file_types("---@alias Cy2 Cy1|string\n", base);
    let ambient = base.with_project_types([&ta, &tb]);

    let consumer = "\
---@param x Cy1
local function use(x) end
use(1)
";
    let found = codes(&check(consumer, &ambient, &HashMap::new()));
    assert!(
        !found.contains(&"LB0305".to_string()),
        "cyclic alias must resolve as a known alias, not an unknown name: {found:?}"
    );
    assert_eq!(found, vec!["LB0314"]);
}

#[test]
fn unresolved_alias_name_still_lb0305() {
    let base = stdlib();
    // A workspace-global alias exists, but the consumer names a DIFFERENT,
    // undeclared type — still LB0305 (workspace aliases don't mask genuine
    // unknown-type errors).
    let types = file_types("---@alias Id string\n", base);
    let ambient = base.with_project_types([&types]);

    let consumer = "\
---@param x Nope
local function use(x) end
";
    assert_eq!(
        codes(&check(consumer, &ambient, &HashMap::new())),
        vec!["LB0305"]
    );
}
