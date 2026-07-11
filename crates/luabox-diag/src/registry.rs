//! The static registry of known diagnostic codes and their explain pages.
//!
//! Each entry carries a short `title` and a Markdown `explain` page surfaced
//! by `luabox explain <code>`.

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
        code: Code::new(306),
        title: "undefined field read",
        explain: LB0306,
    },
    Entry {
        code: Code::new(307),
        title: "class declared by more than one definition package",
        explain: LB0307,
    },
    Entry {
        code: Code::new(500),
        title: "malformed `---@luabox-ignore`",
        explain: LB0500,
    },
    Entry {
        code: Code::new(501),
        title: "unused local (unused-local)",
        explain: LB0501,
    },
    Entry {
        code: Code::new(502),
        title: "unused parameter (unused-param)",
        explain: LB0502,
    },
    Entry {
        code: Code::new(503),
        title: "shadowed local (shadowed-local)",
        explain: LB0503,
    },
    Entry {
        code: Code::new(504),
        title: "assignment to a global (global-write)",
        explain: LB0504,
    },
    Entry {
        code: Code::new(505),
        title: "explicit nil comparison as truthiness (explicit-nil-compare-truthiness)",
        explain: LB0505,
    },
    Entry {
        code: Code::new(506),
        title: "string concatenation in a loop (concat-in-loop)",
        explain: LB0506,
    },
    Entry {
        code: Code::new(507),
        title: "`pairs` on an array (pairs-on-array)",
        explain: LB0507,
    },
    Entry {
        code: Code::new(508),
        title: "empty `if ... then` body (empty-then)",
        explain: LB0508,
    },
    Entry {
        code: Code::new(509),
        title: "read of an undefined global (undefined-global)",
        explain: LB0509,
    },
    Entry {
        code: Code::new(601),
        title: "irreducible `goto`",
        explain: LB0601,
    },
    Entry {
        code: Code::new(602),
        title: "assignment to a `<const>` variable",
        explain: LB0602,
    },
    Entry {
        code: Code::new(603),
        title: "`<close>` lowering fidelity",
        explain: LB0603,
    },
    Entry {
        code: Code::new(604),
        title: "`_ENV` use not lowerable",
        explain: LB0604,
    },
    Entry {
        code: Code::new(605),
        title: "LuaJIT extension not lowerable",
        explain: LB0605,
    },
    Entry {
        code: Code::new(606),
        title: "integer/float divergence on lowering",
        explain: LB0606,
    },
    Entry {
        code: Code::new(1001),
        title: "unknown edition",
        explain: LB1001,
    },
    Entry {
        code: Code::new(1002),
        title: "unresolvable definition package",
        explain: LB1002,
    },
    Entry {
        code: Code::new(1100),
        title: "known security advisory affects a locked dependency",
        explain: LB1100,
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

const LB0306: &str = "\
# LB0306: undefined field read

A field read (or `:` method call) names a key that provably does not exist
on the receiver's type. This is lua-language-server's `undefined-field`,
raised on the strictness ladder (SPEC.md §3, §19): a **warning** in warn
mode, an **error** in strict — stricter than luals, which always warns.

It fires in two situations, both requiring the absence to be *provable*.

## 1. Declared `---@class` values

Reading a field a value's declared class does not provide — no matching
`---@field` (own or inherited through the parent chain), no indexer, and no
inherent method on the carrier. The receiver may be `self` inside a class
method, a `---@type Class` local, a `---@return Class` constructor result,
or a cross-package class shared through a dependency's `---@meta` defs.

```lua
---@class geometry.Point
---@field x number
---@field y number
local Point = {}
Point.__index = Point

function Point:shift()
  return self.nope   -- LB0306: undefined field `nope` on `geometry.Point`
end
```

This is the strictness answer for annotated code (SPEC.md §19): a
declaration is the precondition. Un-annotated code invents no obligation —
a value of `unknown`/`any` type, or a plain structural table from
inference, is never flagged. A class with an indexer (`---@field [string]
T`) declares dynamic access, so any string key is admissible. Writes are
`LB0303`'s business (luals' `inject-field`), not this rule.

