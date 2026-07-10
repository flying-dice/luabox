//! Fuzz `lua::parse` across every dialect (SPEC.md §16.1).
//!
//! Invariants checked for arbitrary input, per [`Dialect`]:
//! - `parse` never panics (libFuzzer catches this for free — any panic is a
//!   crash).
//! - Losslessness: `parse.syntax().text()` is byte-identical to the input.
//! - If the parse is clean (no errors), `fmt::format` of that same input
//!   must itself produce something that reparses cleanly — `format` already
//!   falls back to the original text when unsafe, so this mostly proves the
//!   safety net holds rather than crashing or reintroducing errors.

#![no_main]

use libfuzzer_sys::fuzz_target;
use luabox_syntax::Dialect;
use luabox_syntax::lua::{fmt, parse};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let text: &str = &text;

    for dialect in Dialect::ALL {
        let parsed = parse(text, dialect);

        assert_eq!(
            parsed.syntax().text().to_string(),
            text,
            "lossless roundtrip failed for dialect {dialect:?} on {text:?}"
        );

        if parsed.errors().is_empty() {
            let formatted = fmt::format(text, dialect);
            let reparsed = parse(&formatted, dialect);
            assert!(
                reparsed.errors().is_empty(),
                "format output failed to reparse cleanly for dialect {dialect:?}\ninput: {text:?}\nformatted: {formatted:?}"
            );
        }
    }
});
