//! Callable classes: `---@operator call` (#122).
//!
//! A value whose type is a `---@class` declaring an `---@operator call` may be
//! called, and the operator's declared result is the call's result type.
//! Overloaded `call` operators select first-match on the argument, mirroring
//! binary-operator selection; a class without one stays uncallable.

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
fn inherited_operator_from_a_bound_generic_parent_substitutes_its_parameter() {
    // F37: `collect_operators` performed no substitution at all, unlike its
    // sibling `collect_class`. `---@class Base<U>` / `---@operator add(U): U`
    // inherited by `---@class Sub : Base<number>` left `U` free — the
    // operand's declared type stayed the unresolvable `Ty::Named("U")`, so no
    // overload ever matched a real argument and `s + 5` fell through to
    // `unknown` instead of `Base<number>`'s actual `add(number): number` —
    // the exact leak the #56 export-seam fix closed for fields, surviving on
    // the operator path.
    let src = "\
---@class Base<U>
---@operator add(U): U
local B = {}
---@class Sub : Base<number>
local S = {}
---@type Sub
local s = S
---@type number
local n = s + 5
";
    assert_eq!(
        strict_codes(src),
        Vec::<String>::new(),
        "the parent's bound argument must substitute into the inherited operator's signature"
    );

    // The rejecting probe beside the accepting one: the substituted result is
    // genuinely `number`, not leniently `unknown` (an argument-shape mismatch
    // on a binary operator has no dedicated diagnostic by design — no
    // matching overload just falls back to `unknown`, per
    // `Infer::operator_binary`'s doc comment — so the result-mismatch
    // assignment below is the probe that distinguishes "substituted to
    // `number`" from "erased to `unknown`").
    let wrong_result = "\
---@class Base<U>
---@operator add(U): U
local B = {}
---@class Sub : Base<number>
local S = {}
---@type Sub
local s = S
---@type string
local n = s + 5
";
    assert_eq!(strict_codes(wrong_result), vec!["LB0300"]);
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
