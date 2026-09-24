# Writing Typed Lua

The everyday idioms, each with the rule behind it. Section numbers (§)
refer to the [specification](05-specification.md). Snippets leave out the
`#include` lines for the standard libraries they use (`<string.luah>`,
`<math.luah>`, …); a real file needs them.

## Variables

Write the type before the name, or let the value supply it:

```lua
local integer count = 0
local string name = "ada"
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
so: `local string? label`. Leaving out both the type and the value is an
error — there is nothing to take a type from.

`integer` is a whole number and fits anywhere a `number` does. `1` is an
`integer` literal, `1.0` and `1e3` are `number`s.

## Functions

The return type goes between `function` and the name; each parameter's type
goes before it:

```lua
local function number area(number w, number h)
  return w * h
end

local function void log(string message)   -- `void` may be left out
  print("[log] " .. message)
end
```

**Several results** are written as a list:

```lua
local function (number?, string?) parse(string s)
  local number? n = tonumber(s)
  if n then return n, nil end
  return nil, "not a number: " .. s
end

local value, err = parse("42")   -- number?, string?
```

**Optional arguments** are parameters whose type admits `nil`. A caller may
leave trailing ones out:

```lua
local function string pad(string s, integer? width)
  return string.rep(" ", (width or 8) - #s) .. s
end

pad("x")        -- fine
pad("x", 4)     -- fine
```

**Varargs** have a type too; `{ ... }` is then an array of it:

```lua
local function number sum(number ...)
  local number total = 0
  for _, n in ipairs({ ... }) do total = total + n end
  return total
end
```

**Callbacks** passed where a function type is expected take their types from
it, so they need no declarations:

```lua
#include <table.luah>

local string[] names = { "lua", "c" }
table.sort(names, function(a, b) return a < b end)   -- a, b are string
```

**Generic functions** name their type parameters after the function name.
The arguments decide what the parameters stand for at each call:

```lua
local function T? first<T>(T[] items)
  return items[1]
end

local string? s = first({ "a", "b" })   -- T is string
```

A function that declares results must return on every path. Ending in
`error(...)` counts, because `error` never returns.

## Tables

Name a table shape with `typedef`. Records list their fields, type first:

```lua
typedef { string name, number price, string? note } Item

local Item apple = { name = "apple", price = 0.5 }   -- `note` may be left out
```

A table written where a type is expected is checked against it exactly:
every required field present, no extra fields, nested tables checked too.

**Arrays and maps** are written with suffixes:

```lua
local string[] names = { "ada", "grace" }           -- array
local integer[string] ages = { ada = 36 }            -- map: string → integer
local boolean[string] seen = {}                      -- a set

names[#names + 1] = "linus"      -- arrays and maps take new entries
local integer? age = ages["bob"] -- a map lookup may miss, so it is `integer?`
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

Assigning `nil` to a field is allowed only if the field's type admits it.

## Objects

There are no classes. An object is a table whose metatable's `__index`
supplies its methods, exactly as in plain Lua. Declare the instance type
with its methods, write the methods as local functions, and build the
metatable in one constructor:

```lua
typedef { number x, number y, number(Vec self) len } Vec

local function number len(Vec self)
  return math.sqrt(self.x * self.x + self.y * self.y)
end

local Meta = { __index = { len = len } }

local function Vec new(number x, number y)
  return setmetatable({ x = x, y = y }, Meta)
end

print(new(3, 4):len())   -- 5
```

`setmetatable` adds the fields of `__index` to the table's type, which is
why `{ x = x, y = y }` becomes a `Vec` (§6.7). Operators are not
overloadable: write `add(a, b)`, not `a + b`, even if the metatable has
`__add`.

When methods need each other before they are all defined, forward-declare
the one used early (§6.3):

```lua
typedef { number x, number y, Vec(Vec self, number k) scale } Vec

local Vec(number x, number y) new          -- declared, defined below

local function Vec scale(Vec self, number k)
  return new(self.x * k, self.y * k)
end

function Vec new(number x, number y)
  return setmetatable({ x = x, y = y }, { __index = { scale = scale } })
end
```

## Unions and narrowing

`A | B` is either type, and `T?` is `T | nil`. Before using a union as one
of its members, check which one it is; the check narrows the type inside the
branch (§6.9):

```lua
local function string describe(string | number v)
  if type(v) == "number" then
    return "number " .. v        -- v is number
  end
  return "string " .. v          -- v is string
end
```

A `nil` check is the most common narrowing, and an early return narrows the
rest of the function:

```lua
local function string upper_name(Item? item)
  if not item then
    return "(none)"
  end
  return string.upper(item.name) -- item is Item from here on
end
```

**Tagged unions** are records with a literal `kind` field. Comparing it
narrows to the matching member:

```lua
typedef { "circle" kind, number r } Circle
typedef { "rect" kind, number w, number h } Rect
typedef Circle | Rect Shape

local function number area(Shape s)
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
local config = <{ string host, integer port }> decode(text)   -- decode returns unknown
```

## Porting a Lua file

1. Rename it `.luac` and write a header for it if other modules require it.
2. Add types to every function's parameters and results.
3. Replace build-up tables (`local M = {}` then `M.f = …`) with one
   constructor.
4. Add `#include` lines for the libraries it uses and the modules it
   requires.
5. Check for a `local` at the end of a line followed by a line that starts
   with a name; add `;` if they are meant to be two statements.
6. Compile, and fix what it reports.
