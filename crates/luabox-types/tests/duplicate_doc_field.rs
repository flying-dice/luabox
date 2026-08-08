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

// --- indexer fields collide too (round 6 review M11) ---------------------
//
// A conflicting `---@field [K] V` used to resolve silently while the
// identical conflict on a named field warned. This PR moved the indexer onto
// the named field's *resolution* rule without moving it onto the
// *diagnostic* rule; these pin the two together.
//
// The oracle agrees: lua-language-server 3.13.5 on the same fixture reports
// `duplicate-doc-field  Duplicate defined fields `[string]`.` — measured
// against the pinned binary, not assumed.

#[test]
fn duplicate_indexer_field_is_flagged_like_a_named_one() {
    let src = "\
---@class Bag
---@field [string] number
---@field [string] boolean
local Bag = {}
";
    assert_eq!(codes(src, Strictness::Warn), vec!["LB0311"]);
}

/// The message names the key the way luals does, so a user grepping either
/// tool's output finds the same thing.
#[test]
fn a_duplicate_indexer_names_its_key_in_the_message() {
    let src = "\
---@class Bag
---@field [string] number
---@field [string] boolean
local Bag = {}
";
    let diags = check(src, Strictness::Warn);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert!(
        diags[0]
            .message
            .contains("duplicate field `[string]` on class `Bag`"),
        "{}",
        diags[0].message
    );
}

/// Identity is the *lowered* key type, not its source text — the same notion
/// of "same key" `TypeEnv`'s own indexer merge uses. Two different key types
/// are two different keys and stay clean.
#[test]
fn indexer_fields_with_different_key_types_are_clean() {
    let src = "\
---@class Bag
---@field [string] number
---@field [integer] number
local Bag = {}
";
    assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
}

/// An indexer and a named field are different kinds and never collide with
/// each other, however the key happens to render.
#[test]
fn an_indexer_and_a_named_field_do_not_collide() {
    let src = "\
---@class Bag
---@field [string] number
---@field string boolean
local Bag = {}
";
    assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
}
