//! Table literals checked field-by-field against a `---@class` shape.
//!
//! The payoff of the structural table representation (SPEC.md §3): a table
//! literal flowing into a `---@class` slot is not "a table" but a set of
//! fields, so a wrong field type is `LB0300` on that field, a missing required
//! field is `LB0302`, and an undeclared one is `LB0303` — all pointed at the
//! entry that is wrong, not at the whole literal.

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

const POINT: &str = "\
---@class Point
---@field x number
---@field y number
---@field label? string

---@param p Point
local function use(p) end
";

#[test]
fn table_literal_missing_required_field() {
    let src = format!("{POINT}use({{ x = 1 }})\n");
    let diags = check(&src, Strictness::Strict);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB0302");
    assert!(diags[0].message.contains("`y`"), "{}", diags[0].message);
}

#[test]
fn table_literal_unknown_field() {
    let src = format!("{POINT}use({{ x = 1, y = 2, z = 3 }})\n");
    let diags = check(&src, Strictness::Strict);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB0303");
    assert!(diags[0].message.contains("`z`"), "{}", diags[0].message);
}

#[test]
fn table_literal_field_type_mismatch() {
    let src = format!("{POINT}use({{ x = 1, y = \"two\" }})\n");
    let diags = check(&src, Strictness::Strict);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB0300");
}

#[test]
fn table_literal_optional_field_may_be_absent() {
    let src = format!("{POINT}use({{ x = 1, y = 2 }})\n");
    assert_eq!(strict_codes(&src), Vec::<String>::new());
}

#[test]
fn each_field_problem_is_its_own_diagnostic() {
    let src = format!("{POINT}use({{ y = \"two\", z = 3 }})\n");
    let mut found: Vec<String> = strict_codes(&src);
    found.sort();
    assert_eq!(found, vec!["LB0300", "LB0302", "LB0303"]);
}

#[test]
fn inherited_fields_count() {
    let src = "\
---@class Base
---@field id number

---@class Derived: Base
---@field name string

---@param d Derived
local function f(d) end
f({ name = \"x\" })
f({ id = 1, name = \"x\" })
";
    assert_eq!(strict_codes(src), vec!["LB0302"]);
}

#[test]
fn indexer_keeps_class_open() {
    let src = "\
---@class Bag
---@field size number
---@field [string] boolean

---@param b Bag
local function f(b) end
f({ size = 1, extra = true })
f({ size = 1, extra = 3 })
";
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}

#[test]
fn array_items_checked_against_array_part() {
    let src = "\
---@param xs string[]
local function f(xs) end
f({ \"a\", \"b\" })
f({ \"a\", 2 })
";
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}

#[test]
fn nested_table_literal_checked_field_by_field() {
    let src = "\
---@class Inner
---@field n number

---@class Outer
---@field inner Inner

---@param o Outer
local function f(o) end
f({ inner = { n = \"no\" } })
";
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}
