//! Fuzz `lua::fmt::format` (SPEC.md §16.1, §16.2 `fmt(fmt(x)) == fmt(x)`).
//!
//! `format` already carries its own internal safety net (broken input, or
//! any output that fails to reparse/preserve comments/preserve meaning,
//! comes back as the original text unchanged) — the point of fuzzing it is
//! to prove that net actually holds under arbitrary (mostly garbage) input
//! rather than panicking or silently violating it:
//! - `format` never panics.
//! - Idempotence: `format(format(text)) == format(text)`.
//! - When the input parses cleanly for a dialect, the formatted output
//!   reparses cleanly for that dialect too. (When the input does NOT
//!   parse for the dialect, `format`'s safety net returns it verbatim —
//!   the reparse then reproduces the input's own errors by design, so
//!   asserting clean reparse unconditionally would reject the net
//!   working exactly as specified. Found by a corpus seed that is valid
//!   5.4 but not 5.1: `format(_, Lua51)` bailed verbatim and the
//!   unconditional assert fired.)

#![no_main]

use libfuzzer_sys::fuzz_target;
use luabox_syntax::Dialect;
use luabox_syntax::lua::{fmt, parse};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let text: &str = &text;

    for dialect in Dialect::ALL {
        let input_parses_cleanly = parse(text, dialect).errors().is_empty();

        let once = fmt::format(text, dialect);
        let twice = fmt::format(&once, dialect);

        assert_eq!(
            once, twice,
            "format not idempotent for dialect {dialect:?}\ninput: {text:?}\nonce: {once:?}"
        );

        if input_parses_cleanly {
            let reparsed = parse(&once, dialect);
            assert!(
                reparsed.errors().is_empty(),
                "formatted output failed to reparse cleanly for dialect {dialect:?}\ninput: {text:?}\nonce: {once:?}"
            );
        } else {
            assert_eq!(
                once, text,
                "format must return unparseable input verbatim for dialect {dialect:?}\ninput: {text:?}\nonce: {once:?}"
            );
        }
    }
});
