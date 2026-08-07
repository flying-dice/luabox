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
    // The module's workspace-global types merge into the consumer's ambient
    // (`with_project_types`) exactly as the real pipeline merges every
    // checked file's — since #56 the export IS the class name, so the
    // carrier's member attachments must arrive via that merge, not via a
    // structural export type.
    let base = shape_ambient();
    let (export_ty, types) = surface(SHAPE_MODULE, &base);
    let ambient = base.with_project_types([&types]);
    let mut requires = HashMap::new();
    requires.insert("shape".to_string(), export_ty);

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
    let base = shape_ambient();
    let (export_ty, types) = surface(SHAPE_MODULE, &base);
    let ambient = base.with_project_types([&types]);
    let mut requires = HashMap::new();
    requires.insert("shape".to_string(), export_ty);

    // Calling a method the class does not declare is an undefined-field
    // read (LB0306), reported in the consumer at the misuse site.
    let consumer = "\
local Shape = require(\"shape\")
local s = Shape.new(2)
local _ = s:bogus()
";
    assert_eq!(codes(&check(consumer, &ambient, &requires)), vec!["LB0306"]);
}

// --- generic carriers crossing the boundary --------------------------------

/// A carrier for a *generic* class — the one class name that is not a
/// complete type on its own, since `Ty::Named` carries no type arguments.
const BOX_MODULE: &str = "\
---@class Box<T>
---@field item T
local B = {}
return B
";

fn box_ambient_and_requires() -> (Ambient, HashMap<String, Ty>) {
    let (export_ty, types) = surface(BOX_MODULE, stdlib());
    let ambient = stdlib().with_project_types([&types]);
    let mut requires = HashMap::new();
    requires.insert("box".to_string(), export_ty);
    (ambient, requires)
}

