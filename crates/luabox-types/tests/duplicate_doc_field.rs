//! `LB0311`: the same `---@field` declared twice on one class (#113) —
//! luals' `duplicate-doc-field`.
//!
//! A per-file doc-consistency finding: the first declaration wins, and every
//! later one is a `Warning` naming the class. Same-named fields on *different*
//! classes are unrelated and must stay clean.

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

#[test]
fn duplicate_doc_field_flagged() {
    let src = "\
---@class Point
---@field x number
---@field y number
---@field x integer
local Point = {}
";
    assert_eq!(codes(src, Strictness::Warn), vec!["LB0311"]);
}

#[test]
fn distinct_fields_are_clean() {
    let src = "\
---@class Point
---@field x number
---@field y number
local Point = {}
";
    assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
}

#[test]
fn duplicate_doc_field_suppressed_by_directive() {
    let src = "\
---@class Point
---@field x number
---@diagnostic disable-next-line: duplicate-doc-field
---@field x integer
local Point = {}
";
    assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
}

#[test]
fn duplicate_doc_field_not_under_none() {
    let src = "\
---@class Point
---@field x number
---@field x integer
local Point = {}
";
    assert_eq!(codes(src, Strictness::None), Vec::<String>::new());
}
