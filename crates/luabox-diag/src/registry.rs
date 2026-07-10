//! The static registry of known diagnostic codes and their explain pages.
//!
//! Each entry carries a short `title` and a Markdown `explain` page surfaced
//! by `luabox explain <code>`. The `LB2xxx` shape codes (SHAPES.md §5) are
//! *reserved* here — the checker that emits them lands in P1 — so their
//! explain text documents the condition and points at the shape spec.

use crate::code::Code;

/// A registry entry: a code, its title, and its Markdown explain page.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    /// The code this entry documents.
    pub code: Code,
    /// A short human title (rustc calls this the message template).
    pub title: &'static str,
    /// The full explain page, in Markdown.
    pub explain: &'static str,
}

/// Every known diagnostic code, in ascending order.
static REGISTRY: &[Entry] = &[
    Entry {
        code: Code::new(1),
        title: "syntax error",
        explain: LB0001,
    },
    Entry {
        code: Code::new(10),
        title: "`goto`/label not available in this edition",
        explain: LB0010,
    },
    Entry {
        code: Code::new(11),
        title: "integer division `//` not available in this edition",
        explain: LB0011,
    },
    Entry {
        code: Code::new(12),
        title: "bitwise operator not available in this edition",
        explain: LB0012,
    },
    Entry {
        code: Code::new(13),
        title: "`<const>`/`<close>` attribute not available in this edition",
        explain: LB0013,
    },
    Entry {
        code: Code::new(14),
        title: "hex float literal not available in this edition",
        explain: LB0014,
    },
    Entry {
        code: Code::new(15),
        title: "`\\z`/`\\x` string escape not available in this edition",
        explain: LB0015,
    },
    Entry {
        code: Code::new(16),
        title: "`\\u{...}` string escape not available in this edition",
        explain: LB0016,
    },
    Entry {
        code: Code::new(1001),
        title: "unknown edition",
        explain: LB1001,
    },
    Entry {
        code: Code::new(2001),
        title: "missing non-optional field on shape-bound literal",
        explain: LB2001,
    },
    Entry {
        code: Code::new(2002),
        title: "unknown key on sealed shape",
        explain: LB2002,
    },
    Entry {
        code: Code::new(2003),
        title: "incomplete `---@impl`",
        explain: LB2003,
    },
    Entry {
        code: Code::new(2004),
        title: "impl signature mismatch",
        explain: LB2004,
    },
    Entry {
        code: Code::new(2005),
        title: "unresolved `---@use` module",
        explain: LB2005,
    },
    Entry {
        code: Code::new(2006),
        title: "`---@struct` names an undeclared struct",
        explain: LB2006,
    },
    Entry {
        code: Code::new(2007),
        title: "generic bound unsatisfied",
        explain: LB2007,
    },
    Entry {
        code: Code::new(2008),
        title: "supertrait conformance missing",
        explain: LB2008,
    },
    Entry {
        code: Code::new(2010),
        title: "body in `.lb` file",
        explain: LB2010,
    },
];

/// Look up the explain entry for a code, if it is registered.
#[must_use]
pub fn explain(code: &Code) -> Option<&'static Entry> {
    REGISTRY.iter().find(|entry| entry.code == *code)
}

/// Every registered entry, ascending by code. Useful for `explain --list`
/// style commands and completeness checks.
#[must_use]
pub fn all() -> &'static [Entry] {
    REGISTRY
}

const LB0001: &str = "\
# LB0001: syntax error

The parser reached a token that is not valid at this position, or hit the end
of the file while a construct was still open.

Common causes:

- A missing `end`, `)`, `}`, or `then`.
- A stray keyword or operator where an expression or statement was expected.
- Mismatched string or long-bracket / comment delimiters.

luabox parses with a lossless, error-resilient parser: it keeps going past the
error so later diagnostics and editor features still work, but the offending
region is reported here.

Fix the highlighted token and re-run. If the construct is only valid in a Lua
dialect newer than your `edition`, either raise the edition in `luabox.toml`
or rewrite it for the edition you target.
";

const LB0010: &str = "\
# LB0010: `goto`/label not available in this edition

`goto` statements and `::label::` declarations are Lua 5.2+ (also LuaJIT).
Lua 5.1 has no `goto` statement at all — `goto` lexes as an ordinary
identifier there, so `goto top` in 5.1 source is a variable/call, not a jump.

```lua
-- illegal under edition = \"5.1\"
::top::
i = i + 1
if i < 10 then goto top end

-- workaround: restructure with a loop and a flag
local more = true
while more do
  i = i + 1
  more = i < 10
end
```

