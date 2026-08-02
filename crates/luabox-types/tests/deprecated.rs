// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]
//! `---@deprecated` — luals' `deprecated` rule, `LB0308` (#111).
//!
//! Reading a deprecated symbol is advisory: always a `Warning`, never
//! escalated by the strictness ladder (matching luals). It fires on reads of
//! deprecated functions, fields, classes and methods, and is suppressible by
//! `---@diagnostic disable*: deprecated`.

use std::collections::HashMap;

use luabox_diag::{Diagnostic, Severity};
use luabox_syntax::lua::{Dialect, parse};
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

fn strict_codes(source: &str) -> Vec<String> {
    codes(source, Strictness::Strict)
}

#[test]
fn deprecated_local_function_call_flagged() {
    let src = "\
---@deprecated
local function old() end
old()
";
    assert_eq!(codes(src, Strictness::Warn), vec!["LB0308"]);
}

#[test]
fn deprecated_dotted_function_call_flagged() {
    let src = "\
local M = {}
---@deprecated
function M.legacy() end
M.legacy()
";
    assert_eq!(codes(src, Strictness::Warn), vec!["LB0308"]);
}

#[test]
fn deprecated_value_reference_flagged() {
    let src = "\
---@deprecated
local function old() end
local alias = old
";
    // Referencing (not just calling) a deprecated function is a use.
    assert_eq!(codes(src, Strictness::Warn), vec!["LB0308"]);
}

#[test]
fn deprecated_use_inside_an_assignment_targets_index_is_flagged() {
    // #58 mutation audit (check.rs): an assignment TARGET is a write, but
    // its base/index sub-expressions are reads — `t[old()] = 1` uses `old`
    // even though nothing on the right-hand side does. This is the one
    // shape that reaches the target-traversal arm alone: a value-position
    // read goes through the ordinary expression walk instead.
    let src = "\
---@deprecated
local function old() end

local t = {}
t[old()] = 1
";
    assert_eq!(codes(src, Strictness::Warn), vec!["LB0308"]);
}

#[test]
fn deprecated_declaration_site_not_flagged() {
    let src = "\
---@deprecated
local function old() end
";
    // The declaration itself is never flagged — only uses are.
    assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
}

#[test]
fn deprecated_is_warning_even_under_strict() {
    let src = "\
---@deprecated
local function old() end
old()
";
    let diags = check(src, Strictness::Strict);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].severity, Severity::Warning);
    assert_eq!(diags[0].code.to_string(), "LB0308");
}

#[test]
fn deprecated_does_not_add_arity_errors() {
    // A bare `---@deprecated` block must not enable arity checking on an
    // otherwise unannotated function.
    let src = "\
---@deprecated
local function old(a, b) end
old(1, 2, 3, 4)
";
    assert_eq!(codes(src, Strictness::Warn), vec!["LB0308"]);
}

#[test]
fn deprecated_suppressed_by_directive() {
    let src = "\
---@deprecated
local function old() end
---@diagnostic disable-next-line: deprecated
old()
";
    assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
}

#[test]
fn deprecated_suppressed_file_wide() {
    let src = "\
---@diagnostic disable: deprecated
---@deprecated
local function old() end
old()
old()
";
    assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
}

#[test]
fn non_deprecated_call_is_clean() {
    let src = "\
local function ok() end
ok()
";
    assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
}

#[test]
fn deprecated_cross_file_via_require() {
    // `Api.old` is deprecated in the required module; the flag rides the
    // module's export type into the consumer and flags the use site.
    let api = parse(
        "local Api = {}\n---@deprecated\nfunction Api.old() end\nreturn Api\n",
        Dialect::Lua54,
    );
    let export = module_surface(&api, "api.lua", None)
        .export
        .expect("module exports a table");
    let mut requires = HashMap::new();
    requires.insert("api".to_string(), export);
    let main = parse("local Api = require(\"api\")\nApi.old()\n", Dialect::Lua54);
    let diags = check_file_with_requires(
        &main,
        "main.lua",
        Strictness::Warn,
        Dialect::Lua54,
        None,
        &requires,
    );
    let codes: Vec<String> = diags.iter().map(|d| d.code.to_string()).collect();
    assert_eq!(codes, vec!["LB0308"]);
}

#[test]
fn async_cross_file_via_require() {
    // `Api.fetch` is `---@async` in the required module; the flag rides
    // the module's export type into the consumer, so calling it from a
    // sync function flags LB0316 (`callee_sig` cannot resolve required
    // members — the `check_call` fallback through the resolved
    // expression type covers this, like `deprecated`/`version` do).
    let api = parse(
        "local Api = {}\n---@async\nfunction Api.fetch() end\nreturn Api\n",
        Dialect::Lua54,
    );
    let export = module_surface(&api, "api.lua", None)
        .export
        .expect("module exports a table");
    let mut requires = HashMap::new();
    requires.insert("api".to_string(), export);
    let main = parse(
        "local Api = require(\"api\")\nlocal function sync()\n  Api.fetch()\nend\n",
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
    assert_eq!(codes, vec!["LB0316"]);
}

#[test]
fn multiple_vararg_tags_union_per_luals() {
    // Two legacy `---@vararg` tags on one block are two bound docs in
    // luals, and every bound doc merges into the `...` node — so their
    // types union, exactly like a `---@vararg` next to a `---@param ...`.
    let src = "\
---@vararg number
---@vararg string
local function f(...) end
f(1)
f(\"x\")
f(true)
";
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}
