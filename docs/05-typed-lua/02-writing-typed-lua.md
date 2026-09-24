# Writing Typed Lua

The everyday idioms, each with the rule behind it. Section numbers (§)
refer to the [specification](05-specification.md). Snippets leave out the
`#include` lines for the standard libraries they use (`<string.luah>`,
`<math.luah>`, …); a real file needs them.

## Variables

Write the type after the name, or let the value supply it, as C's `auto`
does:

```lua
local count: integer = 0
local name: string = "ada"
local ratio = 0.5          -- number, from the literal
local label = name         -- string, from `name`
```

A type is taken once, at the declaration, and fixed from then on (§6.2).
Nothing looks ahead to how the variable is used:

```lua
local count = 0
count = count + 1          -- fine
count = "many"             -- error: `string` is not compatible with `integer`
```

Any variable may hold `nil`, as in Lua. A variable declared without a value
starts as `nil`: `local label: string`. Leaving out both the type and the
value is an error — there is nothing to take a type from.

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
local function parse(s: string): (number, string)
  local n: number = tonumber(s)
  if n then return n, nil end
  return nil, "not a number: " .. s
end

local value, err = parse("42")   -- number, string
```

**Leaving out arguments** works as in Lua: missing trailing arguments are
`nil`. Passing more arguments than a function takes is an error.

```lua
local function pad(s: string, width: integer): string
  return string.rep(" ", (width or 8) - #s) .. s
end

pad("x")        -- width is nil
pad("x", 4)
```

**Varargs** are untyped, as in C. Each value from `...` is `any`, so give it
a type before using it:

```lua
local function sum(...): number
  local total: number = 0
  for _, n: number in ipairs({ ... }) do total = total + n end
  return total
end
```

**Function types** are written `(params) -> result`:

```lua
typedef Compare = (a: string, b: string) -> boolean
local on_done: (ok: boolean) -> void
```

**Callbacks** are functions like any other, so they write their types:

```lua
local names: string[] = { "lua", "c" }
table.sort(names, function(a: string, b: string): boolean return a < b end)
```

A function that declares results must return on every path. Ending in
`error(...)` counts, because `error` never returns.

## Loops

A numeric `for` takes its type from its start: `for i = 1, 10` makes `i` an
`integer`. A generic `for` takes its types from the iterator. `ipairs` and
`pairs` return `any`, so declare the variables you use (an unused `_` can
stay `any`):

```lua
for i: integer, name: string in ipairs(names) do
  print(i, name)
end
```

## Tables

Name a record with `typedef`, as you would a C struct:

```lua
typedef Item = { name: string, price: number, note: string }

local apple: Item = { name = "apple", price = 0.5 }   -- note is nil
```

A table written where a type is expected is checked against it: every
field it sets must be declared, at a compatible type. Fields it leaves out
are `nil`, like the members a C initializer leaves out.

Records match by name. Two records with the same fields are different types
unless they come from the same `typedef`, so give a record a name when it is
passed between functions.

**Arrays and maps** are written with suffixes:

```lua
local names: string[] = { "ada", "grace" }        -- array
local ages: integer[string] = { ada = 36 }         -- map: string → integer
local seen: boolean[string] = {}                   -- a set

names[#names + 1] = "linus"      -- arrays and maps take new entries
local age: integer = ages["bob"] -- nil if absent
```

**Records do not grow.** A record's fields are fixed by its type; assigning
a field it doesn't declare is an error (§6.5). Build tables whole:

```lua
-- Not this:
local M = {}
M.version = "1.0"          -- error: `{}` has no field `version`

-- This:
return { version = "1.0", area = area }
```

A record that also takes arbitrary keys says so with an index part:
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

The constructor passed to `setmetatable` needs only the fields `__index`
does not supply (§6.7). Operators are not overloadable: write `add(a, b)`,
not `a + b`, even if the metatable has `__add`.

When functions need each other before they are all defined, declare one
first and define it later, as with a C prototype:

```lua
local new: (x: number, y: number) -> Vec       -- nil until defined below

local function scale(self: Vec, k: number): Vec
  return new(self.x * k, self.y * k)
end

function new(x: number, y: number): Vec
  return setmetatable({ x = x, y = y }, { __index = { scale = scale } })
end
```

## Kinds of record

Data that comes in several kinds is one record with a tag field, the way C
code does it. Each kind uses the fields it needs; the others stay `nil`:

```lua
typedef Shape = { kind: string, r: number, w: number, h: number }

local function area(s: Shape): number
  if s.kind == "circle" then
    return math.pi * s.r ^ 2
  end
  return s.w * s.h
end

local shapes: Shape[] = {
  { kind = "circle", r = 1 },
  { kind = "rect", w = 3, h = 4 },
}
```

## `any` and casts

`any` is a value whose type isn't known, like `void *` in C. It comes from
`...`, `pcall`, `dofile`, a computed `require`, and code written to work on
any type. An `any` converts to a declared type without ceremony, but
nothing can be done with it until it has one:

```lua
local config: Config = dofile("config.lua")   -- fine: converted
print(config.host)
print(dofile("config.lua").host)              -- error: field of `any`
```

A cast `<T> e` gives an expression a type in place. As in C, it is not
checked: it is where you vouch for the value.

```lua
print((<Config> dofile("config.lua")).host)
```

Code that works on any type takes and returns `any`, and callers convert:

```lua
local function first(items: any[]): any
  return items[1]
end

local s: string = first(names)
```

## Errors

`error(...)` never returns, so it ends a path like `return` does.
`pcall(f, ...)` returns a `boolean` and then `any` values:

```lua
local ok, result = pcall(parse_config, text)
if ok then
  local config: Config = result
end
```

The Lua convention of returning `nil, message` on failure types as
`(T, string)`.

## Porting a Lua file

1. Rename it `.luac` and write a header for it if other modules require it.
2. Add types to every function's parameters and results, and to generic
   `for` variables.
3. Replace build-up tables (`local M = {}` then `M.f = …`) with one
   constructor.
4. Give records that are passed around a `typedef`.
5. Add `#include` lines for the libraries it uses and the modules it
   requires.
6. Rename anything called `typedef`.
7. Compile, and fix what it reports.
