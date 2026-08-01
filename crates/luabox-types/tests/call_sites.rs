//! Rule (a): call sites — arguments checked against `---@param`, arity
//! against the declared parameter list.
//!
//! `LB0300` for an argument that does not fit its parameter, `LB0301` for the
//! wrong number of them. Covers `---@overload` selection, varargs, optional
//! parameters, multi-value expansion of a trailing call, and the conservative
//! cases where an unannotated or `unknown` callee must produce nothing.

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
fn closest_overload_prefers_the_better_type_fit_at_equal_arity() {
    // Both candidates are arity-fit for `f(1, true)`, so ranking falls to
    // type fit: the overload matches one argument (`1` against `number`),
    // the primary matches none. Reporting against the overload leaves a
    // single mismatch on the second argument; the primary would have
    // produced two.
    let src = "\
---@param a string
---@param b string
---@overload fun(a: number, b: string)
local function f(a, b) end
f(1, true)
";
    let diags = check(src, Strictness::Strict);
    assert_eq!(
        diags.iter().map(|d| d.code.to_string()).collect::<Vec<_>>(),
        vec!["LB0300"],
        "only the second argument should mismatch: {diags:?}"
    );
    assert_eq!(diags[0].labels[0].message, "expected `string`");
}

#[test]
fn closest_overload_keeps_the_primary_on_a_tie() {
    // Equal arity fit, equal (zero) type fit: the declaration-order
    // tie-break keeps the primary, so the message names *its* parameter
    // type, not the overload's.
    let src = "\
---@param a string
---@overload fun(a: number)
local function f(a) end
f(true)
";
    let diags = check(src, Strictness::Strict);
    assert_eq!(
        diags.iter().map(|d| d.code.to_string()).collect::<Vec<_>>(),
        vec!["LB0300"]
    );
    assert_eq!(diags[0].labels[0].message, "expected `string`");
}

#[test]
fn closest_overload_arity_fit_outranks_the_primary_entirely() {
    // The primary cannot take two arguments at all; the overload can. The
    // reported *code set* changes as a result: against the primary this
    // would be a type mismatch plus LB0301 ("unexpected extra argument"),
    // against the overload it is two type mismatches and no arity finding.
    let src = "\
---@param a string
---@overload fun(a: number, b: number)
local function f(a) end
f(true, false)
";
    let diags = check(src, Strictness::Strict);
    assert_eq!(
        diags.iter().map(|d| d.code.to_string()).collect::<Vec<_>>(),
        vec!["LB0300", "LB0300"],
        "should report against the 2-arg overload, not the 1-arg primary"
    );
    assert!(
        diags
            .iter()
            .all(|d| d.labels[0].message == "expected `number`"),
        "both labels should name the overload's parameter type: {diags:?}"
    );
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

// ---------------------------------------------------------------------------
// A trailing parameter whose declared type admits `nil` is optional for arity
// (#46 follow-up). `---@param b number|nil` and `---@param b? number` say the
// same thing about what may reach `b`, and luals treats both as optional for
// the argument count; only the second spelling did here.
// ---------------------------------------------------------------------------

#[test]
fn a_trailing_nil_admitting_parameter_is_optional_for_arity() {
    let src = "\
---@param a number
---@param b number|nil
local function f(a, b) end
f(1)
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn the_question_mark_spelling_of_the_same_parameter_stays_optional() {
    // Control: the spelling that already worked keeps working.
    let src = "\
---@param a number
---@param b? number
local function g(a, b) end
g(1)
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn every_trailing_nil_admitting_parameter_is_optional() {
    let src = "\
---@param a number
---@param b number|nil
---@param c string|nil
local function f(a, b, c) end
f(1)
f(1, 2)
f(1, 2, \"x\")
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn a_question_mark_parameter_does_not_break_the_trailing_run() {
    let src = "\
---@param a number
---@param b? number
---@param c string|nil
local function f(a, b, c) end
f(1)
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn nil_anywhere_in_the_union_makes_the_parameter_optional() {
    let src = "\
---@param a number
---@param b string|nil|number
local function f(a, b) end
f(1)
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn an_alias_that_expands_to_a_nil_union_is_optional_too() {
    let src = "\
---@alias MaybeNum number|nil
---@param a number
---@param b MaybeNum
local function f(a, b) end
f(1)
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn a_nil_admitting_parameter_before_a_required_one_is_still_required() {
    // Deliberately conservative: only a *trailing* nil-admitting parameter is
    // optional for arity. A caller cannot skip a middle argument in Lua
    // without writing `nil` for it, so relaxing a non-trailing slot would let
    // a genuinely short call through. Documented in
    // docs/03-reference/02-limitations.md.
    let src = "\
---@param a number|nil
---@param b number
local function h(a, b) end
h(1)
";
    assert_eq!(strict_codes(src), vec!["LB0301"]);
}

#[test]
fn a_nil_admitting_parameter_supplied_explicitly_is_still_type_checked() {
    let src = "\
---@param a number
---@param b number|nil
local function f(a, b) end
f(1, \"nope\")
";
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}

#[test]
fn too_many_arguments_past_a_nil_admitting_parameter_is_still_reported() {
    let src = "\
---@param a number
---@param b number|nil
local function f(a, b) end
f(1, 2, 3)
";
    assert_eq!(strict_codes(src), vec!["LB0301"]);
}

#[test]
fn nil_admitting_is_explicit_nil_not_any() {
    // The boundary of the rule: `any` and `unknown` are *unconstrained*, not
    // declarations that the argument may be omitted, so they stay required.
    // Narrower than luals may be here, and deliberately so — see
    // docs/03-reference/02-limitations.md.
    let src = "\
---@param a number
---@param b any
local function f(a, b) end
f(1)
";
    assert_eq!(strict_codes(src), vec!["LB0301"]);
}

#[test]
fn a_nil_admitting_method_parameter_is_optional_too() {
    let src = "\
---@class Widget
local Widget = {}
---@param a number
---@param b number|nil
function Widget:draw(a, b) end
local w = Widget
w:draw(1)
return w
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}
