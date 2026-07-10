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
        code: Code::new(300),
        title: "type mismatch",
        explain: LB0300,
    },
    Entry {
        code: Code::new(301),
        title: "wrong argument count",
        explain: LB0301,
    },
    Entry {
        code: Code::new(302),
        title: "missing required field",
        explain: LB0302,
    },
    Entry {
        code: Code::new(303),
        title: "unknown field",
        explain: LB0303,
    },
    Entry {
        code: Code::new(304),
        title: "return count/type mismatch",
        explain: LB0304,
    },
    Entry {
        code: Code::new(305),
        title: "unknown type name in annotation",
        explain: LB0305,
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

const LB0300: &str = "\
# LB0300: type mismatch

A value flowed into a slot whose annotated type does not accept it: an
argument against its `---@param`, an initializer or assignment against a
`---@type` local, or a table-literal field against its `---@field` type.

```lua
---@param n number
local function double(n) return n * 2 end

double(\"nope\")   -- LB0300: expected `number`, found `\"nope\"`
```

Argument and value types come from annotations and literals (P0 subset):
literals, table constructors, `---@type` locals, references to and calls of
annotated functions. Everything else is `unknown` — in warn mode `unknown`
flows freely; in strict mode (`[types] strict = true`) passing `unknown`
where a concrete type is expected is itself this error, because untyped
code is `unknown`, not `any` (SPEC.md §3).

Fix the value, fix the annotation, or annotate the value's source so the
checker can see its type.
";

const LB0301: &str = "\
# LB0301: wrong argument count

A call to an annotated function supplies fewer arguments than its
non-optional `---@param`s require, or more than its parameter list (plus
`...`) accepts.

```lua
---@param a number
---@param b? number
local function f(a, b) end

f()          -- LB0301: takes at least 1 argument
f(1, 2, 3)   -- LB0301: takes at most 2 arguments
```

Optional parameters (`---@param b? T`) and a `...` vararg relax the limit.
A call or `...` in the final argument position counts as open-ended: too-few
diagnostics are suppressed because its value count is unknowable statically.
";

const LB0302: &str = "\
# LB0302: missing required field

A table literal checked against a `---@class` (or table type) omits a field
the type requires. Fields are required unless declared optional
(`---@field name? T`) or nilable (`T?`/`T|nil`).

```lua
---@class Point
---@field x number
---@field y number

---@param p Point
local function use(p) end

use({ x = 1 })   -- LB0302: missing required field `y`
```

Each missing field is reported separately. Add the field, or mark it
optional in the class declaration.
";

const LB0303: &str = "\
# LB0303: unknown field

A table literal checked against a `---@class` (or table type) contains a
field the type does not declare. Following LuaLS, unknown fields on a class
*literal* are diagnosed — the class is closed for literals unless it
declares an indexer (`---@field [string] T`), which accepts (and types) the
extras. Plain assignability between already-constructed tables stays
width-based: extra fields there are fine.

```lua
---@class Point
---@field x number
---@field y number

---@param p Point
local function use(p) end

use({ x = 1, y = 2, z = 3 })   -- LB0303: unknown field `z`
```

Remove the field, add it to the class, or open the class with an indexer.
";

const LB0304: &str = "\
# LB0304: return count/type mismatch

A `return` inside a function annotated with `---@return` returns the wrong
number of values, or a value of the wrong type.

```lua
---@return number, string
local function pair()
  return 1        -- LB0304: expected 2 return values
end

---@return number
local function n()
  return \"no\"   -- LB0304: expected `number`, found `\"no\"`
end
```

Trailing declared returns that accept `nil` (`T?`) may be omitted. A call
or `...` in the final position is open-ended and suppresses the too-few
check. `---@return T ...` allows any number of extra values.
";

const LB0305: &str = "\
# LB0305: unknown type name in annotation

An annotation references a type name that is neither built-in (`nil`,
`boolean`, `number`, `integer`, `string`, `table`, `function`, `thread`,
`userdata`, `any`, `unknown`, ...) nor declared in the file as a
`---@class`, `---@alias`, or `---@enum`.

```lua
---@param x Wibble   -- LB0305: unknown type name `Wibble`
local function f(x) end
```

Declare the type, fix the spelling, or — if it comes from another module —
note that cross-file annotation resolution (`require`, `---@meta`
definition packages) lands in P1; until then each file is checked against
its own declarations. The unresolved type is treated as `unknown`.
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

A table bound to a struct with `---@struct` (or passed to
`setmetatable(literal, Carrier)` where the carrier is struct-bound) omits a
field the struct declares as non-optional. Structs are *sealed* (SHAPES.md
§5): every field not marked `?` must be present.

```rust
// geometry.lb
struct Point { x: number, y: number, label: string? }
```

```lua
---@use geometry

---@struct Point
local p = { x = 0 }        -- LB2001: missing non-optional field `y`
                           -- (`label` may be omitted: it is `string?`)
```

Shape rules are hard errors at every strictness level — the `---@struct`
binding is itself the opt-in.

Add the missing field, or mark it optional in the `.lb` declaration
(`field: T?`).
";

const LB2002: &str = "\
# LB2002: unknown key on sealed shape

A table literal, field read, or field write used a key the struct does not
declare. Sealed structs reject unknown keys outright (SHAPES.md §5).

```rust
// geometry.lb
struct Point { x: number, y: number }
struct Bag { n: number, .. }          // `..` opens the shape
```

```lua
---@use geometry

---@struct Point
local p = { x = 0, y = 0, z = 0 }   -- LB2002: unknown key `z`
p.w = 1                             -- LB2002: unknown key `w`
print(p.v)                          -- LB2002: unknown key `v`

---@struct Bag
local b = { n = 1, extra = true }   -- fine: Bag is open, extras are `unknown`
```

Remove the key, add the field to the struct, or open the struct with `..`
(extra keys then type as `unknown`). Carrier method names (`function
Point:magnitude()`) and `__`-prefixed metafields are always allowed.
";

const LB2003: &str = "\
# LB2003: incomplete `---@impl`

An `---@impl Trait for Struct` carrier does not define every function the
trait requires. The diagnostic lists all missing functions.

```rust
// geometry.lb
trait Shape {
    fn area(self) -> number;
    fn perimeter(self) -> number;
}
struct Circle { radius: number }
```

```lua
---@use geometry

---@struct Circle
local Circle = {}
Circle.__index = Circle

---@impl Shape for Circle           -- LB2003: missing `perimeter`
function Circle:area()
  return math.pi * self.radius ^ 2
end
```

Define the listed functions on the same carrier (`function Circle:perimeter()
... end`). Extra inherent methods beyond the trait are always fine.
";

const LB2004: &str = "\
# LB2004: impl signature mismatch

A function on an `---@impl` carrier does not match the trait's declared
signature. Parameters are contravariant (the implementation must accept
everything the trait promises callers may pass), returns are covariant (the
implementation must return something the trait's return type accepts), and
the `:` vs `.` receiver must agree with `self` in the trait. Both spans are
shown: the implementation and the trait declaration.

```rust
// geometry.lb
trait Shape {
    fn area(self) -> number;
}
struct Circle { radius: number }
```

```lua
---@use geometry

---@struct Circle
local Circle = {}
Circle.__index = Circle

---@impl Shape for Circle
---@return string
function Circle:area()             -- LB2004: expected return `number`,
  return \"round\"                   --         found `string`
end
```

Also raised when the arity differs, when a parameter type cannot accept the
trait's, and when a `self` trait function is declared with `.` (or vice
versa). A `.`-declared function with an explicit leading `self` parameter
counts as taking `self`. `Result<T, E>` in a trait return position means the
multi-return pair `(T?, E?)` — annotate the implementation `---@return T?,
E?` (SHAPES.md §12.1).
";

const LB2005: &str = "\
# LB2005: unresolved `---@use` module

A `---@use <module>` (or a `use` inside a `.lb` file) names a shape module
that could not be resolved. Resolution tries, first hit wins (SHAPES.md §6):

1. a sibling `<module>.lb` next to the using file;
2. the `[types] shape-paths` directories from `luabox.toml`, in order —
   more than one hit *within* this tier is an ambiguity, also this error;
3. dependency-exported shapes (`[types] shapes` in the dependency's
   manifest) — **not searched yet**; that tier lands in P2.

```lua
---@use geometry     -- LB2005 if no geometry.lb is found in tiers 1–2
```

Create the `.lb` file next to the using file, add its directory to
`[types] shape-paths`, or fix the spelling. For an ambiguity, remove or
rename one of the competing files (the diagnostic lists the candidates).
";

const LB2006: &str = "\
# LB2006: `---@struct` names an undeclared struct

A `---@struct <Name>` (or the trait/struct in an `---@impl`) refers to a
name that no shape module in scope declares.

```rust
// geometry.lb
struct Point { x: number, y: number }
```

```lua
---@use geometry

---@struct Piont      -- LB2006: undeclared struct (typo)
local p = { x = 0, y = 0 }
```

Check the spelling, and check the declaring module is imported with
`---@use`. For `---@impl T for S`, `S` may also be a `---@class` declared in
the same file (LuaCATS interop) — but `T` must be a `.lb` trait.
";

const LB2007: &str = "\
# LB2007: generic bound unsatisfied

A generic shape was instantiated with a type argument that does not satisfy
the parameter's declared bound. Generics are monomorphised per use site and
violations are reported at the use site, rustc-style (SHAPES.md §5).

```rust
// geometry.lb
trait Shape { fn area(self) -> number; }
struct Circle { radius: number }
impl Shape for Circle;

struct Holder<T: Shape> { value: T }
struct Ok  { h: Holder<Circle> }   // fine: impl Shape for Circle
struct Bad { h: Holder<number> }   // LB2007: `number` is not `Shape`
```

The same check runs at `.lua` binding sites (`---@struct Holder<number>`).
Conformance comes from an `impl Bound for Arg;` assertion in a shape module
in scope. Also raised for a wrong number of type arguments.
";

const LB2008: &str = "\
# LB2008: supertrait conformance missing

An `---@impl` of a trait with supertraits (`trait Drawable: Shape`) requires
the carrier to conform to every supertrait as well, on the same carrier.

```rust
// geometry.lb
trait Shape    { fn area(self) -> number; }
trait Drawable: Shape { fn draw(self); }
struct Circle  { radius: number }
```

```lua
---@use geometry

---@struct Circle
local Circle = {}
Circle.__index = Circle

---@impl Drawable for Circle       -- LB2008: `Shape` conformance missing
function Circle:draw() end
```

Add `---@impl Shape for Circle` (with its required functions) to the same
carrier, or assert `impl Shape for Circle;` in a `.lb` module.
";

const LB2010: &str = "\
# LB2010: body in `.lb` file

A `.lb` shape file contains a function body. Shape files are
declaration-only — no bodies, no expressions (SHAPES.md §3). Implementations
live in `.lua` and bind with `---@impl`.

```rust
// geometry.lb
trait Shape {
    fn area(self) -> number { return 1 }   // LB2010
}
```

Write the signature only, terminated with `;`:

```rust
trait Shape {
    fn area(self) -> number;
}
```

and implement it in Lua:

```lua
---@impl Shape for Circle
function Circle:area() return math.pi * self.radius ^ 2 end
```
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_codes_are_all_present() {
        for raw in [
            "LB0001", "LB0010", "LB0011", "LB0012", "LB0013", "LB0014", "LB0015", "LB0016",
            "LB0300", "LB0301", "LB0302", "LB0303", "LB0304", "LB0305", "LB1001", "LB2001",
            "LB2002", "LB2003", "LB2004", "LB2005", "LB2006", "LB2007", "LB2008", "LB2010",
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