## 2. Inferred table shapes

Rich table inference (SPEC.md §3) tracks the *shape* of every table built
in the file — constructor entries, later `t.x = v` assignments, `function
T.f()` / `function T:m()` declarations, and `setmetatable`/`__index`
chains — so a read that none of those supply is a typo, not a dynamic
lookup.

```lua
local Circle = {}
Circle.__index = Circle

function Circle.new(radius)
  local o = setmetatable({}, Circle)
  o.radius = radius
  return o
end

function Circle:area()
  return 3.14 * self.radiuss ^ 2   -- LB0306: no field `radiuss`
end
```

## Conservatism

The check stays silent when the shape is not fully known: tables that
escape into unanalyzed code (arguments to unknown functions, global
writes), tables with dynamic-key writes or indexer types, and tables whose
metatable/`__index` cannot be resolved never produce this diagnostic.

Fix the spelling, declare the field, or — to acknowledge a genuinely
dynamic access — add `---@diagnostic disable: undefined-field` (also
`disable-line` / `disable-next-line`).
";

const LB0307: &str = "\
# LB0307: class declared by more than one definition package

A `---@class` of the same name is declared by more than one package's
`[types] defs` in scope. luabox shares LuaCATS types across a package
boundary the way lua-language-server shares a `workspace.library`: a
dependency's own `[types] defs` files join the consuming package's ambient
scope automatically (#108), and class names are a single global namespace
across the workspace and its libraries — there is no per-package
namespacing.

Where luals silently merges duplicate `---@class` declarations, luabox is
stricter: two packages declaring the same class name is a warning at the
second (losing) declaration, never a silent merge of conflicting fields.

```lua
-- <consumer>/defs/geometry.d.lua
---@class geometry.Point
---@field x number

-- <dependency>/defs/geometry.d.lua   → also declares geometry.Point: LB0307
---@class geometry.Point
---@field y number
```

**Deterministic winner.** Project-local defs are loaded first (the consumer
wins), then each direct dependency's defs alphabetically by dependency name.
The *first* declaration in that order wins — its fields are the ones the
checker uses — and every later declaration of the same name is reported
here, naming the file that already declared it.

Rename one class, drop the duplicate def entry, or (if the duplicate is a
hand-vendored copy of a dependency's types) delete the copy and let the
dependency's own defs reach you automatically.
";

const LB0500: &str = "\
# LB0500: malformed `---@luabox-ignore`

A `---@luabox-ignore` suppression comment is missing its rule id, its
mandatory reason, or both (SPEC.md §9). Every suppression must say *which*
rule it silences and *why* — the reason is not optional, so a bare tag is
itself a (correctness-tier) diagnostic.

```lua
-- wrong: no reason
---@luabox-ignore global-write
counter = 0

-- right: rule id + reason
---@luabox-ignore global-write intentional module-level singleton
counter = 0
```

The comment attaches to the statement on the same line or the line below;
placed before the first statement it suppresses the rule file-wide.
";

const LB0501: &str = "\
# LB0501: unused local (unused-local)

A `local` (or `local function`) is declared but never read. Style tier. Its
value is computed and discarded, which is usually a leftover or a typo.

```lua
local total = compute()   -- LB0501 if `total` is never used
```

Prefix the name with `_` to mark it deliberately unused, or delete the
binding. `luabox lint --fix` renames a never-referenced local to `_name`
(machine-applicable). Resolution is HIR-based: a use inside a nested closure
still counts, so genuine captures are never flagged.

**`---@meta` definition files are exempt.** A file where a `---@meta` tag
appears before any statement (SPEC.md §3, ticket #76) never gets
`unused-local`: a defs file's locals are often structural scaffolding (e.g.
building up a class table before assigning it to a global) rather than
leftovers.
";

const LB0502: &str = "\
# LB0502: unused parameter (unused-param)

A function parameter is never read. Pedantic tier (off unless enabled),
because unused parameters are often required by an interface or callback
shape. The implicit `self` of a `:` method is always exempt.

```lua
---@param event table
---@param _unused number
local function handler(event, _unused) end   -- `_unused` is exempt
```

Prefix the name with `_` to silence, or drop the parameter if the signature
is yours to change.
";

const LB0503: &str = "\
# LB0503: shadowed local (shadowed-local)

A new `local` shadows a still-live binding of the same name in an
*enclosing* scope. Suspicious tier: the outer binding is now unreachable for
the rest of the inner scope, which is a common source of bugs.

```lua
local value = 1
do
  local value = 2   -- LB0503: shadows the outer `value`
  print(value)
end
```

Re-declaring a local in the *same* block (`local x = 1; local x = f(x)`) is
idiomatic Lua and is **not** flagged — only shadowing across a scope
boundary is. Rename one of the two, or reuse the outer binding.
";

const LB0504: &str = "\
# LB0504: assignment to a global (global-write)

An assignment targets a name that resolves to a global (no `local` is in
scope). Suspicious tier: a missing `local` is the classic Lua footgun — the
value silently leaks into `_G`.

```lua
local function reset()
  counter = 0   -- LB0504: writes the global `counter`; missing `local`?
end
```

Add `local`, or — if the global is intentional (a runtime injects it) — add
its name to `[lint] globals` in `luabox.toml`:

```toml
[lint]
globals = [\"vim\", \"love\"]
```

Only bare-name targets are flagged; `t.field = v` and `t[k] = v` are field
writes, not global writes.

**`---@meta` definition files are exempt.** A file where a `---@meta` tag
appears before any statement (SPEC.md §3) is a pure definition surface —
declaring its exported globals (`love = {}` and the like) is the file's
entire purpose, so `global-write` never fires there and no `[lint] globals`
entry is needed for it:

```lua
---@meta
love = {}   -- fine: this file is a `---@meta` defs module
```
";

const LB0505: &str = "\
# LB0505: explicit nil comparison as truthiness (explicit-nil-compare-truthiness)

An `if` condition is exactly `x ~= nil` (or `x == nil`) where plain
truthiness — `if x then` / `if not x then` — is provably equivalent. Style
tier, and **type-informed**: it fires only when `x`'s type cannot be
`boolean` and cannot contain `false`, so the rewrite can never change
behaviour. When the type is unknown the rule stays silent.

```lua
---@type string
local name = get()
if name ~= nil then end   -- LB0505 -> `if name then`

---@type boolean
local flag = get()
if flag ~= nil then end   -- NOT flagged: `flag` may be `false`
```

`luabox lint --fix` applies the rewrite in both directions.
";

const LB0506: &str = "\
# LB0506: string concatenation in a loop (concat-in-loop)

A loop-carried accumulator is grown with `s = s .. expr` on every iteration.
Perf tier: each `..` allocates a fresh string, so the loop is quadratic in
the total length.

```lua
local out = \"\"
for _, line in ipairs(lines) do
  out = out .. line   -- LB0506
end
```

Collect the pieces in a table and join once:

```lua
local parts = {}
for _, line in ipairs(lines) do
  parts[#parts + 1] = line
end
local out = table.concat(parts)
```

No autofix — the correct rewrite depends on the surrounding code.
";

const LB0507: &str = "\
# LB0507: `pairs` on an array (pairs-on-array)

`pairs(t)` is used where `t`'s inferred or declared shape is an array
(`T[]` / `table<integer, V>`, or a positional-only table literal). Perf
tier: `ipairs` (or a numeric `for`) iterates the array part in order without
hashing, and `pairs` gives no ordering guarantee.

```lua
---@param xs number[]
local function total(xs)
  for _, x in pairs(xs) do end   -- LB0507 -> ipairs(xs)
end
```

`luabox lint --fix` rewrites `pairs` to `ipairs` (machine-applicable). The
rule stays silent when the shape is unknown or has non-array keys.
";

const LB0508: &str = "\
# LB0508: empty `if ... then` body (empty-then)

An `if`/`elseif` branch has an empty, comment-free body. Suspicious tier:
either the body was forgotten, the condition is inverted, or the branch is
dead and should be removed.

```lua
if ready then end          -- LB0508
if ready then
  -- TODO: handle later    -- NOT flagged: a comment documents intent
end
```

Fill in the body, invert the condition and move the code, or delete the
branch. A single explanatory comment suppresses the lint.
";

const LB0509: &str = "\
# LB0509: read of an undefined global (undefined-global)

A name resolves to a global (no `local` is in scope) at a **read** position —
a call, an argument, the right-hand side of an expression — and that global
is not one of: the dialect's real stdlib, an ambient `[types] defs` package,
a name this same file itself assigns somewhere (self-defining), or an entry
in `[lint] globals`. Suspicious tier: this is luals' `undefined-global`
finding — usually a typo that silently reads `nil` instead of erroring, since
Lua has no \"unbound name\" error for globals.

```lua
prnit(\"hello\")   -- LB0509: read of undefined global `prnit`
                  -- (did you mean `print`?)
```

Three ways to fix it:

```lua
print(\"hello\")                 -- fix the typo

---@meta
-- defs/acme.d.lua
acme = {}                      -- or: declare it via a defs package
```

```toml
[lint]
globals = [\"acme\"]             -- or: tell the linter it's intentional
```

Assigning the name anywhere in the same file also clears the finding — a
file that does `foo = 1` and later reads `foo` is self-defining; the
assignment itself is `global-write`'s business (LB0504), not this rule's.

**`---@meta` definition files are exempt** (declaring ambient globals is
such a file's entire purpose, same as `global-write` — ticket #76).

**Suppression:** `---@luabox-ignore undefined-global <reason>`, or the
LuaCATS directive luals itself recognises:

```lua
---@diagnostic disable: undefined-global
prnit(\"hello\")   -- not flagged

---@diagnostic disable-next-line: undefined-global
prnit(\"hello\")   -- not flagged, this line only
```
";

const LB0601: &str = "\
# LB0601: irreducible `goto`

`luabox build` lowers `goto`/labels to Lua 5.1 by restructuring the two
reducible shapes (SPEC.md §2.1):

- **backward goto as loop** — a label followed later *in the same block*
  by its single goto, either `if <cond> then goto L end` (becomes
  `repeat … until not (<cond>)`) or an unconditional `goto L` as the
  block's last statement (becomes `while true do … end`);
- **forward goto as skip** — goto(s) before their label in the same block
  (each becomes a flag; the skipped statements are wrapped in
  `if not <flag> then … end`). The `goto continue` idiom is this shape.

A goto that fits neither shape — nested deeper than one `if` branch,
jumping out of a loop, mixing backward and forward jumps to one label,
interleaving regions, or crossing a `break` boundary — cannot be
restructured faithfully and is reported here.

```lua
-- irreducible: jumps out of the loop
while true do
  goto out
end
::out::

-- restructure by hand, e.g.:
while true do
  break
end
```

Restructure the control flow by hand (usually a `break`, a flag, or a
function extraction). Note `---@luabox-allow lossy-lowering` does **not**
apply to goto: the escape hatch exists for bounded fidelity trade-offs
(see LB0603), not for control flow, where any deviation changes what the
program does.
";

const LB0602: &str = "\
# LB0602: assignment to a `<const>` variable

`<const>` (Lua 5.4) is a compile-time reassignment ban. When `luabox
build` lowers it away for a pre-5.4 target, the ban is enforced by the
compiler instead — exactly as Lua 5.4 itself would reject the program —
so dropping the attribute never changes behaviour.

```lua
local limit <const> = 10
limit = 20            -- LB0602, just as Lua 5.4 errors here

local f = function()
  limit = 30          -- LB0602 too: 5.4 also bans upvalue assignment
end
```

Shadowing is respected: a new `local limit` afterwards is a different
variable and may be assigned freely.

Fix the assignment or drop the `<const>` attribute if the variable is
genuinely meant to be mutable. (`<close>` variables are also constant and
get the same check.)
";

const LB0603: &str = "\
# LB0603: `<close>` lowering fidelity

`local h <close> = v` (Lua 5.4) calls `getmetatable(v).__close(v, err)`
when the variable's scope exits, normally or via an error. Lowering to
pre-5.4 targets rewrites the scope tail through the runtime helper:

```lua
local h = open()
__luabox_rt.close_scope(h, function()
  -- original scope tail
end)
```

`close_scope` runs the tail under `pcall`, invokes `__close(v, err)` with
the error object (or `nil` on the normal path), then re-raises the error
unmodified.

**Warn tier (this code, suppressible).** One fidelity delta is not
lowerable at all (SPEC.md §2.1): if a coroutine suspended inside the
scope is discarded, Lua 5.4 still closes the variable when the coroutine
is collected or closed; the `pcall` wrapper never resumes, so the close
action never runs. Annotate the declaration to acknowledge the trade-off:

```lua
---@luabox-allow lossy-lowering
local h <close> = open()
```

**Error tier (this code, not suppressible).** The scope tail becomes a
function body, so a tail containing `return`, a `break` bound to an outer
loop, a `goto` out of the scope, or `...` cannot be wrapped — those
constructs cannot cross a function boundary. Restructure the scope (e.g.
compute the return value before the `<close>` declaration, or narrow the
variable's scope with a `do … end` block).
";

const LB0604: &str = "\
# LB0604: `_ENV` use not lowerable

Explicit `_ENV` (Lua 5.2+) lowers to 5.1/LuaJIT via `setfenv`/`getfenv`
for the common idioms (SPEC.md §2.1):

```lua
local _ENV = t        -- chunk/function-body level → setfenv(1, t)
_ENV = t              -- same position               → setfenv(1, t)
print(_ENV)           -- expression read             → getfenv(1)
```

Everything else is reported here: `_ENV` in a multi-name `local` or
multi-target assignment, `local _ENV` inside a nested block (its 5.2
scope would end at the block's `end`, but `setfenv` affects the whole
function — not equivalent), or an `_ENV` function parameter.

Restructure to one of the lowerable idioms, or keep an explicit table
(`local env = t; env.x = 1`) instead of environment manipulation.
";

const LB0605: &str = "\
# LB0605: LuaJIT extension not lowerable

Lowering LuaJIT code to Lua 5.1 polyfills the `bit.*` library through the
injected `__luabox_rt` module (SPEC.md §2.1): `bit.band`, `bor`, `bxor`,
`bnot`, `lshift`, `rshift`, `arshift`, `rol`, `ror`, `bswap`, `tobit`,
and `tohex` are all covered, including `require(\"bit\")` aliases.

This error reports the extensions with no faithful polyfill:

- **`ffi`** — `require(\"ffi\")`: C data, C calls, and cdata semantics
  cannot be reproduced in plain Lua. Not lowerable, by design.
- **unknown `bit` members** — anything outside the documented `bit` API.
- **64-bit/imaginary number literals** — `42LL`, `7ULL`, `3i` create
  cdata boxes; a double cannot represent them.

Keep such code behind a runtime check and provide a plain-Lua fallback,
or target LuaJIT itself.
";

const LB0606: &str = "\
# LB0606: integer/float divergence on lowering

Lua 5.3+ has true 64-bit integers; 5.1, 5.2, and LuaJIT represent every
number as a double (exact only up to 2^53) and their bit shims are
32-bit. Lowering cannot bridge that representation gap, so `luabox build`
warns on the surfaces where the divergence is observable (SPEC.md §2.1
diagnostic tiers — warn on observable divergence):

- **Integer literals beyond 2^53** — the constant itself is already
  inexact on the target.
- **`//` lowering** (reported once per file) — `math.floor(a / b)` equals
  integer floor division up to 2^53; beyond it the double division can
  round before flooring, and integer division by zero raises in 5.3 but
  yields `inf`/`nan` in doubles.
- **`string.format` with `%d`** — 5.3+ rejects non-integral arguments
  and prints full 64-bit integers; double-only targets coerce and lose
  precision.
- Bitwise operands wider than 32 bits truncate in every available shim
  (`bit32`, `bit`, and the pure-Lua fallback are all 32-bit).

If your values stay within 32 bits (bitops) / 2^53 (arithmetic) — the
overwhelmingly common case for code that targets 5.1 at all — the lowered
program is exact and the warning can be ignored. Otherwise restructure to
avoid the construct or raise the target.
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
typed paradigm with its own owner and toolchain; luabox's typed story is
strict LuaCATS checking over untyped Lua, so `luau` is not a valid edition.
";

const LB1002: &str = "\
# LB1002: unresolvable definition package

`[types] defs = [...]` in `luabox.toml` names ambient definition packages —
`---@meta` `.d.lua` files that declare types for an environment your code
runs in (a game engine, an editor API, a framework).

An entry could not be resolved. Today definition packages are project-local:
an entry `\"love2d\"` must exist as either

- `defs/love2d.d.lua`, or
- `defs/love2d/` containing one or more `.d.lua` files,

relative to the project root. Registry-distributed definition packages
arrive with the package manager (SPEC.md §3, §6).

Fix: create the defs file/directory, correct the name, or remove the entry.
";

const LB1100: &str = "\
# LB1100: known security advisory affects a locked dependency

`luabox audit` (SPEC.md §6, §14) matched a package pinned in `luabox.lock`
against a version range a loaded advisory marks as affected — and not
excluded by a `patched` range, and not withdrawn.

Advisories come from a local, directory-of-TOML-files database (RUSTSEC-
analog; no hosted feed exists yet): `LUABOX_ADVISORY_DB`, or
`~/.luabox/advisory-db` if unset. When neither location exists, `luabox
audit` prints a note and exits `0` — a security check must never fail a
build merely because no database was ever configured; once a database *is*
present, findings are judged normally.

Severity maps to how loud this diagnostic is:

- `critical` / `high` → error (nonzero exit).
- `medium` / `low` → warning (does not fail the command by itself).

```
$ luabox audit
error[LB1100]: LBSEC-2026-0001 insecure-pkg 1.0.0: remote code execution (high)
note: insecure-pkg evaluates untrusted input passed to run()
note: more info: https://example.com/advisories/LBSEC-2026-0001
audit: 1 advisory loaded, 1 finding (1 error, 0 warnings) against 1 locked package(s)
```

Upgrade the dependency to a patched version (`luabox update <pkg>`), pin an
alternative, or — if the advisory genuinely does not apply to how the
package is used — accept the risk consciously; there is no in-manifest
suppression for this code yet (SPEC.md §19).
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_codes_are_all_present() {
        for raw in [
            "LB0001", "LB0010", "LB0011", "LB0012", "LB0013", "LB0014", "LB0015", "LB0016",
            "LB0300", "LB0301", "LB0302", "LB0303", "LB0304", "LB0305", "LB0306", "LB0307",
            "LB0500", "LB0501", "LB0502", "LB0503", "LB0504", "LB0505", "LB0506", "LB0507",
            "LB0508", "LB0509", "LB1001", "LB1100",
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
