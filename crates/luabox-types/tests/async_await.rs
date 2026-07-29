//! `---@async` — luals' `await-in-sync` rule, `LB0316`.
//!
//! Calling an `---@async` function from an enclosing function that is not
//! itself `---@async` is the await-in-sync mistake. The enclosing context is
//! what decides: at chunk top level, or inside another `---@async` function,
//! the same call is clean.

use luabox_diag::{Diagnostic, Severity};
use luabox_syntax::lua;
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::{Strictness, check_file, check_file_with_ambient, stdlib_defs};

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

/// Strict check with the Lua 5.4 stdlib ambient layer (its
/// `setmetatable` signature must not mask inference).
fn strict_codes_ambient(src: &str) -> Vec<String> {
    let parse = lua::parse(src, lua::Dialect::Lua54);
    assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
    check_file_with_ambient(
        &parse,
        "test.lua",
        Strictness::Strict,
        lua::Dialect::Lua54,
        Some(stdlib_defs(lua::Dialect::Lua54)),
    )
    .iter()
    .map(|d| d.code.to_string())
    .collect()
}

#[test]
fn async_call_in_sync_function_flagged() {
    let src = "\
---@async
local function fetch() end
local function sync()
  fetch()
end
";
    assert_eq!(codes(src, Strictness::Warn), vec!["LB0316"]);
}

#[test]
fn async_call_in_async_function_is_clean() {
    let src = "\
---@async
local function fetch() end
---@async
local function loadAll()
  fetch()
end
";
    assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
}

#[test]
fn async_call_at_top_level_is_clean() {
    // The main chunk is an async context in luals, so a top-level call to
    // an async function is not flagged.
    let src = "\
---@async
local function fetch() end
fetch()
";
    assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
}

#[test]
fn async_call_in_nested_sync_closure_flagged() {
    // A closure nested inside an async function is itself sync (unannotated),
    // so an async call within it is flagged — the *nearest* enclosing
    // function governs, matching `guide.getParentFunction`.
    let src = "\
---@async
local function fetch() end
---@async
local function loadAll()
  local cb = function()
    fetch()
  end
  cb()
end
";
    assert_eq!(codes(src, Strictness::Warn), vec!["LB0316"]);
}

#[test]
fn async_dotted_call_flagged() {
    let src = "\
local M = {}
---@async
function M.fetch() end
local function sync()
  M.fetch()
end
";
    assert_eq!(codes(src, Strictness::Warn), vec!["LB0316"]);
}

#[test]
fn async_method_call_flagged() {
    // A `:`-method resolved to an `---@async` signature, called from a
    // non-async function.
    let src = "\
---@class Client
local Client = {}
Client.__index = Client

---@async
function Client:fetch() end

---@return Client
function Client.new()
  return setmetatable({}, Client)
end

local function sync()
  local c = Client.new()
  c:fetch()
end
";
    assert_eq!(strict_codes_ambient(src), vec!["LB0316"]);
}

#[test]
fn non_async_call_is_clean() {
    // A callee with no `---@async` tag is never flagged, in any context.
    let src = "\
local function plain() end
local function sync()
  plain()
end
";
    assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
}

#[test]
fn async_in_sync_is_warning_even_under_strict() {
    // luals-Warning parity: LB0316 stays a Warning under strict mode, never
    // escalating on the strictness ladder (like LB0308/LB0309).
    let src = "\
---@async
local function fetch() end
local function sync()
  fetch()
end
";
    let diags = check(src, Strictness::Strict);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code.to_string(), "LB0316");
    assert_eq!(diags[0].severity, Severity::Warning);
}

#[test]
fn async_in_sync_suppressed_by_directive() {
    let src = "\
---@async
local function fetch() end
local function sync()
  ---@diagnostic disable-next-line: await-in-sync
  fetch()
end
";
    assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
}

#[test]
fn async_bare_tag_adds_no_arity_error() {
    // A bare `---@async` block (no `---@param`/`---@return`) must not enable
    // arity checking on the function it annotates.
    let src = "\
---@async
local function fetch(a, b) end
fetch(1)
";
    assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
}
