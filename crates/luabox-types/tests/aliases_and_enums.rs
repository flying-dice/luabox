//! `---@alias` and `---@enum` resolution, unknown type names (`LB0305`), and
//! self- or mutually-referential alias cycles (`LB0314`, #123).
//!
//! An alias is a name for a type expression, so it must resolve transitively
//! and an unresolvable name must be named as such rather than silently
//! becoming `unknown`. A cycle has no fixpoint: it is reported once, at the
//! declaration, instead of hanging the resolver.

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
fn enum_member_satisfies_enum_param() {
    let src = "\
---@enum Color
local Color = {
  red = 1,
  green = 2,
}
---@param c Color
local function paint(c) end
paint(Color.red)
paint(2)
paint(9)
";
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}

#[test]
fn alias_literal_union() {
    let src = "\
---@alias Mode \"fast\"|\"slow\"
---@param m Mode
local function run(m) end
run(\"fast\")
run(\"medium\")
";
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}

#[test]
fn alias_multiline_literal_quote_forms() {
    // #116: the multiline `---|` member forms `'"x"'`, `"x"`, and `'x'`
    // all denote the literal value `x` — the wrapping quotes are syntax,
    // not content — so every valid call is accepted and only the odd one
    // out is rejected.
    let src = "\
---@alias Level
---| '\"debug\"' # verbose
---| \"info\"
---| 'warn'

---@param l Level
local function set_level(l) end
set_level(\"debug\")
set_level(\"info\")
set_level(\"warn\")
set_level(\"nope\")
";
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}

#[test]
fn direct_cyclic_alias_reported_once() {
    // `---@alias Loop Loop` can never terminate expanding into something
    // real (`expand_alias`'s cycle guard collapses it to `unknown`), so
    // it is LB0314 — reported once even though two annotations reference
    // it (never once per reference site).
    let src = "\
---@alias Loop Loop
---@param a Loop
---@param b Loop
local function f(a, b) end
";
    assert_eq!(strict_codes(src), vec!["LB0314"]);
}

#[test]
fn mutual_cyclic_alias_reported() {
    // `---@alias A B` + `---@alias B A`: neither alias ever bottoms out,
    // so expanding the referenced one (`A`) trips the cycle guard.
    let src = "\
---@alias A B
---@alias B A
---@param x A
local function f(x) end
";
    assert_eq!(strict_codes(src), vec!["LB0314"]);
}

#[test]
fn non_cyclic_alias_is_clean() {
    // The safe-termination and reporting changes must not touch an
    // ordinary alias used in a `---@param`.
    let src = "\
---@alias Status \"on\"|\"off\"
---@param s Status
local function f(s) end
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn unknown_type_name_reported() {
    let src = "\
---@param x Wibble
local function f(x) end
";
    assert_eq!(strict_codes(src), vec!["LB0305"]);
}

#[test]
fn generic_params_do_not_report_lb0305() {
    let src = "\
---@generic T
---@param x T
---@return T
local function id(x)
  return x
end
id(1)
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}
