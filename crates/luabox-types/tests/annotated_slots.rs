//! Rules (b) and (c): the two non-call annotated slots.
//!
//! (b) a `---@type` local's initializer must fit the declared type, and later
//! assignments to that local are checked against it too (`LB0300`).
//! (c) every `return` expression must fit the function's `---@return` list,
//! in count and in type (`LB0304`).
//!
//! Also home to the local-function-literal binding pin (#58): an
//! unannotated `local f = function() end` binds as an (opaque) function,
//! not as `unknown` — the difference is invisible to rejecting probes
//! (strict mode rejects both against a scalar) and separable only by an
//! accepting one.

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

#[test]
fn an_unannotated_function_literal_local_binds_as_a_function() {
    // Behavioural pin (written during the #58 audit): an opaque function
    // satisfies a `fun()` parameter, while `unknown` would be rejected by
    // strict mode. Measured: this answer is served by INFERENCE — the P0
    // checker's own function-literal arm is shadowed (its mutant survives
    // this test), which is evidence for the shadowed-fallback finding, not
    // a kill.
    let src = "\
---@param g fun()
local function want_fun(g) end

local f = function() end
want_fun(f)
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn generic_arity_message_pluralizes_by_count() {
    // #58 mutation audit: the singular/plural pick in the LB0313 message
    // was the one unpinned observable of the arity report. The counts and
    // the code are asserted elsewhere; this pins the words.
    let src = "\
---@class Boxed<T>
---@field v T

---@type Boxed<string, number>
local b
print(b)
";
    let ds = check(src, Strictness::Strict);
    assert_eq!(ds.len(), 1, "{ds:?}");
    assert_eq!(ds[0].code.to_string(), "LB0313");
    assert!(
        ds[0].message.contains("takes 1 type argument,"),
        "singular for one parameter: {}",
        ds[0].message
    );
    assert!(
        ds[0].message.contains("2 were supplied"),
        "{}",
        ds[0].message
    );
}
