// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]
//! The display-inference surface behind editor inlay hints
//! ([`luabox_types::infer_display_types`]).
//!
//! Same inference as the checker, plus call-site parameter seeding — and
//! display-only, so nothing here can manufacture a diagnostic.

use luabox_syntax::lua::{Dialect, parse};
use luabox_types::ty::Ty;
use luabox_types::{DisplayTypes, infer_display_types};

fn display(source: &str) -> DisplayTypes {
    let parse = parse(source, Dialect::Lua54);
    assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
    infer_display_types(&parse, "test.lua", None, None)
}

fn binding_ty(types: &DisplayTypes, name: &str) -> String {
    types
        .bindings
        .iter()
        .find(|b| b.name == name)
        .map_or_else(|| panic!("no binding named `{name}`"), |b| b.ty.to_string())
}

#[test]
fn display_types_publish_bindings_returns_and_the_module_export() {
    let src = "\
local function area(w, h)
  return w * h
end
local size = area(3, 4)
local point = { x = 1, y = 2 }
return point
";
    let types = display(src);
    // Call-site seeding types the unannotated parameters...
    assert_eq!(binding_ty(&types, "w"), "integer");
    assert_eq!(binding_ty(&types, "h"), "integer");
    // ...and the binding range points at the declaration name token.
    let w = types
        .bindings
        .iter()
        .find(|b| b.name == "w")
        .expect("binding `w`");
    assert_eq!(&src[w.range.clone()], "w");
    // The unannotated function's inferred return is published.
    assert_eq!(types.returns.len(), 1, "{:?}", types.returns);
    assert_eq!(types.returns[0].returns, vec![Ty::Integer]);
    // The chunk's `return` is the module export.
    assert_eq!(
        types.module_export.as_ref().map(ToString::to_string),
        Some("{ x: 1, y: 2 }".to_string())
    );
}

#[test]
fn display_types_record_outgoing_call_arguments() {
    // Arguments observed at calls of functions this file does not define
    // are the parameter seeds a required module would consume.
    let src = "\
external_helper(\"a\", 2)
";
    let types = display(src);
    let args = types
        .outgoing_calls
        .get("external_helper")
        .expect("outgoing call recorded");
    assert_eq!(
        args.iter().map(ToString::to_string).collect::<Vec<_>>(),
        vec!["string", "integer"]
    );
}

#[test]
fn display_types_of_an_empty_chunk_are_empty() {
    let types = display("");
    assert_eq!(types, DisplayTypes::default());
}
