// test code — panics document assumptions
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! A `---@class` carried by a **global** collects its members exactly like one
//! carried by a `local` (#50).
//!
//! ```lua
//! ---@class Global
//! Glob = {}
//! function Glob:size() return 1 end
//! ```
//!
//! luals makes no distinction between the two carrier spellings — a
//! `function Glob:size()` is a member of whatever class `Glob` carries — but
//! luabox's inference keyed its carrier maps on `BindingId`, which a free
//! global name does not have, so the tag half of a global carrier worked and
//! the member half silently did not. (Wave 19 hit the same shape in
//! `luabox-lint` and answered it with `ValueRef::Global(String)`; the types
//! crate now keys the same way.)
//!
//! The precedence rules waves 14/16 measured are unchanged, and globals slot
//! into them coherently:
//!
//! * **lexical beats nominal** — a global carrier is lexical in the same sense
//!   a local one is, so `---@class Wrapper` over `Glob = {}` makes
//!   `function Glob:m()` a member of `Wrapper`, not of a class named `Glob`;
//! * **last carrier wins a repeated variable** — Lua resolves `Glob` to its
//!   most recent assignment, so a re-carried global follows the same
//!   last-wins rule a re-declared `local` does.

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

fn surface(src: &str, base: &Ambient) -> FileTypes {
    let parsed = parse(src, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
    module_surface(&parsed, "m.lua", Some(base)).types
}

/// Check `consumer` against the workspace surface `provider` contributes.
fn check_cross(provider: &str, consumer: &str) -> Vec<String> {
    let base = stdlib_defs(Dialect::Lua54);
    let types = surface(provider, base);
    let ambient = base.with_project_types([&types]);
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

// --- the issue's reproduction, and its five shapes ------------------------

#[test]
fn a_global_carrier_collects_its_methods() {
    let src = "\
---@class Global
Glob = {}
function Glob:size() return 1 end
---@param g Global
local function use(g) return g:size() end
return use
";
    assert_eq!(codes(src), none());
}

#[test]
fn a_global_carrier_collects_its_dotted_functions() {
    let src = "\
---@class Global
Glob = {}
function Glob.make() return 1 end
---@param g Global
local function use(g) return g.make() end
return use
";
    assert_eq!(codes(src), none());
}

#[test]
fn a_global_carrier_collects_its_value_fields() {
    let src = "\
---@class Global
Glob = {}
Glob.version = \"1.0\"
---@param g Global
local function use(g) return g.version end
return use
";
    assert_eq!(codes(src), none());
}

#[test]
fn a_global_carrier_collects_an_assigned_function_member() {
    let src = "\
---@class Global
Glob = {}
Glob.run = function() return 1 end
---@param g Global
local function use(g) return g.run() end
return use
";
    assert_eq!(codes(src), none());
}

#[test]
fn a_global_carriers_member_signature_is_checked_at_the_call_site() {
    // Not merely "the member exists": its declared signature has to arrive too.
    let src = "\
---@class Global
Glob = {}
---@param n integer
function Glob:size(n) return n end
---@param g Global
local function use(g) return g:size(\"nope\") end
return use
";
    assert_eq!(codes(src), vec!["LB0300"]);
}

#[test]
fn an_absent_member_of_a_global_carrier_still_reports() {
    // The fix must not turn the class into an open bag.
    let src = "\
---@class Global
Glob = {}
function Glob:size() return 1 end
---@param g Global
local function use(g) return g:missing() end
return use
";
    assert_eq!(codes(src), vec!["LB0306"]);
}

#[test]
fn the_local_carrier_baseline_is_unchanged() {
    let src = "\
---@class Localc
local L = {}
function L:size() return 1 end
---@param g Localc
local function use(g) return g:size() end
return use
";
    assert_eq!(codes(src), none());
}

// --- the workspace surface ------------------------------------------------

#[test]
fn a_global_carriers_members_reach_another_file() {
    // `FileTypes::collect` folds the carrier shape into the exported class, so
    // the members have to be on it by the time the file's surface is taken.
    assert_eq!(
        check_cross(
            "---@class Gx\nGlob = {}\nfunction Glob:size() return 1 end\n",
            "---@param g Gx\nlocal function use(g) return g:size() end\nreturn use\n",
        ),
        none()
    );
}

#[test]
fn a_global_carrier_in_a_defs_file_still_works() {
    // The `---@meta` path harvests attachments syntactically and already
    // accepted a global carrier; pinned so the inference-side fix cannot
    // regress it.
    let defs = "---@meta\n---@class Gd\nGlob = {}\nfunction Glob:size() return 1 end\n";
    let ambient = build_ambient(Dialect::Lua54, &[defs.to_string()]);
    let consumer = parse(
        "---@param g Gd\nlocal function use(g) return g:size() end\nreturn use\n",
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

// --- precedence: lexical beats nominal, both orders -----------------------

#[test]
fn a_global_carrier_is_lexical_so_it_beats_a_nominal_match() {
    // `Glob` names the *binding* the class is carried by. `function Glob:m()`
    // is a member of `Wrapper`, and the class literally named `Glob` — declared
    // on some other carrier — must not collect it.
    let src = "\
---@class Wrapper
Glob = {}
function Glob:m() end

---@class Glob
local other = {}

---@param w Wrapper
local function useW(w) w:m() end
---@param g Glob
local function useG(g) g:m() end
return useW, useG
";
    assert_eq!(codes(src), vec!["LB0306"]);
}

#[test]
fn the_lexical_global_carrier_wins_in_the_other_declaration_order() {
    // Same shape, blocks swapped: the verdict must not flip.
    let src = "\
---@class Glob
local other = {}

---@class Wrapper
Glob = {}
function Glob:m() end

---@param w Wrapper
local function useW(w) w:m() end
---@param g Glob
local function useG(g) g:m() end
return useW, useG
";
    assert_eq!(codes(src), vec!["LB0306"]);
}

// --- precedence: last carrier wins a repeated variable --------------------

#[test]
fn a_repeated_global_carrier_attaches_to_the_last_declaration() {
    let src = "\
---@class First
Glob = {}
---@class Second
Glob = {}
function Glob:m() end

---@param s Second
local function useS(s) s:m() end
return useS
";
    assert_eq!(codes(src), none());
}

#[test]
fn the_earlier_global_carrier_does_not_collect_the_member() {
    let src = "\
---@class First
Glob = {}
---@class Second
Glob = {}
function Glob:m() end

---@param f First
local function useF(f) f:m() end
return useF
";
    assert_eq!(codes(src), vec!["LB0306"]);
}

#[test]
fn the_repeated_carrier_rule_holds_with_the_declarations_swapped() {
    let src = "\
---@class Second
Glob = {}
---@class First
Glob = {}
function Glob:m() end

---@param f First
local function useF(f) f:m() end
return useF
";
    assert_eq!(codes(src), none());
}

// --- mixed local / global carriers of the same name -----------------------

#[test]
fn a_local_carrier_shadowed_by_a_later_global_style_reassignment() {
    // `Glob = {}` *after* `local Glob` assigns the local, so the second
    // `---@class` re-carries that binding and the method follows it.
    let src = "\
---@class Outer
local Glob = {}
---@class Inner
Glob = {}
function Glob:m() end

---@param i Inner
local function useI(i) i:m() end
return useI
";
    assert_eq!(codes(src), none());
}

#[test]
fn a_global_carrier_shadowed_by_a_later_local_of_the_same_name() {
    // The global carries `g`, the later `local` carries `l`; neither steals
    // the other's member.
    let src = "\
---@class GlobalSide
Glob = {}
function Glob:g() end

---@class LocalSide
local Glob = {}
function Glob:l() end

---@param a GlobalSide
local function useA(a) a:g() end
---@param b LocalSide
local function useB(b) b:l() end
return useA, useB
";
    assert_eq!(codes(src), none());
}

#[test]
fn the_local_side_does_not_collect_the_global_sides_member() {
    let src = "\
---@class GlobalSide
Glob = {}
function Glob:g() end

---@class LocalSide
local Glob = {}
function Glob:l() end

---@param b LocalSide
local function useB(b) b:g() end
return useB
";
    assert_eq!(codes(src), vec!["LB0306"]);
}

// --- shapes that must stay unwired ---------------------------------------

#[test]
fn a_class_over_a_field_assignment_is_not_a_carrier() {
    // `---@class` over `M.Sub = {}` names no variable; it declares the class
    // and nothing carries it. Conservative — no false members, no panic.
    let src = "\
local M = {}
---@class Sub
M.Sub = {}
function M.Sub.go() end

---@param s Sub
local function use(s) s.go() end
return M, use
";
    assert_eq!(codes(src), vec!["LB0306"]);
}
