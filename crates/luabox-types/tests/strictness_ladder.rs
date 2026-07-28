// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]
//! The strictness ladder (SPEC.md §3) and end-to-end diagnostic shape.
//!
//! `none` suppresses type diagnostics entirely; `warn` reports mismatches as
//! warnings and lets `unknown` flow both ways; `strict` reports them as errors
//! and treats `unknown -> T` as a mismatch of its own. The end-to-end tests
//! pin what a diagnostic carries: file, span, and the exact rendered range.

use luabox_diag::{Diagnostic, Severity};
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
fn warn_mode_downgrades_severity() {
    let src = "\
---@param n number
local function f(n) end
f(\"no\")
";
    let warn = check(src, Strictness::Warn);
    assert_eq!(warn.len(), 1);
    assert_eq!(warn[0].severity, Severity::Warning);
    let strict = check(src, Strictness::Strict);
    assert_eq!(strict[0].severity, Severity::Error);
}

#[test]
fn unknown_argument_strict_vs_warn() {
    let src = "\
---@param n number
local function f(n) end
local x = tonumber(\"3\")
f(x)
";
    // Warn: `unknown` flows freely. Strict: unknown -> number errors.
    assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
    assert_eq!(codes(src, Strictness::Strict), vec!["LB0300"]);
}

#[test]
fn none_suppresses_everything() {
    let src = "\
---@param n number
local function f(n) end
f(\"no\")
";
    assert_eq!(codes(src, Strictness::None), Vec::<String>::new());
}

#[test]
fn strictness_from_manifest_flag() {
    assert_eq!(Strictness::from_manifest_flag(true), Strictness::Strict);
    assert_eq!(Strictness::from_manifest_flag(false), Strictness::Warn);
}

#[test]
fn clean_annotated_file_has_no_diagnostics() {
    let src = "\
---@class Greeter
---@field name string
---@field excited? boolean

---@param g Greeter
---@return string
local function greet(g)
  return g.name
end

---@type Greeter
local g = { name = \"lua\" }
greet(g)
greet({ name = \"world\", excited = true })
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn diagnostics_carry_file_and_span() {
    let src = "\
---@param n number
local function f(n) end
f(\"no\")
";
    let diags = check(src, Strictness::Strict);
    let label = diags[0].primary_label().expect("primary label");
    assert_eq!(label.span.file, "test.lua");
    assert_eq!(&src[label.span.range.clone()], "\"no\"");
}
