// test code — panics document assumptions
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Duplicate `---@class` declarations for one name **union**, whether they sit
//! in one file or several (#49).
//!
//! Two declarations of a class in code the user wrote are two halves of one
//! intent — luals collects every `doc.class` set for a name and resolves a
//! member against all of them — so the file boundary must not change the
//! answer. Cross-file duplicates already unioned
//! ([`luabox_types::Ambient::with_project_types`]); a same-file duplicate used
//! to *reset* the class, silently dropping every member the earlier
//! declaration had contributed.
//!
//! **Conflict rule: the first declaration of a member wins**, in-file exactly
//! as across files. The cross-file fold has always been first-wins (an
//! already-merged member is never overwritten), and `LB0311`
//! (`duplicate-doc-field`) already says so in its note — "the first
//! declaration wins; remove or rename this one". luals itself unions the two
//! types into `string|number` rather than picking one; luabox keeps the first
//! deterministically and warns, which is the same trade the crate makes for
//! duplicate aliases and enums (#110). Divergence recorded here and in
//! `LIMITATIONS.md`.
//!
//! A file's own declaration still *shadows* an ambient (stdlib / `[types]
//! defs`) class of the same name whole — that is a different axis (the
//! escape hatch, see `Ambient::with_rock_types`) and is unchanged.

use luabox_diag::Diagnostic;
use luabox_syntax::lua::{self, Dialect, parse};
use luabox_types::{
    Ambient, FileTypes, Strictness, build_ambient, check_file_with_ambient, module_surface,
    stdlib_defs,
};