Raise `edition` to `5.2` or later (or `luajit`), or restructure the control
flow by hand. `luabox build` can also lower `goto`/labels down to 5.1 via a
loop/flag rewrite for code that targets an older runtime (SPEC.md §2.1);
irreducible gotos are a hard diagnostic at build time.
";

const LB0011: &str = "\
# LB0011: integer division `//` not available in this edition

`//` (floor division) is Lua 5.3+. It is not available in 5.1, 5.2, or
LuaJIT.

```lua
-- illegal under edition = \"5.1\"/\"5.2\"/\"luajit\"
local q = a // b

-- workaround
local q = math.floor(a / b)
```

Raise `edition` to `5.3` or later, or use `math.floor(a / b)` directly.
`luabox build --target 5.2`/`5.1`/`luajit` lowers `//` to a `math.floor`
call automatically (SPEC.md §2.1).
";

const LB0012: &str = "\
# LB0012: bitwise operator not available in this edition

The bitwise operators `&`, `|`, `~` (binary xor and unary not), `<<`, and
`>>` are Lua 5.3+. They are not available in 5.1, 5.2, or LuaJIT (LuaJIT's
bitwise ops live in the `bit.*` library instead, with different semantics).
Note `~=` (not-equal) is unaffected — it is a distinct operator, legal in
every edition.

```lua
-- illegal under edition = \"5.1\"/\"5.2\"/\"luajit\"
local mask = a & b | c

-- workaround (5.2, with the `bit32` library)
local mask = bit32.bor(bit32.band(a, b), c)
-- workaround (LuaJIT, with the `bit` library)
local mask = bit.bor(bit.band(a, b), c)
```

Raise `edition` to `5.3` or later, or call the `bit32` (5.2) / `bit`
(LuaJIT) shim library directly. `luabox build` injects the matching shim
automatically when lowering to an older target (SPEC.md §2.1).
";

const LB0013: &str = "\
# LB0013: `<const>`/`<close>` attribute not available in this edition

Local variable attributes (`local x <const> = 1`, `local f <close> = …`)
are Lua 5.4 only.

```lua
-- illegal under edition = \"5.1\"/\"5.2\"/\"5.3\"/\"luajit\"
local x <const> = 1

-- workaround: drop the attribute (lose the compile-time guarantee)
local x = 1
```

Raise `edition` to `5.4`, or drop the attribute. `luabox build` lowers
`<const>` by dropping it (with a compile-time const-check already having
run) and lowers `<close>` via a `pcall`-wrapped scope-exit rewrite; truly
non-lowerable `<close>` semantics (e.g. under error inside a 5.1 coroutine)
are a hard diagnostic with the `---@luabox-allow lossy-lowering` escape
hatch (SPEC.md §2.1).
";

const LB0014: &str = "\
# LB0014: hex float literal not available in this edition

Hexadecimal float literals (`0x1p4`, `0x1.8p3`, …) are Lua 5.2+ (also
LuaJIT). Plain hex integers (`0xBEBADA`) are unaffected — they are legal in
every edition.

```lua
-- illegal under edition = \"5.1\"
local x = 0x1p4

-- workaround: write the equivalent decimal float
local x = 16.0
```

Raise `edition` to `5.2` or later (or `luajit`), or write the literal in
decimal form.
";

const LB0015: &str = "\
# LB0015: `\\z`/`\\x` string escape not available in this edition

The `\\z` (skip following whitespace) and `\\xXX` (hex byte) string escapes
are Lua 5.2+ (also LuaJIT).

```lua
-- illegal under edition = \"5.1\"
local s = \"a\\z
           b\\x41\"

-- workaround: avoid the escapes
local s = \"a\" ..
          \"b\" .. string.char(0x41)
```

Raise `edition` to `5.2` or later (or `luajit`), or rewrite the string
without them.
";

const LB0016: &str = "\
# LB0016: `\\u{...}` string escape not available in this edition

The `\\u{XXX}` Unicode escape (UTF-8 encoded at compile time) is Lua 5.3+.
It is not available in 5.1, 5.2, or LuaJIT.

```lua
-- illegal under edition = \"5.1\"/\"5.2\"/\"luajit\"
local s = \"\\u{2603}\"

-- workaround: spell out the UTF-8 bytes
local s = \"\\xE2\\x98\\x83\"
```

Raise `edition` to `5.3` or later, or spell out the encoded bytes directly.
`luabox build` can perform this substitution automatically when lowering to
an older target.
";

const LB1001: &str = "\
# LB1001: unknown edition

`edition` in `luabox.toml` (or the `--edition` flag) names the Lua dialect you
*write*. It must be one of:

- `5.1`
- `5.2`
- `5.3`
- `5.4`
- `luajit`

The value supplied is none of these.

