//! Fuzz LuaCATS annotation parsing (SPEC.md §3, §16.1).
//!
//! Mirrors the no-panic property tests in
//! `crates/luabox-syntax/src/luacats/tests.rs`, extended to full libFuzzer
//! byte-soup coverage:
//! - `luacats::parse_block` never panics on arbitrary text, and every
//!   returned span stays within the input's bounds.
//! - `luacats::harvest`, walking a real (dialect-agnostic — Lua 5.4 is the
//!   union-ish superset) parsed Lua tree built from the same arbitrary
//!   bytes, never panics, and every resolved target span stays in bounds.

#![no_main]

use libfuzzer_sys::fuzz_target;
use luabox_syntax::Dialect;
use luabox_syntax::lua::parse as lua_parse;
use luabox_syntax::luacats::{harvest, parse_block};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let text: &str = &text;

    let block = parse_block(text, 0);
    assert!(
        block.span.end <= text.len(),
        "block span out of bounds for {text:?}"
    );
    for tag in &block.tags {
        assert!(
            tag.span().end <= text.len(),
            "tag span out of bounds for {text:?}"
        );
    }
    for err in &block.errors {
        assert!(
            err.span.end <= text.len(),
            "error span out of bounds for {text:?}"
        );
    }

    let parsed = lua_parse(text, Dialect::Lua54);
    let items = harvest(&parsed);
    for item in &items {
        if let Some(t) = item.target {
            assert!(t.end <= text.len(), "harvest target out of bounds for {text:?}");
        }
    }
});
