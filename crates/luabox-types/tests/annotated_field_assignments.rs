// test code — panics document assumptions
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! `---@type` over an *assignment* statement whose target is not a plain
//! `local` — a table field, an index, or a global (#48).
//!
//! Before this, the annotation was consumed only for `local` statements: a
//! `---@type string` above `M.a = 1` neither declared `M.a` nor diagnosed the
//! initializer, so it looked accepted and did nothing. luals makes no such
//! distinction — a doc block binds to the assignment it precedes whatever the
//! target's syntactic shape — so the annotation is now the declared type of
//! the assigned slot and the initializer is checked against it (`LB0300`),
//! exactly as on a `local`.
//!
//! **Positional, as on a `local`.** `---@type A, B` over `a, b = x, y` gives
//! each target the type in its own slot, so a lone `---@type T` over a
//! multi-target assignment declares the *first* target only. This is the rule
//! `---@type fun(…)` on an assignment has followed since #38; extending it to
//! every type keeps one rule for the statement rather than two.

use luabox_diag::Diagnostic;
use luabox_syntax::lua;
use luabox_types::{Strictness, check_file_with_ambient, stdlib_defs};

fn check(src: &str) -> Vec<Diagnostic> {
    let parse = lua::parse(src, lua::Dialect::Lua54);
    assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
    check_file_with_ambient(
        &parse,
        "test.lua",
        Strictness::Strict,
        lua::Dialect::Lua54,
        Some(stdlib_defs(lua::Dialect::Lua54)),
    )
}

fn codes(src: &str) -> Vec<String> {
    check(src).iter().map(|d| d.code.to_string()).collect()
}

fn none() -> Vec<String> {
    Vec::new()
}

// --- `---@type fun(…)` attaches the signature to the assigned name --------
//
// The other half of the statement (#38, #58 mutation audit): beyond checking
// the initializer, a `---@type fun(…)` over `NAME = function` registers the
// signature under the TARGET'S name so calls to it are argument-checked.
// The declaration half above cannot fail if the attachment silently no-ops,
// mis-names the target, or drops one target shape — only call-site probes
// can, one per target arm (dotted field, plain global).

