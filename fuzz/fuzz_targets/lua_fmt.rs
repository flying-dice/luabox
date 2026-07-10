//! Fuzz `lua::fmt::format` (SPEC.md §16.1, §16.2 `fmt(fmt(x)) == fmt(x)`).
//!
//! `format` already carries its own internal safety net (broken input, or
//! any output that fails to reparse/preserve comments/preserve meaning,
//! comes back as the original text unchanged) — the point of fuzzing it is
//! to prove that net actually holds under arbitrary (mostly garbage) input
//! rather than panicking or silently violating it:
//! - `format` never panics.
//! - Idempotence: `format(format(text)) == format(text)`.
//! - The formatted output always reparses without errors (trivially true
//!   when `format` bailed out and returned the original text unchanged,
//!   since `format` only leaves errors in place by returning the input
//!   verbatim — this only fails if the safety net itself is broken).

#![no_main]

use libfuzzer_sys::fuzz_target;
use luabox_syntax::Dialect;
use luabox_syntax::lua::{fmt, parse};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let text: &str = &text;

    for dialect in Dialect::ALL {
        let once = fmt::format(text, dialect);
        let twice = fmt::format(&once, dialect);

        assert_eq!(
            once, twice,
            "format not idempotent for dialect {dialect:?}\ninput: {text:?}\nonce: {once:?}"
        );

        let reparsed = parse(&once, dialect);
        assert!(
            reparsed.errors().is_empty(),
            "formatted output failed to reparse cleanly for dialect {dialect:?}\ninput: {text:?}\nonce: {once:?}"
        );
    }
});