#[test]
fn generic_carrier_export_never_names_its_unbound_parameter() {
    // Crossing as `Box` would type the member `T` in the consumer — a type
    // variable it can neither name nor produce, so the diagnostic points at no
    // action. The unbound parameter reads `unknown`, exactly as it does for a
    // bare `Box` written by hand.
    let (ambient, requires) = box_ambient_and_requires();
    let consumer = "\
---@param n number
local function want(n) end
local b = require(\"box\")
want(b.item)
";
    let diags = check(consumer, &ambient, &requires);
    assert_eq!(codes(&diags), vec!["LB0300"]);
    assert!(
        diags[0].message.contains("found `unknown`"),
        "{}",
        diags[0].message
    );
    assert!(
        !diags[0].message.contains("`T`"),
        "the parameter must not reach the consumer: {}",
        diags[0].message
    );
    // F72 (round 3 review) had this message append a blanket "(add
    // `---@type <expected>` to check it)" remedy for any `unknown` mismatch.
    // Round 4 review R12 withdrew it: `slot_range(slot)` here is `b.item`,
    // the call-argument *expression* — not `b`'s own binding, which is
    // where `---@type Box<number>` (the action
    // `binding_the_type_argument_types_the_required_generic_carrier` below
    // measures) actually has to go. A remedy anchored at a site the reader
    // cannot annotate misdirects rather than helps, so the message no
    // longer guesses one.
    assert!(
        !diags[0].message.contains("---@type"),
        "no remedy the reader cannot act on at this site: {}",
        diags[0].message
    );
}

#[test]
fn binding_the_type_argument_types_the_required_generic_carrier() {
    // The other direction, and the action the `unknown` implies: naming the
    // argument on the binding monomorphises the same export to `number`.
    // Measured as a *control*, not a red pin — this held before the export
    // stopped leaking `T` too (the `---@type` overrides the export type), and
    // it is what makes the `unknown` above actionable rather than terminal.
    let (ambient, requires) = box_ambient_and_requires();
    let consumer = "\
---@param n number
local function want(n) end
---@type Box<number>
local b = require(\"box\")
want(b.item)
";
    assert_eq!(
        codes(&check(consumer, &ambient, &requires)),
        Vec::<String>::new()
    );

    // …and a wrong argument is rejected against the bound parameter, so the
    // members are genuinely typed rather than leniently erased.
    let mismatched = "\
---@param n number
local function want(n) end
---@type Box<string>
local b = require(\"box\")
want(b.item)
";
    assert_eq!(
        codes(&check(mismatched, &ambient, &requires)),
        vec!["LB0300"]
    );
}

#[test]
fn a_plain_carrier_still_crosses_as_the_class_itself() {
    // The one-variable control for the two tests above: drop the `<T>` and the
    // #56 behaviour is unchanged — the export is the class, so an undeclared
    // member is `LB0306`.
    const PLAIN: &str = "\
---@class Crate
---@field item number
local C = {}
return C
";
    let (export_ty, types) = surface(PLAIN, stdlib());
    let ambient = stdlib().with_project_types([&types]);
    let mut requires = HashMap::new();
    requires.insert("crate".to_string(), export_ty);

    let consumer = "\
local c = require(\"crate\")
local _ = c.nope
";
    assert_eq!(codes(&check(consumer, &ambient, &requires)), vec!["LB0306"]);
}

#[test]
fn an_undeclared_member_on_a_generic_carrier_stays_lenient() {
    // The documented exception to #56, pinned in the direction it actually
    // behaves. A carrier with an unbound parameter crosses as the *template*
    // rather than as `Ty::Named` — that is what stops `T` leaking into the
    // consumer — and a template is a structural table, which carries no
    // undefined-field obligation. So the read below is clean where
    // `a_plain_carrier_still_crosses_as_the_class_itself`'s identical read is
    // `LB0306`; the two differ by `<T>` alone.
    //
    // Without this pin, `reify_export`'s erasure branch is one edit away from
    // flipping the rule back with nothing failing (round 2, finding 3).
    let (ambient, requires) = box_ambient_and_requires();
    let consumer = "\
local b = require(\"box\")
local _ = b.nope
";
    assert_eq!(
        codes(&check(consumer, &ambient, &requires)),
        Vec::<String>::new()
    );
}

/// `---@class Sub : Base<number>` — a plain class inheriting a member whose
/// type is the *parent's* type parameter.
const BOUND_PARENT_MODULE: &str = "\
---@class Base<U>
---@field item U
---@class Sub : Base<number>
local S = {}
return S
";

fn sub_ambient_and_requires(module: &str) -> (Ambient, HashMap<String, Ty>) {
    let (export_ty, types) = surface(module, stdlib());
    let ambient = stdlib().with_project_types([&types]);
    let mut requires = HashMap::new();
    requires.insert("sub".to_string(), export_ty);
    (ambient, requires)
}

#[test]
fn a_parent_type_argument_binds_the_inherited_member() {
    // `: Base<number>` used to bind nothing at all: the argument was dropped
    // at lowering, `Sub` inherited `item: U`, and the consumer was told
    // `found `U`` — a name it can neither produce nor act on. The argument now
    // binds the parent's parameter where the members merge, so `item` is
    // `number` on both sides of the `require`, and `Sub` has nothing unbound
    // left to erase: it keeps its #56 class identity.
    let (ambient, requires) = sub_ambient_and_requires(BOUND_PARENT_MODULE);
    let consumer = "\
---@param n number
local function want(n) end
local s = require(\"sub\")
want(s.item)
";
    assert_eq!(
        codes(&check(consumer, &ambient, &requires)),
        Vec::<String>::new()
    );

    // The rejecting probe beside the accepting one: `item` is genuinely
    // `number`, not leniently erased to `unknown`.
    let mismatched = "\
---@param s string
local function want(s) end
local s = require(\"sub\")
want(s.item)
";
    let diags = check(mismatched, &ambient, &requires);
    assert_eq!(codes(&diags), vec!["LB0300"]);
    assert!(
        diags[0].message.contains("found `number`"),
        "{}",
        diags[0].message
    );

    // …and the class identity survives, so an undeclared member is `LB0306`.
    let undeclared = "\
local s = require(\"sub\")
local _ = s.nope
";
    assert_eq!(
        codes(&check(undeclared, &ambient, &requires)),
        vec!["LB0306"]
    );
}

#[test]
fn a_parent_written_bare_leaves_its_parameter_unbound_and_erased() {
    // The one-variable control: the same two classes with the argument
    // dropped from the parent reference. Nothing binds `U`, so the seam does
    // what it does for a carrier's own unbound parameter — cross as the
    // template with `unknown` — rather than leak the name. That is the rule
    // the round-2 regression broke, kept pinned for the shape that still
    // reaches it.
    const BARE_PARENT: &str = "\
---@class Base<U>
---@field item U
---@class Sub : Base
local S = {}
return S
";
    let (ambient, requires) = sub_ambient_and_requires(BARE_PARENT);
    let consumer = "\
---@param n number
local function want(n) end
local s = require(\"sub\")
want(s.item)
";
    let diags = check(consumer, &ambient, &requires);
    assert_eq!(codes(&diags), vec!["LB0300"]);
    assert!(
        diags[0].message.contains("found `unknown`"),
        "{}",
        diags[0].message
    );
    assert!(
        !diags[0].message.contains("`U`"),
        "an inherited parameter must not reach the consumer either: {}",
        diags[0].message
    );
}

#[test]
fn a_parent_argument_written_in_the_childs_own_parameter_passes_through() {
    // Two levels, with the middle class passing its own parameter up:
    // `Mid<M> : Slot<M>` and `Leaf : Mid<number>` means `Leaf.slot` is
    // `number`. The binding is positional at each level, so a class's own map
    // must reach the arguments it passes to its parent before those bind the
    // parent's parameters — one substitution, applied on the way up.
    const CHAIN: &str = "\
---@class Slot<S>
---@field slot S
---@class Mid<M> : Slot<M>
---@class Leaf : Mid<number>
local L = {}
return L
";
    let (ambient, requires) = sub_ambient_and_requires(CHAIN);
    let consumer = "\
---@param n number
local function want(n) end
local l = require(\"sub\")
want(l.slot)
";
    assert_eq!(
        codes(&check(consumer, &ambient, &requires)),
        Vec::<String>::new()
    );

    let mismatched = "\
---@param s string
local function want(s) end
local l = require(\"sub\")
want(l.slot)
";
    assert_eq!(
        codes(&check(mismatched, &ambient, &requires)),
        vec!["LB0300"]
    );
}

#[test]
fn a_real_class_name_reused_as_an_ancestors_parameter_does_not_erase_the_export() {
    // F43: `class_params_in_scope` collects an ancestor's *declared parameter
    // names* unconditionally, and class names and type parameters share one
    // namespace (`env.rs` documents the collision explicitly). `Emitter<Event>`
    // names its OWN type parameter `Event` — legal, if confusing — and `Sub :
    // Emitter<Click>` binds it away. `Sub`'s own (unrelated) field `pending`
    // is typed `Event`, naming the REAL class. Pre-fix, `class_params_in_scope`
    // reports "Event" as a parameter in scope (from `Emitter`'s declaration)
    // with no regard for `Sub`'s parent reference already binding it, so
    // `reify_export` erases the real class reference to `unknown` and, in the
    // same stroke, costs the module its `Ty::Named("Sub")` identity — an
    // undeclared member that should be `LB0306` reads as lenient instead.
    const EVENT_DEF: &str = "\
---@meta
---@class Event
---@field id number
";
    const COLLIDING_MODULE: &str = "\
---@class Emitter<Event>
---@class Sub : Emitter<Click>
---@field pending Event
local S = {}
return S
";
    let base = build_ambient(Dialect::Lua54, &[EVENT_DEF.to_string()]);
    let (export_ty, types) = surface(COLLIDING_MODULE, &base);
    let ambient = base.with_project_types([&types]);
    let mut requires = HashMap::new();
    requires.insert("sub".to_string(), export_ty);

    // The real class's identity must survive the export seam: an undeclared
    // member is LB0306, exactly as the plain (non-generic-neighbour) carrier
    // control (`a_plain_carrier_still_crosses_as_the_class_itself`) pins.
    let undeclared = "\
local s = require(\"sub\")
local _ = s.nope
";
    assert_eq!(
        codes(&check(undeclared, &ambient, &requires)),
        vec!["LB0306"],
        "Sub must keep its #56 class identity: Emitter's own parameter (which \
         happens to be spelled `Event`) is bound by `Sub`'s parent reference, \
         not left free"
    );

    // …and `pending`'s declared type is genuinely the real class `Event`
    // (with its own `id` field), not erased to `unknown` because its name
    // collides with an unrelated ancestor's parameter.
    let good = "\
local s = require(\"sub\")
---@type number
local n = s.pending.id
";
    assert_eq!(
        codes(&check(good, &ambient, &requires)),
        Vec::<String>::new(),
        "`pending` must resolve as the real `Event` class, not `unknown`"
    );
}

/// A class-carrier module, optionally opened with a UTF-8 BOM — the
/// operator-surfaced defect behind
/// `a_bom_prefixed_declaring_file_still_types_its_fields_across_require`.
fn point_module(bom: bool) -> String {
    let prefix = if bom { "\u{feff}" } else { "" };
    format!(
        "{prefix}\
---@class Point
---@field x number
local P = {{}}
return P
"
    )
}

#[test]
fn a_bom_prefixed_declaring_file_still_types_its_fields_across_require() {
    // `luacats::harvest`'s `resolve_target` scanned backward from a doc
    // block for "the real token before it" to decide leading-vs-trailing,
    // and did not treat a leading BOM as trivia (fixed in
    // `crates/luabox-syntax/src/luacats/mod.rs`'s `collect_tokens`). A doc
    // block that opens a BOM'd file — exactly the `---@class`/`---@field`
    // pair every carrier module starts with — was misread as a *trailing*
    // comment on the (nonexistent) statement containing byte 0, so its
    // `target` came back `None`: the class never linked to the `local P =
    // {}` carrier statement it declares. Same-file inference was unaffected
    // (it resolves fields through `env.classes`, populated regardless), but
    // `record_class_carrier` never marked the carrier `declared`, so
    // `reify_export` crossed `require` as a bare structural `{}` instead of
    // `Ty::Named("Point")` — field existence went lenient (no class to
    // enumerate against) while every declared field's *type* silently
    // dropped to `unknown` for want of the class shape.
    let base = stdlib();
    let (export_ty, types) = surface(&point_module(true), base);
    assert_eq!(
        export_ty,
        Ty::Named("Point".to_string()),
        "a BOM must not cost the carrier its class identity at the export seam"
    );
    let ambient = base.with_project_types([&types]);
    let mut requires = HashMap::new();
    requires.insert("point".to_string(), export_ty);

    let clean = "\
---@param n number
local function want(n) end
local p = require(\"point\")
want(p.x)
";
    assert_eq!(
        codes(&check(clean, &ambient, &requires)),
        Vec::<String>::new(),
        "x's declared `number` must reach the consumer, not `unknown`"
    );
    let rejecting = "\
---@param s string
local function want(s) end
local p = require(\"point\")
want(p.x)
";
    assert_eq!(
        codes(&check(rejecting, &ambient, &requires)),
        vec!["LB0300"],
        "x must be genuinely typed `number`, not leniently `unknown`"
    );
    let existence = "\
local p = require(\"point\")
local _ = p.nope
";
    assert_eq!(
        codes(&check(existence, &ambient, &requires)),
        vec!["LB0306"],
        "the class identity must survive, so an undeclared member is still flagged"
    );
}

#[test]
fn a_bom_free_control_beside_the_bom_pin_above_is_identical() {
    // Same fixture, no BOM — the control the BOM test above is measured
    // against, so a future regression in either direction shows up as a
    // difference between this test and that one, not just a difference from
    // some doc comment's claim.
    let base = stdlib();
    let (export_ty, types) = surface(&point_module(false), base);
    assert_eq!(export_ty, Ty::Named("Point".to_string()));
    let ambient = base.with_project_types([&types]);
    let mut requires = HashMap::new();
    requires.insert("point".to_string(), export_ty);

    let clean = "\
---@param n number
local function want(n) end
local p = require(\"point\")
want(p.x)
";
    assert_eq!(
        codes(&check(clean, &ambient, &requires)),
        Vec::<String>::new()
    );
    let rejecting = "\
---@param s string
local function want(s) end
local p = require(\"point\")
want(p.x)
";
    assert_eq!(
        codes(&check(rejecting, &ambient, &requires)),
        vec!["LB0300"]
    );
    let existence = "\
local p = require(\"point\")
local _ = p.nope
";
    assert_eq!(
        codes(&check(existence, &ambient, &requires)),
        vec!["LB0306"]
    );
}

#[test]
fn a_genuinely_free_parameter_spelled_like_a_real_class_still_erases() {
    // The mirror of `a_real_class_name_reused_as_an_ancestors_parameter_
    // does_not_erase_the_export` above, in the OTHER direction (round 4
    // review R6). That test pins the bound case: `Emitter`'s own parameter
    // (spelled `Event`) is bound away by `Sub`'s parent reference, so a
    // genuinely unrelated field typed `Event` must resolve as the real
    // class. This test pins the free case: `Base`'s own parameter (spelled
    // `Event`) is left BARE by `Sub : Base` — genuinely unbound — so
    // `item`, typed with that very parameter, must erase to `unknown`
    // exactly as it would if the parameter were spelled `U`.
    //
    // A round 3 review F43 regression inverted this: `collect_free_names`
    // asked whether each `Ty::Named` leaf in the *resolved* shape failed to
    // resolve as a real class, which cannot distinguish "unbound" from
    // "bound to a real class that happens to share the parameter's
    // spelling" — both read back as `Ty::Named("Event")`. It answered
    // "bound" for both, so the genuinely-free case here stopped erasing:
    // `s.item.nope` read as an undefined field *on the real `Event` class*
    // instead of the lenient `unknown` an erased carrier gets, and
    // `s.item.id` reached the real class's `id` field instead of `unknown`.
    const EVENT_DEF: &str = "\
---@meta
---@class Event
---@field id number
";
    const COLLIDING_MODULE: &str = "\
---@class Base<Event>
---@field item Event
---@class Sub : Base
local S = {}
return S
";
    let base = build_ambient(Dialect::Lua54, &[EVENT_DEF.to_string()]);
    let (export_ty, types) = surface(COLLIDING_MODULE, &base);
    let ambient = base.with_project_types([&types]);
    let mut requires = HashMap::new();
    requires.insert("sub".to_string(), export_ty);

    // Erased to `unknown`: an undefined-field read on it is lenient, not
    // "undefined field `nope` on `Event`".
    let lenient = "\
local s = require(\"sub\")
local _ = s.item.nope
";
    assert_eq!(
        codes(&check(lenient, &ambient, &requires)),
        Vec::<String>::new(),
        "an erased (unknown) member must not read as the real `Event` class"
    );

    // …and it cannot supply a concrete-typed parameter either — `unknown`,
    // not `Event`'s real (and here coincidentally well-typed) `id: number`.
    let mismatched = "\
---@param n number
local function want(n) end
local s = require(\"sub\")
want(s.item.id)
";
    assert_eq!(
        codes(&check(mismatched, &ambient, &requires)),
        vec!["LB0300"],
        "`item` must be `unknown`, not the real `Event`, so `.id` must not type-check as `number`"
    );
}

#[test]
fn undefined_member_on_a_cross_file_class_names_the_remedy_and_the_declaring_file() {
    // F71 (round 3 review): the undefined-field diagnostic's "declared here"
    // secondary was populated only by `absorb_block` (same-file classes), so
    // a cross-file consumer's `p.nope` — previously-accepted Lua turned into
    // a CI failure by #56, the case the largest number of users meet first —
    // named a class that appears nowhere in their own file, with no pointer
    // to `mod.lua` and no statement of what to write instead. The class's
    // own file/span now travel through `FileTypes`/`merge_file_types`, and
    // the message itself names the three ways to add the member.
    const PLAIN: &str = "\
---@class Point
---@field x number
local P = {}
return P
";
    let (export_ty, types) = surface(PLAIN, stdlib());
    let ambient = stdlib().with_project_types([&types]);
    let mut requires = HashMap::new();
    requires.insert("point".to_string(), export_ty);

    let consumer = "\
local p = require(\"point\")
local _ = p.nope
";
    let diags = check(consumer, &ambient, &requires);
    assert_eq!(codes(&diags), vec!["LB0306"]);
    let label = diags[0].primary_label().expect("primary label");
    assert!(
        label.message.contains("---@field")
            && label.message.contains("function")
            && label.message.contains("[string]"),
        "the label must name all three remedies: {}",
        label.message
    );
    let secondary = diags[0]
        .labels
        .iter()
        .find(|l| !l.primary)
        .unwrap_or_else(|| panic!("expected a \"declared here\" secondary label: {diags:#?}"));
    assert_eq!(
        secondary.span.file, "mod.lua",
        "the secondary label must point at the class's OWN file, not the consumer's: {diags:#?}"
    );
    assert!(
        secondary.message.contains("`Point` declared here"),
        "{}",
        secondary.message
    );
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

// --- cross-file method carrier tags (#33) ----------------------------------
//
// `---@deprecated`/`---@async` written on a `function Class:method()` carrier
// must reach a *consumer* file's `obj:method()` call. The tags ride the class
// surface through [`Ambient::with_project_types`] like the signature itself —
// including when the method is also `---@field`-declared, where the
// declaration shadows the carrier on type but inherits its tags.

/// A class whose tagged methods are declared as `---@field`s *and* defined,
/// exported for a consumer to require.
const TAGGED_CLASS_MODULE: &str = "\
---@class Session
---@field close fun(self: Session)
---@field fetch fun(self: Session)
local Session = {}
Session.__index = Session

---@deprecated
function Session:close() end

---@async
function Session:fetch() end

---@return Session
function Session.new()
  return setmetatable({}, Session)
end

return Session
";

/// Assemble the merged ambient + registry for [`TAGGED_CLASS_MODULE`] and
/// strict-check `consumer` against them.
fn check_tagged(consumer: &str) -> Vec<Diagnostic> {
    let base = stdlib();
    let (export, types) = surface(TAGGED_CLASS_MODULE, base);
    let ambient = base.with_project_types([&types]);
    let mut requires = HashMap::new();
    requires.insert("session".to_string(), export);
    check(consumer, &ambient, &requires)
}

#[test]
fn cross_file_deprecated_method_flagged_at_the_consumer() {
    let consumer = "\
---@type Session
local s
s:close()
";
    assert_eq!(codes(&check_tagged(consumer)), vec!["LB0308"]);
}

#[test]
fn cross_file_async_method_flagged_at_the_consumer() {
    let consumer = "\
---@type Session
local s
local function sync()
  s:fetch()
end
";
    assert_eq!(codes(&check_tagged(consumer)), vec!["LB0316"]);
}

#[test]
fn cross_file_async_method_in_an_async_consumer_is_clean() {
    let consumer = "\
---@type Session
local s
---@async
local function poll()
  s:fetch()
end
";
    assert_eq!(codes(&check_tagged(consumer)), Vec::<String>::new());
}

#[test]
fn cross_file_method_tags_keep_the_declared_signature() {
    // The carry-over adds tags only: the `---@field` declaration still governs
    // arity, so a surplus argument is still LB0301 at the consumer.
    let consumer = "\
---@type Session
local s
s:close(1)
";
    assert_eq!(codes(&check_tagged(consumer)), vec!["LB0308", "LB0301"]);
}

// --- where a free type parameter can hide inside a member's type -----------
//
// `class_params_in_scope` decides whether an export keeps its class identity
// (#56 enforcement) or crosses as the erased template, by walking the
// *resolved* shape for parameters nothing binds. The walk has one arm per
// composite type, and a missing arm is invisible in the ordinary case: the
// parameter simply is not found, the class keeps its name, and the export
// becomes STRICTER — an undeclared member starts reporting LB0306 instead of
// staying lenient. Each fixture below hides the parameter one level down in a
// different composite, so a deleted arm is a failing test rather than a
// silent tightening. (Measured: each was a surviving mutant of the
// `Ty::Union` / `Ty::Table` / `Ty::Function` arms before these landed.)

/// Assert a carrier whose only mention of `T` is inside `member_decl` crosses
/// as the erased template: an undeclared read stays lenient, and the declared
/// member does not leak the parameter name into the consumer.
fn generic_member_erases(member_decl: &str) -> Vec<String> {
    let module = format!(
        "\
---@class Hidden<T>
{member_decl}
local H = {{}}
return H
"
    );
    let (export_ty, types) = surface(&module, stdlib());
    let ambient = stdlib().with_project_types([&types]);
    let mut requires = HashMap::new();
    requires.insert("hidden".to_string(), export_ty);

    let consumer = "\
local h = require(\"hidden\")
local _ = h.nope
";
    codes(&check(consumer, &ambient, &requires))
}

#[test]
fn a_parameter_inside_a_union_member_still_erases_the_export() {
    assert_eq!(
        generic_member_erases("---@field maybe T|nil"),
        Vec::<String>::new()
    );
}

#[test]
fn a_parameter_inside_a_nested_table_member_still_erases_the_export() {
    assert_eq!(
        generic_member_erases("---@field nested { inner: T }"),
        Vec::<String>::new()
    );
}

#[test]
fn a_parameter_inside_a_function_member_still_erases_the_export() {
    assert_eq!(
        generic_member_erases("---@field pick fun(): T"),
        Vec::<String>::new()
    );
}

#[test]
fn a_plain_member_is_the_control_for_the_three_hiding_places() {
    // One variable against the three above: the same carrier with no type
    // parameter anywhere keeps its class identity, so the identical read IS
    // LB0306. Without this, "clean" above could mean the rule never ran.
    const PLAIN: &str = "\
---@class Shown
---@field maybe number|nil
local S = {}
return S
";
    let (export_ty, types) = surface(PLAIN, stdlib());
    let ambient = stdlib().with_project_types([&types]);
    let mut requires = HashMap::new();
    requires.insert("shown".to_string(), export_ty);

    let consumer = "\
local s = require(\"shown\")
local _ = s.nope
";
    assert_eq!(codes(&check(consumer, &ambient, &requires)), vec!["LB0306"]);
}

#[test]
fn an_unknown_name_in_a_parent_argument_is_reported_once_not_twice() {
    // The parent reference is lowered twice — once for its diagnostics, once
    // to capture the bound arguments — and the second pass rolls its
    // diagnostics back (`QuietMark`). Without the rollback the same LB0305
    // is reported twice for one written mistake. Measured: a surviving
    // mutant replaced `QuietMark::rollback` with a no-op and no test noticed.
    let consumer = "\
---@class Holder<H>
---@field held H
---@class Bad : Holder<Nope>
local B = {}
";
    assert_eq!(
        codes(&check(consumer, stdlib(), &HashMap::new())),
        ["LB0305"]
    );
}

#[test]
fn the_declaring_file_label_survives_the_surface_clone() {
    // #56's undefined-field message points a secondary label at the file that
    // declares the class, which a consumer's own file never names. The span
    // map is repopulated by `merge_file_types` for every file merged in the
    // same call, so a single-layer fixture cannot see whether `clone_surface`
    // carries it — the merge would put it back. **Layering is what
    // separates them**: the second `with_project_types` merges only the
    // second file, so the first file's declaring span survives solely by
    // being cloned off the base. Measured: a surviving mutant dropped that
    // field from the clone, and the single-layer shape stayed green.
    const DECLARING: &str = "\
---@class Labelled
---@field item number
local L = {}
return L
";
    const LATER: &str = "\
---@class Unrelated
---@field other number
local U = {}
return U
";
    let (export_ty, types) = surface(DECLARING, stdlib());
    let (_, later_types) = surface(LATER, stdlib());
    let ambient = stdlib()
        .with_project_types([&types])
        .with_project_types([&later_types]);
    let mut requires = HashMap::new();
    requires.insert("labelled".to_string(), export_ty);

    let consumer = "\
local b = require(\"labelled\")
local _ = b.nope
";
    let diags = check(consumer, &ambient, &requires);
    assert_eq!(codes(&diags), vec!["LB0306"]);
    assert!(
        diags[0]
            .labels
            .iter()
            .any(|l| !l.primary && l.span.file == "mod.lua"),
        "the declaring file must be labelled: {:?}",
        diags[0].labels
    );
}

// --- diamond over a generic ancestor: one edge binds, one leaves bare ------
//
// Round 4 review R1 (finding 1, this pass): `collect_class`'s `free`
// accumulator is a straight union over every edge it ever visits, but
// `shape.fields`/`shape.indexers` are last-edge-wins (R1/R3). A diamond where
// one parent binds a generic ancestor's parameter and a sibling parent
// leaves it bare contributes that parameter to `free` on the bare edge, and
// nothing removes it once the bound edge overwrites the field it produced —
// so `free` and the winning shape disagree about whether the parameter is
// still open. `class_params_in_scope` (which `reify_export` reads to decide
// class identity, #56) then reports a parameter that is not actually free in
// the resolved shape, and since the parameter happens to be spelled like a
// real, unrelated class (`V`), `reify_export`'s substitution erases every
// occurrence of `V` it finds — including `Ctop`'s own `other: V` field,
// which names the real class, not the stale template parameter.
//
// Declaration order is the signature of the bug: `Ctop : Bone, Aone` visits
// the bare edge (Bone -> Base, contributing "V" to the pre-fix `free`)
// before the bound edge (Aone -> Base<number>, which overwrites `item` in
// `shape` but never retracted "V" from `free`) — pre-fix, this order
// silently loses the read. Swapping to `Ctop : Aone, Bone` visits the two
// edges the other way around, and there `free`'s pre-fix (buggy) value and
// its post-fix (retracting) value are the SAME set, because the *last*
// visit to `Base` in this order is the bare one (`Bone`) — the established,
// separately-pinned "last-listed-parent-wins" field-merge rule
// (`duplicate_class_merge.rs`'s `conflicting_generic_diamond_bindings_...`)
// already makes the bare edge win `item` on its own, with or without this
// fix. Both orders are exercised below, on purpose, and they pin DIFFERENT
// outcomes: that asymmetry is what proves the fix tracks `free` per-visit
// rather than merely happening to paper over the one order in the original
// report.

/// The winning-item's underlying merge-rule outcome for a given parent
/// order — proof of *why* the two directions below expect different
/// results: which edge (`Base<number>` or bare `Base`) is last-listed
/// decides whether `item` ends up bound or genuinely free, independent of
/// this fix (`duplicate_class_merge.rs` pins the same rule for a
/// non-generic diamond).
fn diamond_item_type_matches_the_last_listed_parent(parents: &str, want_bound: bool) {
    let module = diamond_over_generic_ancestor(parents);
    let (export_ty, types) = surface(&module, stdlib());
    let ambient = stdlib().with_project_types([&types]);
    let mut requires = HashMap::new();
    requires.insert("mod".to_string(), export_ty);

    // `item` is optional (`item? V`), so a mismatched-type use always
    // reports `LB0300` — bound reports "found `number`", free reports
    // "found `unknown|nil`" (matching
    // `generic_carrier_export_never_names_its_unbound_parameter`'s
    // `unknown` contrast, `|nil` for the field's own optionality). Read the
    // message rather than the bare code so bound-vs-free is unambiguous
    // either way, and confirm the free case never leaks the parameter's own
    // spelling (`V`) into the consumer.
    let consumer = "\
---@param s string
local function want(s) end
local m = require(\"mod\")
want(m.item)
";
    let diags = check(consumer, &ambient, &requires);
    assert_eq!(codes(&diags), vec!["LB0300"], "{parents}: {diags:?}");
    if want_bound {
        assert!(
            diags[0].message.contains("found `number"),
            "{parents}: item must be bound `number`: {}",
            diags[0].message
        );
    } else {
        assert!(
            diags[0].message.contains("found `unknown"),
            "{parents}: item must be erased to `unknown`, not leak a stale bind: {}",
            diags[0].message
        );
        assert!(
            !diags[0].message.contains('V'),
            "{parents}: an inherited parameter must not reach the consumer either: {}",
            diags[0].message
        );
    }
}

/// Build the diamond module: `Base<V>` is a generic ancestor with a `V`-typed
/// field, reached twice from `Ctop` — once through a parent that binds `V`
/// to `number` (`Aone`), once through a parent that leaves it bare (`Bone`).
/// `Ctop` also declares its own field `other`, typed with the REAL class `V`
/// (`---@class V` / `---@field payload string`), unrelated to the template
/// parameter that merely happens to share its spelling. `parents` lets each
/// direction of the diamond reuse this builder.
fn diamond_over_generic_ancestor(parents: &str) -> String {
    format!(
        "\
---@class V
---@field payload string
---@class Base<V>
---@field item? V
---@class Aone : Base<number>
---@class Bone : Base
---@class Ctop : {parents}
---@field other V
local M = {{}}
return M
"
    )
}

/// `Ctop`'s own field `other` names the real class `V`. Diagnostic codes for
/// an undeclared read off it — `["LB0306"]` proves `other` kept its real `V`
/// identity; an empty result means the export lost it (either the stale
/// pre-fix bug, or the separate, still-open name-collision gap the second
/// order below documents).
fn diamond_undeclared_field_is_reported(parents: &str) -> Vec<String> {
    let module = diamond_over_generic_ancestor(parents);
    let (export_ty, types) = surface(&module, stdlib());
    let ambient = stdlib().with_project_types([&types]);
    let mut requires = HashMap::new();
    requires.insert("mod".to_string(), export_ty);

    let consumer = "\
local m = require(\"mod\")
local _ = m.other.nope
";
    codes(&check(consumer, &ambient, &requires))
}

#[test]
fn a_diamond_binding_v_last_reports_the_undeclared_read_and_keeps_item_bound() {
    // `Ctop : Bone, Aone` — the finder's exact repro. The bare edge
    // (Bone -> Base) is visited first, the binding edge (Aone ->
    // Base<number>) last, so `item` ends up genuinely bound to `number` and
    // `Base`'s `V` has nothing left free. Pre-fix, `free` still carried "V"
    // from the superseded first visit, so the export erased anyway —
    // dragging `other`'s real, unrelated `V` class down with it and
    // reporting 0 errors instead of `LB0306`.
    diamond_item_type_matches_the_last_listed_parent("Bone, Aone", true);
    assert_eq!(
        diamond_undeclared_field_is_reported("Bone, Aone"),
        vec!["LB0306"],
        "Bone-then-Aone: `other`'s real `V` identity must survive the stale \
         free-parameter contribution from the earlier bare edge"
    );
}

#[test]
fn a_diamond_binding_v_first_leaves_item_free_and_other_pays_the_known_collision() {
    // `Ctop : Aone, Bone` — the opposite order. The binding edge runs
    // first, the bare edge (Bone -> Base) runs last, so — independent of
    // this fix, per the already-pinned "last-listed-parent-wins" merge rule
    // — Bone's bare reference wins `item`, exactly as a lone bare parent
    // does (`a_parent_written_bare_leaves_its_parameter_unbound_and_erased`):
    // `item` is genuinely, correctly free, and `Ctop`'s export legitimately
    // erases at the #56 seam.
    diamond_item_type_matches_the_last_listed_parent("Aone, Bone", false);

    // This is where this fix's job ends and a separate, narrower, KNOWN
    // limitation begins: the erasure substitutes every `Ty::Named("V")` it
    // finds in the resolved shape, and the resolved shape cannot, by
    // representation alone, distinguish "the ancestor's genuinely-free
    // template parameter" from "a field that happens to name the real class
    // `V`" once both have collapsed to the identical `Ty::Named("V")` value.
    // `other` pays for that collision here — same as it would for a lone
    // (non-diamond) bare `---@class Sub : Base` whose sibling field also
    // happened to be spelled like `Base`'s own parameter. This is NOT a
    // regression: `free` already, coincidentally, agreed with `shape` for
    // THIS order before this fix (the last-visited edge for `Base` is the
    // bare one either way), so this fix changes nothing observable here —
    // it only retracts a stale contribution when the LAST visit disagrees
    // with an EARLIER one, which is precisely the other test's order, not
    // this one.
    assert_eq!(
        diamond_undeclared_field_is_reported("Aone, Bone"),
        Vec::<String>::new(),
        "Aone-then-Bone: item is genuinely free by the existing merge rule, \
         so the export erases — other's collision with the same spelling is \
         a separate, pre-existing representational gap, not this fix's job"
    );
}

#[test]
fn a_plain_class_is_the_control_for_the_diamond_over_a_generic_ancestor() {
    // No diamond, no generic ancestor at all: `Ctop` declared with no
    // parents. This must behave identically to the two diamond fixtures
    // above — same read, same `LB0306` — proving the rule runs at all and
    // is not simply lenient by default.
    const PLAIN: &str = "\
---@class V
---@field payload string
---@class Ctop
---@field other V
local M = {}
return M
";
    let (export_ty, types) = surface(PLAIN, stdlib());
    let ambient = stdlib().with_project_types([&types]);
    let mut requires = HashMap::new();
    requires.insert("mod".to_string(), export_ty);

    let consumer = "\
local m = require(\"mod\")
local _ = m.other.nope
";
    assert_eq!(codes(&check(consumer, &ambient, &requires)), vec!["LB0306"]);
}
