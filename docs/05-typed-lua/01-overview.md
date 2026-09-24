# Typed Lua

Status: **proposed**. This section describes a language that does not have a
compiler yet. It exists to be reviewed before the work starts.

Typed Lua is Lua with type declarations. You write `.luac` files, the
compiler checks them, removes the types, and hands you the `.lua` that runs.
Nothing is added at runtime. Modules are organised the way C organises
them: each has a header, and code that uses it includes the header.

```lua
#include <string.luah>

local function greet(name: string, times: integer): string
  return string.rep("hello " .. name .. "! ", times)
end

local names: string[] = { "ada", "grace", "linus" }
for i: integer, name: string in ipairs(names) do
  print(i, greet(name, 2))
end
```

compiles to:

```lua


local function greet(name        , times         )
  return string.rep("hello " .. name .. "! ", times)
end

local names           = { "ada", "grace", "linus" }
for i         , name         in ipairs(names) do
  print(i, greet(name, 2))
end
```

The types are replaced by spaces, so every line and column of the output
matches the source. An error at runtime points at the line you wrote.

## The idea in five rules

1. **Types follow names.** `local x: number`,
   `local function area(r: Rect): number`,
   `typedef Point = { x: number, y: number }`. Every Lua program still
   parses exactly as it does in Lua.
2. **Types are written, not guessed.** A variable's type is what you wrote,
   or what the right-hand side already has — `local n = 1` is an `integer`.
   Nothing is ever worked out from how a value is used later.
3. **Modules have headers.** `geometry.luah` declares what `geometry` offers.
   Code that uses it writes `#include "geometry.luah"`, the same as C. Plain
   Lua libraries and native `.dll`/`.so` modules get headers too.
4. **Tables don't grow.** A table's shape is fixed by the constructor that
   builds it. You build a table whole, in one `{ … }`, which is also the
   fastest way to build one in Lua.
5. **Always strict.** Every error stops compilation. There is no lenient
   mode and no comment that silences an error. `any` is the one, visible,
   escape hatch.

## What it leaves out

No classes, interfaces, enums, inheritance, access modifiers, overloading,
operator overloading, decorators, namespaces, macros or conditional
compilation. Lua already has tables, metatables, closures and modules;
Typed Lua types those and adds nothing that would need runtime support.

## File kinds

| File      | What it is                                                  |
|-----------|-------------------------------------------------------------|
| `.luac`   | Typed Lua source. Compiles to `.lua`.                       |
| `.luah`   | A header: a module's interface, and global declarations.    |
| `.lua`    | Plain Lua, used from Typed Lua through a header.            |

## Read next

- [Writing Typed Lua](02-writing-typed-lua.md) — the everyday idioms:
  objects, modules, unions, callbacks, optional arguments, errors.
- [Headers](03-headers.md) — interfaces for your modules, for plain Lua,
  for native libraries and for host globals.
- [Building](04-building.md) — compiling, targeting older Lua, bundling and
  source maps.
- [Specification](05-specification.md) — the normative definition.
- [Examples](../../examples/typed-lua/) — five programs with their compiled
  output, checked against the spec.

## Design choices a reviewer should weigh

These are the decisions most likely to be argued with. Each trades something
away on purpose.

- **Types after names, C for everything else.** `name: Type` puts every
  type where Lua allows nothing, so a parser never has to guess and plain
  Lua parses unchanged. It also matches how an editor shows a type it
  worked out (`local n = 1` displays as `local n: integer = 1`). Headers,
  `#include`, `extern` and `typedef` follow C. `typedef` becomes a reserved
  word. Casts are `<T> x`: C's `(T) x` clashes with Lua's `(f) "…"` and
  `(f) {…}` calls.
- **Every shared module needs a hand-written header.** That is extra typing
  for internal modules. In return, a module's interface is always one short
  file you can read, and changing an implementation cannot silently change
  what its users see.
- **Tables are fixed at construction.** The common Lua habit of
  `local M = {}` followed by `M.x = …` is an error. Modules end with
  `return { … }` instead. The rule is simple to check and to learn, and it
  steers code to the constructor form, which allocates once instead of
  rehashing as fields are added.
- **No operator overloading.** A vector type with an `__add` metamethod
  still cannot be added with `+`; you call `vec.add(a, b)`. Operators mean
  exactly one thing, and nothing about a type depends on metamethods other
  than `__index`.
- **Narrowing works on locals, not fields.** `if p.label then` does not
  narrow `p.label`; copy it into a local first. This keeps the rule sound
  without tracking which fields might change between the check and the use.
- **The standard library is included explicitly.** `#include <string.luah>`
  before using `string`. Only the base functions (`print`, `pairs`, `type`,
  `require` and so on) are always there.