Note: **Luau is intentionally out of scope** (SPEC.md §1). It is a separate
typed paradigm with its own owner and toolchain; luabox's typed story is the
`.lb` shape DSL layered over untyped Lua, so `luau` is not a valid edition.
";

const LB2001: &str = "\
# LB2001: missing non-optional field on shape-bound literal

A table bound to a struct with `---@struct` (or produced by
`setmetatable(literal, Carrier)`) omits a field the struct declares as
non-optional. Structs are *sealed*: every non-`?` field must be present.

Add the missing field, or mark it optional in the `.lb` declaration
(`field: T?`).

Reserved code — the shape checker that emits it lands in P1. See SHAPES.md §5.
";

const LB2002: &str = "\
# LB2002: unknown key on sealed shape

A read or write used a key the struct does not declare. Sealed structs reject
unknown keys. To allow extra keys, open the struct with `..` in its `.lb`
declaration (extras then type as `unknown`).

Reserved code — the shape checker that emits it lands in P1. See SHAPES.md §5.
";

const LB2003: &str = "\
# LB2003: incomplete `---@impl`

An `---@impl Trait for Struct` carrier does not define every function the trait
requires. The diagnostic lists the missing functions; a fix-it can generate
stubs for them.

Reserved code — the shape checker that emits it lands in P1. See SHAPES.md §5.
";

const LB2004: &str = "\
# LB2004: impl signature mismatch

A function on an `---@impl` carrier does not match the trait signature:
parameters are contravariant, returns covariant, and a `:` vs `.` receiver
must agree with `self`. Both the trait and the implementation spans are shown.

Reserved code — the shape checker that emits it lands in P1. See SHAPES.md §5.
";

const LB2005: &str = "\
# LB2005: unresolved `---@use` module

A `---@use <module>` (or a `use` inside a `.lb` file) names a shape module that
could not be resolved. Resolution tries, in order: a sibling `<name>.lb`, the
`[types] shape-paths` directories, then dependency-exported shapes (SHAPES.md
§6). Same-tier ambiguity is also an error.

Reserved code — the shape checker that emits it lands in P1. See SHAPES.md §5.
";

const LB2006: &str = "\
# LB2006: `---@struct` names an undeclared struct

A `---@struct <Name>` annotation refers to a struct that no in-scope shape
module declares. Check the name and that the module is imported with `---@use`.

Reserved code — the shape checker that emits it lands in P1. See SHAPES.md §5.
";

const LB2007: &str = "\
# LB2007: generic bound unsatisfied

A generic shape was instantiated with a type argument that does not satisfy the
declared bound. Generics are monomorphised per use site and bound violations
are reported at the call, rustc-style.

Reserved code — the shape checker that emits it lands in P1. See SHAPES.md §5.
";

const LB2008: &str = "\
# LB2008: supertrait conformance missing

An `impl` for a trait with supertraits (`trait Drawable: Shape`) requires the
carrier to also conform to every supertrait. Add the missing `---@impl` for the
supertrait on the same carrier.

Reserved code — the shape checker that emits it lands in P1. See SHAPES.md §5.
";

const LB2010: &str = "\
# LB2010: body in `.lb` file

A `.lb` file contains a body or expression. Shape files are declaration-only:
no bodies, no expressions. Implementations live in `.lua` and bind with
`---@impl`.

Reserved code — the shape parser that emits it lands in P1. See SHAPES.md §5.
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_codes_are_all_present() {
        for raw in [
            "LB0001", "LB0010", "LB0011", "LB0012", "LB0013", "LB0014", "LB0015", "LB0016",
            "LB1001", "LB2001", "LB2002", "LB2003", "LB2004", "LB2005", "LB2006", "LB2007",
            "LB2008", "LB2010",
        ] {
            let code: Code = raw.parse().unwrap();
            assert!(explain(&code).is_some(), "{raw} missing from registry");
        }
    }

    #[test]
    fn every_entry_has_nonempty_title_and_explain() {
        for entry in all() {
            assert!(!entry.title.is_empty(), "{} has empty title", entry.code);
            assert!(
                entry.explain.trim().len() > 20,
                "{} has a suspiciously short explain page",
                entry.code
            );
            // Explain pages are Markdown headed by the code itself.
            assert!(
                entry.explain.contains(&entry.code.to_string()),
                "{} explain page does not mention its own code",
                entry.code
            );
        }
    }

    #[test]
    fn registry_is_sorted_and_unique() {
        for pair in all().windows(2) {
            assert!(
                pair[0].code < pair[1].code,
                "registry not strictly ascending at {} / {}",
                pair[0].code,
                pair[1].code
            );
        }
    }

    #[test]
    fn unknown_code_is_none() {
        let code: Code = "LB9999".parse().unwrap();
        assert!(explain(&code).is_none());
    }
}
