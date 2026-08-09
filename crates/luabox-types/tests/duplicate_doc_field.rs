//! `LB0311`: the same `---@field` declared twice on one class (#113) —
//! luals' `duplicate-doc-field`.
//!
//! A per-file doc-consistency finding: the first declaration wins, and every
//! later one is a `Warning` naming the class. Same-named fields on *different*
//! classes are unrelated and must stay clean.

use luabox_diag::Diagnostic;
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::{
    Strictness, build_ambient, check_file, check_file_with_ambient, module_surface,
};

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

/// `LB0311` codes only, from a check run with `defs` declaring types the
/// checked file names but does not declare itself. The other codes a fixture
/// may raise (`LB0305` for a name nothing declares) are not what these
/// assertions are about.
fn dup_codes_with_defs(source: &str, defs: &[&str]) -> Vec<String> {
    let extra: Vec<String> = defs.iter().map(|s| (*s).to_string()).collect();
    let ambient = build_ambient(Dialect::Lua54, &extra);
    let parse = parse(source, Dialect::Lua54);
    assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
    check_file_with_ambient(
        &parse,
        "test.lua",
        Strictness::Warn,
        Dialect::Lua54,
        Some(&ambient),
    )
    .iter()
    .map(|d| d.code.to_string())
    .filter(|c| c == "LB0311")
    .collect()
}

/// `LB0311` codes only, from a check run with another *project* file's
/// declarations merged in the way `luabox check` merges them
/// (`Ambient::with_project_types`) — the cross-file sibling of
/// [`dup_codes_with_defs`]'s definition-package layer.
fn dup_codes_with_project_file(source: &str, sibling: &str) -> Vec<String> {
    let base = build_ambient(Dialect::Lua54, &[]);
    let sibling_parse = parse(sibling, Dialect::Lua54);
    assert_eq!(sibling_parse.errors(), &[], "fixture must parse cleanly");
    let surface = module_surface(&sibling_parse, "sibling.lua", Some(&base));
    let ambient = base.with_project_types([&surface.types]);
    let parse = parse(source, Dialect::Lua54);
    assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
    check_file_with_ambient(
        &parse,
        "test.lua",
        Strictness::Warn,
        Dialect::Lua54,
        Some(&ambient),
    )
    .iter()
    .map(|d| d.code.to_string())
    .filter(|c| c == "LB0311")
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

// --- indexer key identity survives an unresolvable key --------------------
//
// Identity is the lowered key type, but a *file-local* lowering context
// resolves only what this file declares: every key naming a type declared in
// a `[types] defs` package or another project file lowered to the one
// `unknown` type, so two unrelated keys read as one and `LB0311` fired on a
// class with no duplicate at all. The residue of unresolved names is now part
// of the key, so two keys collide only when both resolve to the same type, or
// both fail to resolve on the same names.

/// Two distinct classes declared by a definition package, used as two
/// indexer keys. Nothing is duplicated; nothing may be reported.
#[test]
fn defs_declared_indexer_keys_of_different_classes_do_not_collide() {
    let defs = "\
---@meta
---@class KA
---@field a number

---@class KB
---@field b number
";
    let src = "\
---@class Keyed
---@field [KA] number
---@field [KB] string
local K = {}
return K
";
    assert_eq!(dup_codes_with_defs(src, &[defs]), Vec::<String>::new());
}

/// The same shape with the two key classes declared by another *project*
/// file rather than a definition package — the workspace-global path.
#[test]
fn project_declared_indexer_keys_of_different_classes_do_not_collide() {
    let sibling = "\
---@class KA
---@field a number

---@class KB
---@field b number
return {}
";
    let src = "\
---@class Keyed
---@field [KA] number
---@field [KB] string
local K = {}
return K
";
    assert_eq!(
        dup_codes_with_project_file(src, sibling),
        Vec::<String>::new()
    );
}

/// The same unresolvable *name* used twice is still one key: the residue
/// matches, so the duplicate is still caught.
#[test]
fn the_same_defs_declared_indexer_key_twice_is_still_a_duplicate() {
    let defs = "\
---@meta
---@class KA
---@field a number
";
    let src = "\
---@class Keyed
---@field [KA] number
---@field [KA] string
local K = {}
return K
";
    assert_eq!(dup_codes_with_defs(src, &[defs]), vec!["LB0311"]);
}

// --- a class's own type parameters are distinct keys ----------------------

/// `---@class Keyed<T, U>`'s two parameters are two different key types. They
/// only read as one when the class's own parameters are missing from the
/// lowering context — then both are undeclared names collapsing to `unknown`.
#[test]
fn class_type_parameters_are_distinct_indexer_keys() {
    let src = "\
---@class Keyed<T, U>
---@field [T] number
---@field [U] string
local K = {}
return K
";
    assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
}

/// The same parameter twice is a genuine duplicate and stays reported.
#[test]
fn the_same_type_parameter_twice_is_a_duplicate_indexer_key() {
    let src = "\
---@class Keyed<T>
---@field [T] number
---@field [T] string
local K = {}
return K
";
    assert_eq!(codes(src, Strictness::Warn), vec!["LB0311"]);
}

/// A `---@generic` on the same doc block is in scope for the key too.
#[test]
fn generic_tag_parameters_are_distinct_indexer_keys() {
    let src = "\
---@generic T, U
---@class Keyed
---@field [T] number
---@field [U] string
local K = {}
return K
";
    assert_eq!(codes(src, Strictness::Warn), Vec::<String>::new());
}

// --- an alias key collides with what it expands to ------------------------

/// A file-local `---@alias` resolving to `string` is the same key as
/// `[string]` — the identity `TypeEnv::collect_class`'s own indexer merge
/// keys on.
#[test]
fn a_file_local_alias_key_collides_with_its_expansion() {
    let src = "\
---@alias MyAlias string
---@class Keyed
---@field [MyAlias] number
---@field [string] string
local K = {}
return K
";
    assert_eq!(codes(src, Strictness::Warn), vec!["LB0311"]);
}
