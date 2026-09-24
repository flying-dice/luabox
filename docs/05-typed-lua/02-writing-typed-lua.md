# Writing Typed Lua

The everyday idioms, each with the rule behind it. Section numbers (§)
refer to the [specification](05-specification.md). Snippets leave out the
`#include` lines for the standard libraries they use (`<string.luah>`,
`<math.luah>`, …); a real file needs them.

## Variables

Write the type after the name, or let the value supply it:

```lua
local count: integer = 0
local name: string = "ada"
local ratio = 0.5          -- number, from the literal
local label = name         -- string, from `name`
```

A type taken from a value is taken once, at the declaration, and is fixed
from then on (§6.2). Nothing looks ahead to how the variable is used:

```lua
local count = 0
count = count + 1          -- fine
count = "many"             -- error: `string` is not compatible with `integer`
```

A variable declared without a value starts as `nil`, so its type must say
so: `local label: string?`. Leaving out both the type and the value is an
error — there is nothing to take a type from.

`integer` is a whole number and fits anywhere a `number` does. `1` is an
`integer` literal, `1.0` and `1e3` are `number`s.

Attributes go before the type: `local limit <const>: number = 10`.

## Functions

Each parameter's type follows it, and the return type follows the
parameter list:

```lua
local function area(w: number, h: number): number
  return w * h
end

local function log(message: string)      -- returns nothing; `: void` optional
  print("[log] " .. message)
end
```

**Several results** are written as a list:

```lua
local function parse(s: string): (number?, string?)
  local n: number? = tonumber(s)
  if n then return n, nil end
  return nil, "not a number: " .. s
end

local value, err = parse("42")   -- number?, string?
```

**Optional arguments** are parameters whose type admits `nil`. A caller may
leave trailing ones out:

```lua
local function pad(s: string, width: integer?): string
  return string.rep(" ", (width or 8) - #s) .. s
end

pad("x")        -- fine
pad("x", 4)     -- fine
```

**Varargs** have a type too; `{ ... }` is then an array of it:

```lua
local function sum(...: number): number
  local total: number = 0
  for _, n in ipairs({ ... }) do total = total + n end
  return total
end
```

**Function types** are written `(params) -> result`:

```lua
local on_done: ((ok: boolean) -> void)? = nil
typedef Compare<T> = (a: T, b: T) -> boolean
```

**Callbacks** passed where a function type is expected take their types from
it, so they need no declarations:

```lua
local names: string[] = { "lua", "c" }
table.sort(names, function(a, b) return a < b end)   -- a, b are string
```

**Generic functions** name their type parameters after the function name.
The arguments decide what the parameters stand for at each call:

```lua
local function first<T>(items: T[]): T?
  return items[1]
end

local s: string? = first({ "a", "b" })   -- T is string
```

A function that declares results must return on every path. Ending in
`error(...)` counts, because `error` never returns.

## Tables

Name a table shape with `typedef`. Records list their fields:

```lua
typedef Item = { name: string, price: number, note: string? }

local apple: Item = { name = "apple", price = 0.5 }   -- `note` may be left out
```

A table written where a type is expected is checked against it exactly:
every required field present, no extra fields, nested tables checked too.

**Arrays and maps** are written with suffixes:

```lua
local names: string[] = { "ada", "grace" }        -- array
local ages: integer[string] = { ada = 36 }         -- map: string → integer
local seen: boolean[string] = {}                   -- a set

names[#names + 1] = "linus"        -- arrays and maps take new entries
local age: integer? = ages["bob"]  -- a map lookup may miss, so it is `integer?`
```

**Records do not grow.** A record's fields are fixed when it is built;
assigning a field it doesn't have is an error (§6.5). Build tables whole:

```lua
-- Not this:
local M = {}
M.version = "1.0"          -- error: `{}` has no field `version`

-- This:
return { version = "1.0", area = area }
```

Assigning `nil` to a field is allowed only if the field's type admits it. A
record that also takes arbitrary keys says so with an index part:
`{ name: string, [string]: any }`.

## Objects

There are no classes. An object is a table whose metatable's `__index`
supplies its methods, exactly as in plain Lua. Declare the instance type
with its methods, write the methods as local functions, and build the
metatable in one constructor:

```lua
typedef Vec = { x: number, y: number, len: (self: Vec) -> number }

local function len(self: Vec): number
  return math.sqrt(self.x * self.x + self.y * self.y)
end

local Meta = { __index = { len = len } }

local function new(x: number, y: number): Vec
  return setmetatable({ x = x, y = y }, Meta)
end

print(new(3, 4):len())   -- 5
```

`setmetatable` adds the fields of `__index` to the table's type, which is
why `{ x = x, y = y }` becomes a `Vec` (§6.7). Operators are not
overloadable: write `add(a, b)`, not `a + b`, even if the metatable has
`__add`.

When functions need each other before they are all defined, forward-declare
the one used early (§6.3):

```lua
typedef Vec = { x: number, y: number, scale: (self: Vec, k: number) -> Vec }

local new: (x: number, y: number) -> Vec     -- declared, defined below

local function scale(self: Vec, k: number): Vec
  return new(self.x * k, self.y * k)
end

function new(x: number, y: number): Vec
  return setmetatable({ x = x, y = y }, { __index = { scale = scale } })
end
```

## Unions and narrowing

`A | B` is either type, and `T?` is `T | nil`. Before using a union as one
of its members, check which one it is; the check narrows the type inside the
branch (§6.9):

```lua
local function describe(v: string | number): string
  if type(v) == "number" then
    return "number " .. v        -- v is number
  end
  return "string " .. v          -- v is string
end
```

A `nil` check is the most common narrowing, and an early return narrows the
rest of the function:

```lua
local function upper_name(item: Item?): string
  if not item then
    return "(none)"
  end
  return string.upper(item.name) -- item is Item from here on
end
```

**Tagged unions** are records with a literal `kind` field. Comparing it
narrows to the matching member:

```lua
typedef Circle = { kind: "circle", r: number }
typedef Rect = { kind: "rect", w: number, h: number }
typedef Shape = Circle | Rect

local function area(s: Shape): number
  if s.kind == "circle" then
    return math.pi * s.r ^ 2
  end
  return s.w * s.h
end
```

Narrowing applies to locals and parameters. To narrow a field, copy it into
a local first:

```lua
local note = item.note
if note then
  print(string.upper(note))
end
```

## Errors

`error(...)` never returns, so it ends a path like `return` does.
`assert(x, "message")` narrows `x` for the statements after it.
`pcall(f, ...)` returns `boolean` and then `any` results; cast them once
you know what they are.

The Lua convention of returning `nil, message` on failure types naturally as
`(T?, string?)`.

## Escape hatches

- `any` accepts and allows everything. Use it where types genuinely don't
  apply; it is visible in the source, never implied.
- `unknown` accepts everything but allows nothing until you narrow or cast
  it. Prefer it to `any` for values you will check.
- `<T> e` casts `e` to `T`. It is checked: casting between unrelated types
  is an error. Casting from `any` or `unknown` is always allowed.

```lua
local config = <{ host: string, port: integer }> decode(text)  -- decode returns unknown
```

## Porting a Lua file

1. Rename it `.luac` and write a header for it if other modules require it.
2. Add types to every function's parameters and results.
3. Replace build-up tables (`local M = {}` then `M.f = …`) with one
   constructor.
4. Add `#include` lines for the libraries it uses and the modules it
   requires.
5. Rename anything called `typedef`.
6. Compile, and fix what it reports.
