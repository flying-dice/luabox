// test code — panics document assumptions
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! A bare (unbound) generic parameter must never leak its literal name
//! (`T`) into user-facing diagnostic text — production readiness review
//! finding 5.
//!
//! `---@class Box<T>` referenced without binding its argument (`Box`, or
//! `---@class Sub : Base` naming a generic `Base` without `<...>`) leaves
//! `T` genuinely free: a class-merge precedence matrix fixture (`docs/
//! 03-reference/03-class-merge-precedence.md`'s `field-G`/`indexer-G`
//! `bound-vs-bare` cells) measured both the pre-fix binary and its merge
//! base printing that free parameter's literal spelling straight into
//! `LB0300` text — a type variable the consumer can neither name nor
//! produce, in a message that therefore points at no action.
//!
//! The module-export seam (`crate::infer::Infer::reify_export` /
//! `TypeEnv::class_shape_bound_export`, #56) already solved the identical
//! problem where a generic carrier crosses `require`: an unbound parameter
//! reads as `unknown`, not its name. This file pins the same rule reused at
//! every same-file reference-consuming site a diagnostic can name a
//! member's type in: a field read, an `ipairs`/indexer read, a `: Parent`
//! conformance obligation, and a table-literal's expected shape — each with
//! a **bound** control proving the fix does not touch a reference that
//! genuinely supplies its argument.

use luabox_diag::Diagnostic;
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::{Strictness, check_file};

fn diags(source: &str) -> Vec<Diagnostic> {
    let parsed = parse(source, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
    check_file(&parsed, "test.lua", Strictness::Strict, Dialect::Lua54)
}

fn messages(source: &str) -> Vec<String> {
    diags(source).into_iter().map(|d| d.message).collect()
}

/// Control: `Base<number>` (bound) still resolves the real argument.
/// Treatment: a bare `Base` reference — no `<...>` anywhere in its own
/// declaration or an ancestor's — reads its inherited member as `unknown`,
/// never as the parameter's own literal name.
#[test]
fn field_read_on_bare_generic_ancestor_erases_to_unknown() {
    let bound = "\
---@class Base<T>
---@field item T
local Base = {}

---@class Sub : Base<number>

---@param s Sub
local function use_sub(s)
  ---@param x string
  local function want_string(x) end
  want_string(s.item)
end
";
    let msgs = messages(bound);
    assert_eq!(msgs.len(), 1, "{msgs:?}");
    assert!(
        msgs[0].contains("found `number`"),
        "a bound parent argument must still substitute: {}",
        msgs[0]
    );

    let bare = "\
---@class Base<T>
---@field item T
local Base = {}

---@class Sub : Base

---@param s Sub
local function use_sub(s)
  ---@param x string
  local function want_string(x) end
  want_string(s.item)
end
";
    let msgs = messages(bare);
    assert_eq!(msgs.len(), 1, "{msgs:?}");
    assert!(
        !msgs[0].contains('T'),
        "a bare parent reference must not leak the free parameter's own \
         literal name into diagnostic text: {}",
        msgs[0]
    );
    assert!(
        msgs[0].contains("found `unknown`"),
        "an unbound parameter reads as `unknown`, matching the `require` \
         boundary's own rule (#56): {}",
        msgs[0]
    );
}

/// The `---@class Sub : Interface` conformance obligation (#107): `Sub`
/// really is missing `item` here (its carrier declares nothing), so the
/// obligation must still fire — an unbound parameter must not *also* make
/// the checker treat the member as optional and silently drop the
/// obligation (measured regression from this fix's first attempt: erasing
/// `class_shape_bound`'s result directly made `Ty::Unknown::admits_nil()`
/// return `true`, which reads as "this member need not be present" and
/// deleted the whole diagnostic). The fix keeps obligation-gating on the
/// raw, non-erasing resolution and only erases the *displayed* type.
#[test]
fn conformance_missing_member_on_bare_generic_ancestor_still_fires_and_erases_to_unknown() {
    let src = "\
---@class Base<T>
---@field item T
local Base = {}

---@class Sub : Base
local Sub = {}
";
    let msgs = messages(src);
    assert_eq!(
        msgs,
        vec!["`Sub` does not satisfy `Base`: missing member `item`".to_string()],
        "the obligation must still be detected — erasing the parameter's \
         display text must never erase the obligation itself"
    );
    let labels: Vec<String> = diags(src)
        .into_iter()
        .flat_map(|d| d.labels)
        .map(|l| l.message)
        .collect();
    assert!(
        labels
            .iter()
            .any(|l| l == "expected member `item` of type `unknown`"),
        "{labels:?}"
    );
}

/// The table-literal obligation (LuaLS `missing-fields` parity): same
/// regression risk as the conformance case above, pinned separately because
/// `check::Checker::table_shape`/`field_shape` is a second, independent
/// split of the same raw-vs-display resolution.
#[test]
fn table_literal_missing_field_on_bare_generic_ancestor_still_fires_and_erases_to_unknown() {
    let src = "\
---@class Base<T>
---@field item T
local Base = {}

---@class Sub : Base

---@param s Sub
local function want_sub(s) end
want_sub({})
";
    let msgs = messages(src);
    assert!(
        msgs.iter()
            .any(|m| m == "missing required field `item` in table literal"),
        "the table-literal obligation must still be detected: {msgs:?}"
    );
    let labels: Vec<String> = diags(src)
        .into_iter()
        .flat_map(|d| d.labels)
        .map(|l| l.message)
        .collect();
    assert!(
        labels
            .iter()
            .any(|l| l == "expected field `item` of type `unknown`"),
        "{labels:?}"
    );
}
