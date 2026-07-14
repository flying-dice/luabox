// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]
//! `---@class Impl : Interface` conformance (#107) — pure LuaCATS.
//!
//! luals declares `: Shape` but trusts it; luabox verifies the carrier
//! actually provides the interface's members with compatible signatures,
//! `__index`-aware so classic inheritance is not wrongly flagged. No ambient
//! defs, no second type format — LuaCATS annotations are the one type format
//! (DIRECTION.md).

use luabox_diag::Diagnostic;
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::Strictness;

/// Check a plain LuaCATS source (no ambient defs) at strict.
fn check_plain(source: &str) -> Vec<Diagnostic> {
    let parsed = parse(source, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
    luabox_types::check_file(&parsed, "src/main.lua", Strictness::Strict, Dialect::Lua54)
}

fn plain_codes(source: &str) -> Vec<String> {
    check_plain(source)
        .iter()
        .map(|d| d.code.to_string())
        .collect()
}

/// A `Shape` interface plus a `Circle` carrier body, parameterised over the
/// members the carrier actually defines.
const SHAPE_INTERFACE: &str = "\
---@class Shape
---@field area fun(self): number
---@field perimeter fun(self): number
";

#[test]
fn class_conformance_missing_method_flagged() {
    // Circle claims `: Shape` but defines only `area` — `perimeter` is
    // missing, and luabox says so at the `---@class Circle` annotation.
    let src = format!(
        "{SHAPE_INTERFACE}\
---@class Circle : Shape
local Circle = {{}}
Circle.__index = Circle

---@return number
function Circle:area()
  return 1
end

return Circle
"
    );
    let diags = check_plain(&src);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB0300");
    assert!(
        diags[0].message.contains("perimeter"),
        "{}",
        diags[0].message
    );
    // Anchored at the `---@class Circle : Shape` annotation, not a method.
    let label = diags[0].primary_label().expect("primary label");
    assert_eq!(&src[label.span.range.clone()], "@class Circle : Shape");
    // The parent `Shape` is declared in this file, so its declaration site is
    // attached as a secondary "declared here" label (#107 requirement 3).
    assert!(
        diags[0]
            .labels
            .iter()
            .any(|l| !l.primary && l.message == "`Shape` declared here"),
        "{:?}",
        diags[0]
    );
}

#[test]
fn class_conformance_complete_passes() {
    // A carrier providing every interface member with compatible signatures
    // is silent.
    let src = format!(
        "{SHAPE_INTERFACE}\
---@class Circle : Shape
local Circle = {{}}
Circle.__index = Circle

---@return number
function Circle:area()
  return 1
end

---@return number
function Circle:perimeter()
  return 2
end

return Circle
"
    );
    assert_eq!(plain_codes(&src), Vec::<String>::new());
}

#[test]
fn class_conformance_wrong_signature_flagged() {
    // `area` is provided but returns a string where `Shape` demands a number:
    // a signature mismatch naming the member.
    let src = format!(
        "{SHAPE_INTERFACE}\
---@class Circle : Shape
local Circle = {{}}
Circle.__index = Circle

---@return string
function Circle:area()
  return \"nope\"
end

---@return number
function Circle:perimeter()
  return 2
end

return Circle
"
    );
    let diags = check_plain(&src);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB0300");
    assert!(diags[0].message.contains("area"), "{}", diags[0].message);
    assert!(
        diags[0].message.contains("wrong type"),
        "{}",
        diags[0].message
    );
}

#[test]
fn inherited_concrete_method_not_flagged() {
    // Classic inheritance: `Base` is a concrete carrier defining `area`;
    // `Child : Base` inherits it through a `Child.__index = Base` chain and
    // defines no `area` of its own. luabox must NOT tell Child to
    // re-implement `area` — the parent-carrier fallback counts it as
    // provided. This is the difference between "stricter" and "annoying".
    let src = "\
---@class Base
---@field area fun(self): number
local Base = {}
Base.__index = Base

---@return number
function Base:area()
  return 1
end

---@class Child : Base
local Child = {}
Child.__index = Base

return Child
";
    assert_eq!(plain_codes(src), Vec::<String>::new());
}

#[test]
fn undefined_parent_reported() {
    // `: Nope` names a type nothing declares — LB0305 at the parent
    // reference (no conformance obligation can be formed from it).
    let src = "\
---@class X : Nope
local X = {}
return X
";
    let diags = check_plain(src);
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0].code.to_string(), "LB0305");
    let label = diags[0].primary_label().expect("primary label");
    assert_eq!(&src[label.span.range.clone()], "Nope");
}

#[test]
fn undefined_parent_forward_reference_ok() {
    // A parent declared LATER in the same file is a valid forward reference,
    // not an undefined-parent error.
    let src = "\
---@class Early : Late
local Early = {}
return Early

---@class Late
";
    // `Late` declares no members, so `Early` has nothing to conform to and
    // the forward reference resolves: no LB0305, no LB0300.
    assert_eq!(plain_codes(src), Vec::<String>::new());
}

#[test]
fn defs_class_declarations_not_checked() {
    // Inside a `---@meta` definition package the interface `---@class`es are
    // contracts, not carriers: a `local` bound to one incurs no conformance
    // obligation of its own, even when it omits a member.
    let src = "\
---@meta

---@class Shape
---@field area fun(self): number
---@field perimeter fun(self): number

---@class Circle : Shape
local Circle = {}
Circle.__index = Circle

---@return number
function Circle:area()
  return 1
end
";
    // No LB0300 despite the missing `perimeter` — defs are not carriers.
    let codes = plain_codes(src);
    assert!(
        !codes.iter().any(|c| c == "LB0300"),
        "defs must not raise conformance errors: {codes:?}"
    );
}
