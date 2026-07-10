//! The mechanical semantics net available without real runtimes
//! (differential execution is ticket #23): for every corpus snippet and
//! every dialect pair, a successful lowering must produce output that
//! parses with **zero errors** under the *target* dialect and passes the
//! target's dialect-legality validation with **zero findings**. Combined
//! with the exact-output tests this pins both directions: the rewrites are
//! the intended text, and the text is legal where it ships.

use luabox_lower::lower;
use luabox_syntax::{Dialect, lua};
use proptest::prelude::*;

/// Snippets covering every lowering rule plus plain cross-dialect Lua.
/// Each is valid under the dialect it is paired with (its "edition").
const CORPUS: &[(&str, Dialect)] = &[
    // Plain, dialect-neutral code.
    (
        "local function fib(n)\n  if n < 2 then return n end\n  return fib(n - 1) + fib(n - 2)\nend\nprint(fib(10))\n",
        Dialect::Lua51,
    ),
    (
        "local t = { 1, 2, x = 'y' }\nfor i = 1, #t do t[i] = t[i] * 2 end\n",
        Dialect::Lua54,
    ),
    // Floor division and bitops (5.3+).
    ("local q = a // b\nlocal r = a // b // c\n", Dialect::Lua53),
    (
        "local bits = a & b | c ~ d\nbits = bits << 1 >> 2\nbits = ~bits\n",
        Dialect::Lua53,
    ),
    ("x = (a + b) // (c - d)\ny = -a // b\n", Dialect::Lua54),
    // goto shapes (5.2+).
    (
        "local i = 0\n::top::\ni = i + 1\nif i < 3 then goto top end\nprint(i)\n",
        Dialect::Lua52,
    ),
    (
        "for i = 1, 10 do\n  if i % 2 == 0 then goto continue end\n  print(i)\n  ::continue::\nend\n",
        Dialect::Lua54,
    ),
    (
        "while true do\n  if a then goto continue end\n  mid()\n  if b then goto continue end\n  work()\n  ::continue::\nend\n",
        Dialect::Lua52,
    ),
    ("::spin::\nstep()\ngoto spin\n", Dialect::Lua52),
    // Attributes (5.4).
    ("local x <const> = 1\nprint(x + 1)\n", Dialect::Lua54),
    (
        "do\n  local h <close> = open()\n  use(h)\nend\nprint('after')\n",
        Dialect::Lua54,
    ),
    (
        "do\n  local a <close> = f()\n  mid()\n  local b <close> = g()\n  fin()\nend\n",
        Dialect::Lua54,
    ),
    // _ENV idioms (5.2+).
    (
        "local _ENV = setmetatable({}, mt)\nx = 1\nreturn x\n",
        Dialect::Lua52,
    ),
    ("print(_ENV)\n_ENV.field = 1\n", Dialect::Lua53),
    // LuaJIT bit library.
    (
        "x = bit.band(a, 3)\ny = bit.bor(x, bit.lshift(1, 4))\n",
        Dialect::LuaJit,
    ),
    (
        "local bit = require(\"bit\")\nreturn bit.tohex(bit.bxor(a, b))\n",
        Dialect::LuaJit,
    ),
    // Mixed: everything at once (5.4).
    (
        "local mask <const> = 0xFF\nlocal function f(v)\n  local r = v & mask\n  return r // 2\nend\n::again::\nif f(x) > 0 then goto again end\n",
        Dialect::Lua54,
    ),
];

proptest! {
    /// Lowered output must be legal under the target: reparse with zero
    /// errors, validate with zero dialect findings.
    #[test]
    fn lowered_output_is_target_legal(
        case in 0..CORPUS.len(),
        target in prop::sample::select(Dialect::ALL.as_slice()),
    ) {
        let (source, edition) = CORPUS[case];
        // Source must be legal under its own edition, or the corpus is broken.
        let own = lua::parse(source, edition);
        prop_assert!(own.errors().is_empty(), "corpus snippet does not parse: {source}");
        prop_assert!(
            lua::validate::validate(&own, edition).is_empty(),
            "corpus snippet illegal under its own edition: {source}"
        );

        let Ok(lowered) = lower(source, edition, target) else {
            // Hard diagnostics (irreducible constructs, edition→target pairs
            // outside the downgrade matrix) are a legitimate outcome; the
            // net only judges *successful* lowerings.
            return Ok(());
        };
        let parse = lua::parse(&lowered.text, target);
        prop_assert!(
            parse.errors().is_empty(),
            "lowered output does not parse under {target:?}:\n{}\nerrors: {:?}",
            lowered.text,
            parse.errors(),
        );
        let findings = lua::validate::validate(&parse, target);
        prop_assert!(
            findings.is_empty(),
            "lowered output illegal under {target:?}:\n{}\nfindings: {findings:?}",
            lowered.text,
        );
    }

    /// Lowering to the same dialect is byte-identity for the whole corpus.
    #[test]
    fn same_dialect_lowering_is_identity(case in 0..CORPUS.len()) {
        let (source, edition) = CORPUS[case];
        let lowered = lower(source, edition, edition).expect("identity lowering");
        prop_assert_eq!(lowered.text.as_str(), source);
        prop_assert!(lowered.polyfills.is_empty());
    }
}