#[test]
fn typed_function_field_assignment_checks_calls_by_the_dotted_name() {
    let src = "\
local M = {}
---@type fun(n: number): string
M.f = function(n) return \"s\" end
local x = M.f(\"not a number\")
return M
";
    assert_eq!(codes(src), vec!["LB0300"]);
    let clean = "\
local M = {}
---@type fun(n: number): string
M.f = function(n) return \"s\" end
local x = M.f(1)
return M
";
    assert_eq!(codes(clean), none());
}

#[test]
fn typed_function_global_assignment_checks_calls_by_the_plain_name() {
    let src = "\
---@type fun(n: number): string
handler = function(n) return \"s\" end
local x = handler(\"not a number\")
";
    assert_eq!(codes(src), vec!["LB0300"]);
}

// --- the annotation is enforced on a field assignment ---------------------

#[test]
fn typed_field_assignment_mismatch_reports() {
    // The issue's own reproduction: the annotation must not be silently
    // dropped.
    let src = "\
local M = {}
---@type string
M.a = 1
return M
";
    assert_eq!(codes(src), vec!["LB0300"]);
}

#[test]
fn typed_field_assignment_match_is_clean() {
    let src = "\
local M = {}
---@type string
M.a = \"ok\"
return M
";
    assert_eq!(codes(src), none());
}

#[test]
fn typed_field_assignment_mismatch_the_other_direction() {
    // Both directions of the mismatch, not just "number where string wanted".
    let src = "\
local M = {}
---@type integer
M.a = \"nope\"
return M
";
    assert_eq!(codes(src), vec!["LB0300"]);
}

#[test]
fn typed_field_assignment_error_points_at_the_initializer() {
    let src = "\
local M = {}
---@type string
M.a = 1
return M
";
    let diags = check(src);
    let label = diags[0].primary_label().expect("primary label");
    assert_eq!(
        src.get(label.span.range.clone()),
        Some("1"),
        "the error must sit on the initializer, not the annotation"
    );
}

#[test]
fn typed_field_assignment_declares_the_field_for_reads() {
    // The annotation is the *declared* type of the slot, so a later read of
    // `M.a` is the declared `number` — not the initializer's narrower
    // inferred `1`/`integer`. The message is what proves which one arrived.
    let src = "\
local M = {}
---@type number
M.a = 1
---@param s string
local function want(s) end
want(M.a)
return M
";
    let diags = check(src);
    assert_eq!(
        diags.iter().map(|d| d.code.to_string()).collect::<Vec<_>>(),
        vec!["LB0300"]
    );
    assert!(
        diags[0].message.contains("found `number`"),
        "the declared type must reach the read, got: {}",
        diags[0].message
    );
}

// --- the one-step-away neighbourhood --------------------------------------

#[test]
fn typed_global_assignment_mismatch_reports() {
    let src = "\
---@type string
G = 1
";
    assert_eq!(codes(src), vec!["LB0300"]);
}

#[test]
fn typed_global_assignment_match_is_clean() {
    let src = "\
---@type string
G = \"ok\"
";
    assert_eq!(codes(src), none());
}

#[test]
fn typed_index_assignment_mismatch_reports() {
    // `M["a"] = 1` is `M.a = 1` written the other way; the annotation reaches
    // both.
    let src = "\
local M = {}
---@type string
M[\"a\"] = 1
return M
";
    assert_eq!(codes(src), vec!["LB0300"]);
}

#[test]
fn typed_index_assignment_match_is_clean() {
    let src = "\
local M = {}
---@type string
M[\"a\"] = \"ok\"
return M
";
    assert_eq!(codes(src), none());
}

#[test]
fn typed_nested_field_assignment_applies_to_the_innermost_field() {
    // `M.a.b = 1` — the annotation declares `M.a.b`, the slot actually being
    // assigned, not `M.a`. luals binds the doc to the assignment, and the
    // assignment's target is the innermost field.
    let src = "\
local M = {}
M.a = {}
---@type string
M.a.b = 1
return M
";
    assert_eq!(codes(src), vec!["LB0300"]);
}

#[test]
fn typed_nested_field_assignment_match_is_clean() {
    let src = "\
local M = {}
M.a = {}
---@type string
M.a.b = \"ok\"
return M
";
    assert_eq!(codes(src), none());
}

#[test]
fn lone_annotation_over_a_multi_target_assignment_declares_the_first_only() {
    // Positional: slot 0 is annotated `string` and mismatches; slot 1 has no
    // annotation and is left alone. Exactly one diagnostic.
    let src = "\
local M = {}
---@type string
M.a, M.b = 1, 2
return M
";
    assert_eq!(codes(src), vec!["LB0300"]);
}

#[test]
fn positional_annotations_over_a_multi_target_assignment_check_each_slot() {
    let src = "\
local M = {}
---@type string, integer
M.a, M.b = 1, \"nope\"
return M
";
    assert_eq!(codes(src), vec!["LB0300", "LB0300"]);
}

#[test]
fn positional_annotations_over_a_multi_target_assignment_accept_matches() {
    let src = "\
local M = {}
---@type string, integer
M.a, M.b = \"ok\", 1
return M
";
    assert_eq!(codes(src), none());
}

// --- interaction with a `---@field` declaration ---------------------------

#[test]
fn annotation_is_enforced_even_when_a_field_is_declared() {
    // The `---@field` declares the class *surface*; the `---@type` declares
    // this assignment's slot. The initializer is checked against the
    // annotation directly above it — the annotation is never a no-op.
    let src = "\
---@class Cfg
---@field a number
local M = {}
---@type string
M.a = 1
return M
";
    assert_eq!(codes(src), vec!["LB0300"]);
}

#[test]
fn a_declared_field_still_governs_the_class_surface() {
    // luals parity, and the rule already in force for carrier attachments
    // (#33): a `---@field` declaration wins on the class surface over
    // whatever the carrier statement contributes. So an *instance* of the
    // class reads `a` as the declared `number`, not as the assignment's
    // `---@type string`.
    let src = "\
---@class Cfg
---@field a number
local M = {}
---@type string
M.a = \"ok\"
---@param s string
local function want(s) end
---@param c Cfg
local function use(c) want(c.a) end
return M
";
    let diags = check(src);
    assert_eq!(
        diags.iter().map(|d| d.code.to_string()).collect::<Vec<_>>(),
        vec!["LB0300"]
    );
    assert!(
        diags[0].message.contains("found `number`"),
        "the `---@field` must win on the class surface, got: {}",
        diags[0].message
    );
}

// --- shapes the annotation must keep ignoring -----------------------------

#[test]
fn an_unannotated_field_assignment_stays_clean() {
    let src = "\
local M = {}
M.a = 1
return M
";
    assert_eq!(codes(src), none());
}

#[test]
fn the_local_baseline_is_unchanged() {
    let src = "\
---@type string
local a = 1
";
    assert_eq!(codes(src), vec!["LB0300"]);
}

#[test]
fn a_typed_function_literal_assignment_still_takes_its_signature() {
    // #38's path must survive: the `fun(…)` annotation still supplies the
    // signature to the dotted callable, so the *call* is what mismatches —
    // one diagnostic, at the call site, not at the literal.
    let src = "\
local M = {}
---@type fun(n: integer)
M.f = function(n) end
M.f(\"nope\")
return M
";
    assert_eq!(codes(src), vec!["LB0300"]);
}

#[test]
fn a_class_annotation_over_a_global_table_is_checked_on_the_spot() {
    // The `local X = {}` carrier defers its conformance to the *final*
    // accumulated shape (a luabox leniency). The assignment spellings do not:
    // the literal is checked where it is written, which is what luals does for
    // both. Documented in `docs/03-reference/02-limitations.md`.
    let src = "\
---@class Iface
---@field a string

---@type Iface
G = {}
G.a = \"x\"
";
    assert_eq!(codes(src), vec!["LB0302"]);
}

#[test]
fn the_local_carrier_deferral_is_unchanged() {
    let src = "\
---@class Iface
---@field a string

---@type Iface
local L = {}
L.a = \"x\"
return L
";
    assert_eq!(codes(src), none());
}

#[test]
fn an_annotation_on_a_call_target_is_not_a_slot() {
    // Only assignment *targets* take an annotation; a bare call statement
    // under a `---@type` declares nothing and must not manufacture a
    // diagnostic.
    let src = "\
---@type string
print(1)
";
    assert_eq!(codes(src), none());
}
