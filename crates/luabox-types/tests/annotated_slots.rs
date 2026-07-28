//! Rules (b) and (c): the two non-call annotated slots.
//!
//! (b) a `---@type` local's initializer must fit the declared type, and later
//! assignments to that local are checked against it too (`LB0300`).
//! (c) every `return` expression must fit the function's `---@return` list,
//! in count and in type (`LB0304`).

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
