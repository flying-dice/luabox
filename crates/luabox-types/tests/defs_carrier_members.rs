// test code — panics document assumptions
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Carrier-style member definitions inside a `---@meta` definition file (#39).
//!
//! `---@field` is the defs convention, but luals makes no distinction: a
//! `function Class:method()` written in a library file is a member of `Class`
//! wherever it is written. A checked *project* file gets that for free —
//! inference walks the carrier table and the file's surface folds it into the
//! class — but a definition file is never inferred, so its carrier attachments
//! previously reached nothing and every use site reported `LB0306`.
//!
//! They are now harvested syntactically, with their signatures and their
//! use-site tags, and a same-name `---@field` stays authoritative on type
//! while inheriting the attachment's tags (the #33 rule).

use luabox_diag::Diagnostic;
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::{Ambient, Strictness, build_ambient, check_file_with_ambient};

/// A definition package whose members are written carrier-style.
const CARRIER_DEF: &str = "\
---@meta

---@class Widget
local Widget = {}

---@deprecated
---@async
---@param n integer
---@return string
function Widget:render(n) end

---@param a integer
---@return integer
function Widget.helper(a) end

function Widget:untagged() end

---@class Gadget
---@field spin fun(self: Gadget, turns: integer)
local Gadget = {}

---@deprecated
function Gadget:spin(turns) end
";

fn ambient() -> Ambient {
    build_ambient(Dialect::Lua54, &[CARRIER_DEF.to_string()])
}

fn check(src: &str) -> Vec<Diagnostic> {
    let parse = parse(src, Dialect::Lua54);
    assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
    let ambient = ambient();
    check_file_with_ambient(
        &parse,
        "test.lua",
        Strictness::Strict,
        Dialect::Lua54,
        Some(&ambient),
    )
}

fn strict_codes(src: &str) -> Vec<String> {
    check(src).iter().map(|d| d.code.to_string()).collect()
}

#[test]
fn a_carrier_method_is_a_member_of_the_class() {
    // Before #39 this was `LB0306`: the member existed only in the defs file's
    // syntax, never in `Widget`'s surface.
    let src = "\
---@param w Widget
local function use(w)
  return w:render(1)
end
return use
";
    // `render` is `---@deprecated` *and* `---@async`; the enclosing `use` is
    // not async, so both tags fire — and nothing else.
    assert_eq!(strict_codes(src), vec!["LB0308", "LB0316"]);
}

#[test]
fn a_carrier_method_signature_is_enforced() {
    let src = "\
---@param w Widget
---@async
local function use(w)
  return w:render(\"nope\")
end
return use
";
    assert_eq!(strict_codes(src), vec!["LB0308", "LB0300"]);
}

#[test]
fn a_carrier_method_return_type_flows_to_the_caller() {
    let src = "\
---@param w Widget
---@async
local function use(w)
  ---@type integer
  local n = w:render(1)
  return n
end
return use
";
    // `LB0300` is anchored at the `---@type integer` slot, ahead of the
    // deprecation reported at the call.
    assert_eq!(strict_codes(src), vec!["LB0300", "LB0308"]);
}

#[test]
fn a_dotted_carrier_function_is_a_member_of_the_class() {
    // `Widget.helper` joins the surface, so reading it off an instance is not
    // an undefined field. (Argument-checking a dotted call reached through a
    // *value* is a separate, pre-existing boundary — `callee_sig` resolves
    // dotted callees by name — and is unchanged by the fold: a `---@field`
    // declaration behaves identically here.)
    let src = "\
---@param w Widget
local function use(w)
  return w.helper(1)
end
return use
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn a_dotted_carrier_function_type_flows_to_the_caller() {
    // The signature really is on the member, not just its name: the declared
    // `---@return integer` reaches the assignment slot.
    let src = "\
---@param w Widget
local function use(w)
  ---@type string
  local s = w.helper(1)
  return s
end
return use
";
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}

#[test]
fn an_undocumented_carrier_method_joins_the_surface_permissively() {
    // No doc block at all: the member exists (no `LB0306`) and, exactly like
    // any unannotated function, is never arity- or type-checked.
    let src = "\
---@param w Widget
local function use(w)
  w:untagged(1, 2, 3)
end
return use
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}

#[test]
fn a_member_neither_declared_nor_attached_is_still_undefined() {
    // Folding only *adds* members: a genuine typo is still `LB0306`.
    let src = "\
---@param w Widget
local function use(w)
  w:nosuchthing()
end
return use
";
    assert_eq!(strict_codes(src), vec!["LB0306"]);
}

#[test]
fn a_field_declaration_stays_authoritative_over_a_same_name_attachment() {
    // `Gadget.spin` is declared `fun(self: Gadget, turns: integer)` and also
    // defined carrier-style with `---@deprecated`. The declaration governs the
    // signature; the attachment contributes the tag (#33).
    let src = "\
---@param g Gadget
local function use(g)
  g:spin(\"nope\")
end
return use
";
    assert_eq!(strict_codes(src), vec!["LB0308", "LB0300"]);
}

#[test]
fn a_carrier_attachment_does_not_leak_across_classes() {
    let src = "\
---@param g Gadget
local function use(g)
  g:render(1)
end
return use
";
    assert_eq!(strict_codes(src), vec!["LB0306"]);
}

#[test]
fn a_defs_file_with_no_class_carrier_is_unaffected() {
    // `function love.load()` on a plain global module table declares a
    // by-name callable, not a class member — unchanged by the fold.
    let ambient = build_ambient(
        Dialect::Lua54,
        &["---@meta\nlove = {}\n---@param n integer\nfunction love.load(n) end\n".to_string()],
    );
    let src = "love.load(\"nope\")\n";
    let parse = parse(src, Dialect::Lua54);
    assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
    let codes: Vec<String> = check_file_with_ambient(
        &parse,
        "test.lua",
        Strictness::Strict,
        Dialect::Lua54,
        Some(&ambient),
    )
    .iter()
    .map(|d| d.code.to_string())
    .collect();
    assert_eq!(codes, vec!["LB0300"]);
}
