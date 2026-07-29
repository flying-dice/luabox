// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]
//! `---@nodiscard` — luals' `discard-returns` rule, `LB0309` (#112).
//!
//! A call to a `---@nodiscard` function in *statement* position throws its
//! result away, which for such a function is the mistake the tag exists to
//! catch. Advisory like `---@deprecated`: always a `Warning`, suppressible by
//! `---@diagnostic disable*: discard-returns`.

use std::collections::HashMap;

use luabox_diag::Diagnostic;
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::ty::Ty;
use luabox_types::{Strictness, check_file, check_file_with_requires, module_surface};

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

/// The `require`-export registry for a single module, for the cross-file
/// nodiscard tests (mirrors [`deprecated_cross_file_via_require`]).
fn require_registry(module: &str, src: &str) -> HashMap<String, Ty> {
    let parse = parse(src, Dialect::Lua54);
    assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
    let export = module_surface(&parse, "mod.lua", None)
        .export
        .expect("module exports a table");
    let mut requires = HashMap::new();
    requires.insert(module.to_string(), export);
    requires
}

#[test]
fn nodiscard_bare_call_flagged() {
    let src = "\
---@nodiscard
---@return boolean
local function save() return true end
save()
";
    assert_eq!(codes(src, Strictness::Warn), vec!["LB0309"]);
}

#[test]
fn nodiscard_bound_result_is_clean() {
    let src = "\
---@nodiscard
---@return boolean
local function save() return true end
local ok = save()
";
    assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
}

#[test]
fn nodiscard_used_in_expression_is_clean() {
    let src = "\
---@nodiscard
---@return boolean
local function save() return true end
if save() then end
";
    assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
}

#[test]
fn nodiscard_suppressed_by_directive() {
    let src = "\
---@nodiscard
---@return boolean
local function save() return true end
---@diagnostic disable-next-line: discard-returns
save()
";
    assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
}

#[test]
fn nodiscard_cross_file_via_require() {
    // `old.important` is nodiscard in the required module; the flag rides
    // the module's export type into the consumer, so a bare call statement
    // in the consumer is a discard (#112 parity with LB0308's reach).
    let requires = require_registry(
        "old",
        "local M = {}\n---@nodiscard\n---@return number\nfunction M.important() return 2 end\nreturn M\n",
    );
    let main = parse(
        "local old = require(\"old\")\nold.important()\n",
        Dialect::Lua54,
    );
    let diags = check_file_with_requires(
        &main,
        "main.lua",
        Strictness::Warn,
        Dialect::Lua54,
        None,
        &requires,
    );
    let codes: Vec<String> = diags.iter().map(|d| d.code.to_string()).collect();
    assert_eq!(codes, vec!["LB0309"]);
}

#[test]
fn nodiscard_cross_file_via_require_bound_is_clean() {
    // Binding the result accepts it — no discard, exactly as same-file.
    let requires = require_registry(
        "old",
        "local M = {}\n---@nodiscard\n---@return number\nfunction M.important() return 2 end\nreturn M\n",
    );
    let main = parse(
        "local old = require(\"old\")\nlocal n = old.important()\nprint(n)\n",
        Dialect::Lua54,
    );
    let diags = check_file_with_requires(
        &main,
        "main.lua",
        Strictness::Warn,
        Dialect::Lua54,
        None,
        &requires,
    );
    let codes: Vec<String> = diags.iter().map(|d| d.code.to_string()).collect();
    assert_eq!(codes, Vec::<String>::new());
}
