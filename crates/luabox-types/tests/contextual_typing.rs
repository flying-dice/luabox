//! Bidirectional (contextual) typing of function literals (#120).
//!
//! A lambda written where a `fun(...)` is expected takes its parameter types
//! from that expectation, so its body checks with no per-parameter annotation.
//! The expectation propagates into table literals, through `return` position,
//! and across nested callback layers. Conservative: no expected function type
//! leaves the parameters `unknown` exactly as before, and an explicit
//! `---@param` on the lambda always wins.

use luabox_diag::Diagnostic;
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::{Strictness, check_file};

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