fn check(src: &str) -> Vec<Diagnostic> {
    let parsed = parse(src, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
    check_file_with_ambient(
        &parsed,
        "test.lua",
        Strictness::Strict,
        lua::Dialect::Lua54,
        Some(stdlib_defs(Dialect::Lua54)),
    )
}

fn codes(src: &str) -> Vec<String> {
    check(src).iter().map(|d| d.code.to_string()).collect()
}

fn none() -> Vec<String> {
    Vec::new()
}

/// The workspace surface one project file contributes.
fn surface(src: &str, base: &Ambient) -> FileTypes {
    let parsed = parse(src, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
    module_surface(&parsed, "m.lua", Some(base)).types
}

/// Check `consumer` against the merged surface of every `file`.
fn check_cross(files: &[&str], consumer: &str) -> Vec<String> {
    let base = stdlib_defs(Dialect::Lua54);
    let types: Vec<FileTypes> = files.iter().map(|f| surface(f, base)).collect();
    let ambient = base.with_project_types(types.iter());
    let parsed = parse(consumer, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "consumer must parse cleanly");
    check_file_with_ambient(
        &parsed,
        "consumer.lua",
        Strictness::Strict,
        Dialect::Lua54,
        Some(&ambient),
    )
    .iter()
    .map(|d| d.code.to_string())
    .collect()
}

// --- same-file duplicates union -------------------------------------------

#[test]
fn same_file_duplicate_class_unions_its_fields() {
    // The issue's own reproduction: `host` must survive the second
    // declaration.
    let src = "\
---@class Config
---@field host string

---@class Config
---@field port number

---@param c Config
local function use(c) print(c.host, c.port) end
return use
";
    assert_eq!(codes(src), none());
}

#[test]
fn same_file_duplicate_class_keeps_the_earlier_field_type() {
    // Not merely "the name exists" — the *type* from the first declaration
    // must be the one that arrives.
    let src = "\
---@class Config
---@field host string

---@class Config
---@field port number

---@param n number
local function want(n) end
---@param c Config
local function use(c) want(c.host) end
return use
";
    let diags = check(src);
    assert_eq!(
        diags.iter().map(|d| d.code.to_string()).collect::<Vec<_>>(),
        vec!["LB0300"]
    );
    assert!(
        diags[0].message.contains("found `string`"),
        "the first declaration's field type must survive, got: {}",
        diags[0].message
    );
}

#[test]
fn three_same_file_declarations_all_union() {
    let src = "\
---@class C3
---@field a string
---@class C3
---@field b string
---@class C3
---@field c string
---@param c C3
local function use(c) print(c.a, c.b, c.c) end
return use
";
    assert_eq!(codes(src), none());
}

#[test]
fn a_bare_duplicate_declaration_drops_nothing() {
    // One carrier, one bare re-declaration: re-opening a class to hang more
    // docs off it must not wipe what it already had.
    let src = "\
---@class Bare
---@field a string
local M = {}
function M:go() end

---@class Bare

---@param b Bare
local function use(b) print(b.a) b:go() end
return use
";
    assert_eq!(codes(src), none());
}

#[test]
fn a_duplicate_declaration_can_add_members_via_a_second_carrier() {
    // Both carriers attach to the same class name; both sets of methods must
    // land on it.
    let src = "\
---@class Two
local A = {}
function A:a() end

---@class Two
local B = {}
function B:b() end

---@param t Two
local function use(t) t:a() t:b() end
return use
";
    assert_eq!(codes(src), none());
}

#[test]
fn a_duplicate_declaration_unions_parents() {
    let src = "\
---@class Base1
---@field one string
---@class Base2
---@field two string

---@class Kid : Base1
---@class Kid : Base2

---@param k Kid
local function use(k) print(k.one, k.two) end
return use
";
    assert_eq!(codes(src), none());
}

#[test]
fn a_duplicate_declaration_can_supply_the_type_parameters_the_first_omitted() {
    // Generic parameters follow the same first-wins rule as everything else,
    // with one allowance: a bare first declaration has none to keep, so a
    // later `<T>` is taken rather than dropped.
    let src = "\
---@class Boxed
---@class Boxed<T>
---@field value T

---@param b Boxed<string>
local function use(b) return b.value end
---@param s string
local function want(s) end
---@param b2 Boxed<string>
local function bad(b2) want(b2.value) end
return use, bad
";
    assert_eq!(codes(src), none());
}

#[test]
fn the_first_declarations_type_parameters_are_not_renamed_by_a_duplicate() {
    // A duplicate that renames the parameter does not rewrite the first
    // declaration's body: `value` stays bound to `T`, so a `Boxed<string>`
    // still monomorphises to `string`. Before the merge, the second
    // declaration replaced the template — its parameter list said `U` while
    // the surviving field body still said `T`, which resolved to nothing and
    // reported the first declaration's own annotation as an unknown name.
    let src = "\
---@class Boxed<T>
---@field value T
---@class Boxed<U>

---@param n number
local function want(n) end
---@param b Boxed<string>
local function bad(b) want(b.value) end
return bad
";
    let diags = check(src);
    assert_eq!(
        diags.iter().map(|d| d.code.to_string()).collect::<Vec<_>>(),
        vec!["LB0300"]
    );
    assert!(
        diags[0].message.contains("found `string`"),
        "the first declaration's parameter binding must survive, got: {}",
        diags[0].message
    );
}

#[test]
fn a_bare_duplicate_adds_members_to_a_generic_class() {
    // The generic template is built per declaration too, so a bare
    // re-declaration that only hangs more `---@field`s off the class has to
    // reach it — in either order.
    let src = "\
---@class Pair<T>
---@field first T

---@class Pair
---@field label string

---@param s string
local function want(s) end
---@param p Pair<string>
local function use(p) want(p.first) want(p.label) end
return use
";
    assert_eq!(codes(src), none());
}

#[test]
fn a_generic_declaration_after_a_bare_one_still_collects_both() {
    let src = "\
---@class Pair
---@field label string

---@class Pair<T>
---@field first T

---@param s string
local function want(s) end
---@param p Pair<string>
local function use(p) want(p.first) want(p.label) end
return use
";
    assert_eq!(codes(src), none());
}

// --- the conflict rule, pinned in both directions -------------------------

#[test]
fn cross_file_conflicting_field_keeps_the_first_files_type() {
    // The rule the in-file case must match — measured, not assumed.
    let files = [
        "---@class Cf\n---@field a string\n",
        "---@class Cf\n---@field a number\n",
    ];
    assert_eq!(
        check_cross(
            &files,
            "---@param n number\nlocal function want(n) end\n---@param c Cf\nlocal function use(c) want(c.a) end\n",
        ),
        vec!["LB0300"],
        "the first file's `string` must survive"
    );
    assert_eq!(
        check_cross(
            &files,
            "---@param s string\nlocal function want(s) end\n---@param c Cf\nlocal function use(c) want(c.a) end\n",
        ),
        none()
    );
}

#[test]
fn same_file_conflicting_field_keeps_the_first_declarations_type() {
    // Identical rule in-file. `LB0311` flags the duplicate as it always has.
    let src = "\
---@class Cx
---@field a string
---@class Cx
---@field a number
---@param s string
local function want(s) end
---@param c Cx
local function use(c) want(c.a) end
return use
";
    assert_eq!(codes(src), vec!["LB0311"]);
}

#[test]
fn same_file_conflicting_field_rejects_the_later_declarations_type() {
    let src = "\
---@class Cx
---@field a string
---@class Cx
---@field a number
---@param n number
local function want(n) end
---@param c Cx
local function use(c) want(c.a) end
return use
";
    assert_eq!(codes(src), vec!["LB0311", "LB0300"]);
}

#[test]
fn a_duplicate_field_inside_one_block_follows_the_same_first_wins_rule() {
    // One step away: the same conflict written inside a single `---@class`
    // block. `LB0311`'s note promises first-wins, so the stored type must
    // agree — one rule for the file, not two.
    let src = "\
---@class Cy
---@field a string
---@field a number
---@param s string
local function want(s) end
---@param c Cy
local function use(c) want(c.a) end
return use
";
    assert_eq!(codes(src), vec!["LB0311"]);
}

// --- combined with the cross-file fold ------------------------------------

#[test]
fn same_file_and_cross_file_duplicates_union_together() {
    // Three declarations across two files — every member must arrive.
    let files = [
        "---@class C3\n---@field a string\n---@class C3\n---@field b string\n",
        "---@class C3\n---@field c string\n",
    ];
    assert_eq!(
        check_cross(
            &files,
            "---@param c C3\nlocal function use(c) print(c.a, c.b, c.c) end\n",
        ),
        none()
    );
}

#[test]
fn a_defs_file_duplicate_class_unions_too() {
    // `---@meta` definition files take the same path; they are never inferred,
    // so their duplicates used to reset just as loudly.
    let defs =
        "---@meta\n---@class Dd\n---@field host string\n---@class Dd\n---@field port number\n";
    let ambient = build_ambient(Dialect::Lua54, &[defs.to_string()]);
    let consumer = parse(
        "---@param c Dd\nlocal function use(c) print(c.host, c.port) end\n",
        Dialect::Lua54,
    );
    assert_eq!(consumer.errors(), &[], "consumer must parse cleanly");
    let diags = check_file_with_ambient(
        &consumer,
        "consumer.lua",
        Strictness::Strict,
        Dialect::Lua54,
        Some(&ambient),
    );
    assert_eq!(
        diags.iter().map(|d| d.code.to_string()).collect::<Vec<_>>(),
        none()
    );
}

// --- what must NOT change --------------------------------------------------

#[test]
fn a_file_declaration_still_shadows_an_ambient_class_of_the_same_name() {
    // The escape hatch: a project's own `---@class Shadow` replaces the
    // definition package's, it does not union with it. Unioning here would
    // defeat `[types] defs`.
    let defs = "---@meta\n---@class Shadow\n---@field fromDefs string\n";
    let ambient = build_ambient(Dialect::Lua54, &[defs.to_string()]);
    let src = parse(
        "---@class Shadow\n---@field own string\n---@param s Shadow\nlocal function use(s) print(s.fromDefs) end\nreturn use\n",
        Dialect::Lua54,
    );
    assert_eq!(src.errors(), &[], "fixture must parse cleanly");
    let diags = check_file_with_ambient(
        &src,
        "test.lua",
        Strictness::Strict,
        Dialect::Lua54,
        Some(&ambient),
    );
    assert_eq!(
        diags.iter().map(|d| d.code.to_string()).collect::<Vec<_>>(),
        vec!["LB0306"],
        "the defs field must not leak through the file's own declaration"
    );
}

#[test]
fn distinct_classes_stay_distinct() {
    let src = "\
---@class A1
---@field a string
---@class B1
---@field b string
---@param x A1
local function use(x) print(x.b) end
return use
";
    assert_eq!(codes(src), vec!["LB0306"]);
}
