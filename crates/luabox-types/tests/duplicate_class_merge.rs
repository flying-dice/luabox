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

use luabox_syntax::lua::{Dialect, parse};
use luabox_types::{Strictness, build_ambient, check_file_with_ambient};

// F79 (round 3 review): `check`/`codes`/`surface`/`check_cross` used to be
// ~40 lines defined here near-verbatim identically to
// `duplicate_class_merge_property.rs` — now one shared module both import.
mod support;
use support::{check, check_cross, codes};

fn none() -> Vec<String> {
    Vec::new()
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
fn cross_file_conflicting_parent_argument_keeps_the_first_files_binding() {
    // F40 (round 3 review): `ParentRef { name, args }` makes "same parent,
    // different arguments" representable — before this PR `parents` was
    // `Vec<String>`, so the state could not even exist. Both merge seams
    // dedup a parent by NAME alone (`env.rs`'s `merge_file_types` and
    // `absorb_block`), so `---@class Sub : Base<number>` in one file and
    // `---@class Sub : Base<string>` in another is not diagnosed either way
    // — it resolves silently by fold order, the same rule
    // `cross_file_conflicting_field_keeps_the_first_files_type` pins for a
    // `---@field` conflict (that one *is* diagnosed, `LB0311`; this one is
    // not — noted as a residual, not fixed here). Pinned in both directions
    // so a future change to the fold order — or a diagnostic added later —
    // is a deliberate edit to this test, not a silent behaviour change.
    let base = "---@class Base<U>\n---@field item U\n";
    let files = [
        format!("{base}---@class Sub : Base<number>\n"),
        format!("{base}---@class Sub : Base<string>\n"),
    ];
    let file_refs: Vec<&str> = files.iter().map(String::as_str).collect();
    assert_eq!(
        check_cross(
            &file_refs,
            "---@param n number\nlocal function want(n) end\n---@param s Sub\nlocal function use(s) want(s.item) end\n",
        ),
        none(),
        "the first file's `Base<number>` binding must survive: item is `number`"
    );
    assert_eq!(
        check_cross(
            &file_refs,
            "---@param str string\nlocal function want(str) end\n---@param s Sub\nlocal function use(s) want(s.item) end\n",
        ),
        vec!["LB0300"],
        "the second file's `Base<string>` binding must NOT win — item is not `string`"
    );

    // Reversing which file is listed first flips the answer: this is
    // fold-order-dependent, not a stable per-name rule, which is the shape
    // of the residual (nothing compares the two bindings or reports a
    // conflict).
    let reversed: Vec<&str> = file_refs.iter().rev().copied().collect();
    assert_eq!(
        check_cross(
            &reversed,
            "---@param str string\nlocal function want(str) end\n---@param s Sub\nlocal function use(s) want(s.item) end\n",
        ),
        none(),
        "with the files reversed, `Base<string>` is now first and wins: item is `string`"
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

// ---------------------------------------------------------------------------
// Per-declaration type-parameter scoping (#49 follow-up).
//
// Each declaration's `---@field` bodies are lowered against *its own* `<...>`
// parameter list; the templates are then unified **positionally** when they
// merge, so declaration 2's slot-0 parameter becomes declaration 1's slot-0
// name in the merged template. Before this, the first non-empty parameter
// list was handed to every declaration of the name, so a duplicate that
// renamed the parameter had its own field bodies resolved against names that
// were not in scope for it (LB0305 on the user's own, valid annotation).
// ---------------------------------------------------------------------------

#[test]
fn a_duplicate_that_renames_the_parameter_lowers_its_own_fields() {
    // Both declarations carry field bodies under *different* parameter names.
    // luals accepts this: `U` is in scope for the second declaration because
    // the second declaration is the one that declares it.
    let src = "\
---@class Boxed<T>
---@field value T

---@class Boxed<U>
---@field other U

---@param b Boxed<string>
local function use(b) return b.value end
return use
";
    assert_eq!(codes(src), none());
}

#[test]
fn a_bare_first_declaration_does_not_capture_a_later_declarations_parameter() {
    // The canonical parameter list comes from the first *non-empty*
    // declaration (`<U>`), but the second declaration's field body is written
    // against `<T>` — its own list — and must lower against that.
    let src = "\
---@class Boxed<U>

---@class Boxed<T>
---@field value T
return 1
";
    assert_eq!(codes(src), none());
}

#[test]
fn a_renamed_duplicates_field_monomorphises_through_the_canonical_parameter() {
    // The functional half: both declarations' fields have to *substitute*,
    // not merely lower without complaint. `Boxed<string>` makes `value` and
    // `other` both `string`, whichever declaration contributed them.
    let src = "\
---@class Boxed<T>
---@field value T

---@class Boxed<U>
---@field other U

---@param s string
local function want(s) end
---@param b Boxed<string>
local function use(b) want(b.value) want(b.other) end
return use
";
    assert_eq!(codes(src), none());
}

#[test]
fn a_renamed_duplicates_field_still_reports_a_mismatch_after_substitution() {
    // The negative direction of the previous test: `other` is the second
    // declaration's `U`, mapped to canonical slot 0, so `Boxed<string>` types
    // it `string` — passing it where a `number` is wanted is LB0300.
    let src = "\
---@class Boxed<T>
---@field value T

---@class Boxed<U>
---@field other U

---@param n number
local function want(n) end
---@param b Boxed<string>
local function bad(b) want(b.other) end
return bad
";
    let diags = check(src);
    assert_eq!(
        diags.iter().map(|d| d.code.to_string()).collect::<Vec<_>>(),
        vec!["LB0300"]
    );
    assert!(
        diags[0].message.contains("found `string`"),
        "the renamed parameter must monomorphise to the type argument, got: {}",
        diags[0].message
    );
}

#[test]
fn a_lone_declaration_with_any_parameter_name_is_clean() {
    // Control: nothing about the merge is involved, so `<U>` alone works.
    let src = "\
---@class Boxed<U>
---@field other U

---@param s string
local function want(s) end
---@param b Boxed<string>
local function use(b) want(b.other) end
return use
";
    assert_eq!(codes(src), none());
}

#[test]
fn two_declarations_sharing_a_parameter_name_stay_clean() {
    // Control: the shape that already worked keeps working.
    let src = "\
---@class Boxed<T>
---@field value T

---@class Boxed<T>
---@field other T

---@param s string
local function want(s) end
---@param b Boxed<string>
local function use(b) want(b.value) want(b.other) end
return use
";
    assert_eq!(codes(src), none());
}

#[test]
fn three_declarations_each_rename_the_parameter_independently() {
    // Neighborhood: the positional unification is not a two-declaration
    // special case.
    let src = "\
---@class Tri<A>
---@field a A

---@class Tri<B>
---@field b B

---@class Tri<C>
---@field c C

---@param s string
local function want(s) end
---@param t Tri<string>
local function use(t) want(t.a) want(t.b) want(t.c) end
return use
";
    assert_eq!(codes(src), none());
}

#[test]
fn a_bare_declaration_between_two_renamed_ones_contributes_nothing_and_breaks_nothing() {
    let src = "\
---@class Mid<T>
---@field first T

---@class Mid

---@class Mid<U>
---@field second U

---@param s string
local function want(s) end
---@param m Mid<string>
local function use(m) want(m.first) want(m.second) end
return use
";
    assert_eq!(codes(src), none());
}

#[test]
fn a_duplicate_declaring_more_parameters_keeps_the_canonical_arity() {
    // Parameter counts follow the same first-wins rule as the names: the
    // canonical list is `<T>`, so `Mixed<string>` is arity-correct and the
    // second declaration's slot-0 `A` maps onto `T`. Its *surplus* `B` has no
    // canonical slot to map to and stays lenient (`unknown`), the same
    // leniency a bare generic reference gets.
    let src = "\
---@class Mixed<T>
---@field first T

---@class Mixed<A, B>
---@field second A
---@field spare B

---@param s string
local function want(s) end
---@param m Mixed<string>
local function use(m) want(m.first) want(m.second) end
return use
";
    assert_eq!(codes(src), none());
}

#[test]
fn a_duplicate_declaring_fewer_parameters_lowers_against_its_own_list() {
    let src = "\
---@class Fewer<K, V>
---@field key K
---@field value V

---@class Fewer<X>
---@field extra X

---@param s string
local function want(s) end
---@param n number
local function wantn(n) end
---@param f Fewer<string, number>
local function use(f) want(f.key) wantn(f.value) want(f.extra) end
return use
";
    assert_eq!(codes(src), none());
}

#[test]
fn a_declaration_referring_to_another_declarations_parameter_is_still_unknown() {
    // The scoping cuts both ways: `T` belongs to the first declaration only,
    // so the second declaration naming it is a genuine LB0305 — exactly what
    // luals reports.
    let src = "\
---@class Scoped<T>
---@field first T

---@class Scoped<U>
---@field second T
return 1
";
    assert_eq!(codes(src), vec!["LB0305"]);
}
