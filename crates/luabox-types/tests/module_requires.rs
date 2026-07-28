//! The static `require` inventory ([`luabox_types::module_requires`]) — the
//! keys a cross-file export registry is built over.
//!
//! Literal module strings in source order; dynamic (non-literal) requires are
//! excluded, since they are unresolvable and the bundler hard-errors on them.

use luabox_syntax::lua::{Dialect, parse};
use luabox_types::module_requires;

#[test]
fn module_requires_lists_static_strings_in_source_order() {
    let parse = parse(
        "\
local a = require(\"pkg.alpha\")
local b = require(\"pkg.beta\")
local c = require(\"pkg.alpha\")
",
        Dialect::Lua54,
    );
    assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
    assert_eq!(
        module_requires(&parse),
        vec!["pkg.alpha", "pkg.beta", "pkg.alpha"]
    );
}

#[test]
fn module_requires_excludes_dynamic_requires() {
    let parse = parse(
        "\
local name = \"pkg.\" .. suffix
local dynamic = require(name)
local static = require(\"pkg.static\")
",
        Dialect::Lua54,
    );
    assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
    assert_eq!(module_requires(&parse), vec!["pkg.static"]);
}

#[test]
fn module_requires_is_empty_without_requires() {
    let parse = parse("local x = 1\n", Dialect::Lua54);
    assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
    assert_eq!(module_requires(&parse), Vec::<String>::new());
}
