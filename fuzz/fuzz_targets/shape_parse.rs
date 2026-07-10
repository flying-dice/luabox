//! Fuzz the `.luab` shape grammar: `shape::parse` and `shape::format`
//! (SHAPES.md, SPEC.md §16.1).
//!
//! Invariants checked for arbitrary input:
//! - `shape::parse` never panics.
//! - Losslessness: `parse.syntax().text()` is byte-identical to the input.
//! - `shape::format` never panics and is idempotent:
//!   `format(format(text)) == format(text)`.

#![no_main]

use libfuzzer_sys::fuzz_target;
use luabox_syntax::shape;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let text: &str = &text;

    let parsed = shape::parse(text);
    assert_eq!(
        parsed.syntax().text().to_string(),
        text,
        "lossless roundtrip failed on {text:?}"
    );

    let once = shape::format(text);
    let twice = shape::format(&once);
    assert_eq!(
        once, twice,
        "shape::format not idempotent\ninput: {text:?}\nonce: {once:?}"
    );
});
