//! How `---@param` lines bind to the parameters they name (#74), and the
//! legacy `EmmyLua` `---@vararg Type` spelling of `---@param ... Type`.
//!
//! `---@param` binds **by name**, not by position: annotating a subset, or
//! writing the lines out of order, must still type the right parameters — and
//! a name that matches nothing must not silently retype a neighbour.

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

fn strict_codes(source: &str) -> Vec<String> {
    codes(source, Strictness::Strict)
}

#[test]
fn partial_param_annotations_bind_by_name() {
    // Annotating only params 2..5 of a 6-param function must not shift
    // the tags onto the wrong positions (#74's exact repro).
    let src = "\
---@param b string
---@param c boolean
---@param d integer
---@param e string
local function f(a, b, c, d, e, g)
  return a, b, c, d, e, g
end
f(1, \"s\", true, 2, \"t\", 3)
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
    // And a call violating the *named* params errors per slot.
    let bad = src.replace(
        "f(1, \"s\", true, 2, \"t\", 3)",
        "f(1, true, \"s\", 2.5, 2, 3)",
    );
    assert_eq!(
        strict_codes(&bad),
        vec!["LB0300", "LB0300", "LB0300", "LB0300"]
    );
}

#[test]
fn out_of_order_param_tags_bind_by_name() {
    let src = "\
---@param b string
---@param a number
local function f(a, b) end
f(1, \"s\")
f(\"s\", 1)
";
    assert_eq!(strict_codes(src), vec!["LB0300", "LB0300"]);
}

#[test]
fn vararg_param_tag_binds_regardless_of_position() {
    let src = "\
---@param ... string
---@param a number
local function f(a, ...) end
f(1, \"x\", \"y\")
f(1, \"x\", 2)
";
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}

#[test]
fn legacy_vararg_tag_alone_types_dots() {
    // `---@vararg string` alone must type `...` exactly like
    // `---@param ... string` would: extra call args are checked against
    // it, so a non-string extra arg is a type mismatch.
    let src = "\
---@vararg string
local function f(...) end
f(\"x\", \"y\")
f(\"x\", 2)
";
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}

#[test]
fn legacy_vararg_tag_with_fixed_params() {
    // `---@vararg` combines with ordinary `---@param` tags on fixed
    // (non-vararg) parameters just like the modern `---@param ...` form
    // does in `vararg_param_tag_binds_regardless_of_position` above.
    let src = "\
---@param a number
---@vararg string
local function f(a, ...) end
f(1, \"x\", \"y\")
f(1, \"x\", 2)
";
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}

#[test]
fn legacy_vararg_tag_and_param_dots_union_per_luals() {
    // luals precedent (lua-language-server, `script/vm/compiler.lua`,
    // the `'...'`-source case): it loops over every doc bound to the AST
    // vararg node — `script/parser/luadoc.lua`'s `bindDoc` binds *both* a
    // `doc.vararg` tag and a `doc.param` tag whose `param[1] == '...'` to
    // that same source — and calls `vm.setNode(source, ...)` for each
    // one. `vm.setNode` without a `cover` argument *merges* (unions) into
    // the existing node rather than overwriting it (`script/vm/node.lua`,
    // `vm.setNode`). So when a block carries both a legacy `---@vararg`
    // and a modern `---@param ...`, luals's effective vararg type is the
    // union of the two — not "last tag wins" and not "param form wins".
    // We match that: string|number here, so a `boolean` extra arg is the
    // only misuse.
    let src = "\
---@vararg string
---@param ... number
local function f(...) end
f(\"x\")
f(1)
f(true)
";
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}

#[test]
fn duplicate_param_tag_names_first_wins() {
    let src = "\
---@param a number
---@param a string
local function f(a) end
f(1)
f(\"x\")
";
    assert_eq!(strict_codes(src), vec!["LB0300"]);
}

#[test]
fn param_tag_naming_no_parameter_is_unbound() {
    // TODO(P2): LuaLS warns here; today the tag is silently unbound and
    // the parameter stays permissive `unknown`.
    let src = "\
---@param sied number
local function f(side)
  return side
end
f(1)
f(\"anything\")
";
    assert_eq!(strict_codes(src), Vec::<String>::new());
}
