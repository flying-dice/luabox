# Building

What the compiler produces, how it targets older Lua, and how bundles map
back to your source. Section numbers (§) refer to the
[specification](05-specification.md).

## Projects

A project names three things:

- the **source root**, which module names are relative to —
  `src/net/http.luac` is the module `net.http`;
- the **header directories**, searched by `#include` (§8.2);
- the **target**: Lua 5.1, 5.2, 5.3, 5.4 or LuaJIT.

## Compiling

Each `.luac` file compiles to a `.lua` file at the same path under the output
directory. Headers produce nothing, and plain `.lua` files in the source
root are copied unchanged. A program with any error produces no output;
there is no way to compile a program that doesn't check.

The compiler removes types by overwriting them with spaces (§11.1):

```lua
local function parse(s: string): (number, string)   -- source
local function parse(s        )                    -- output
```

Removed: `: Type` after names, return types, casts,
whole `typedef` statements and `#include` lines. Newlines inside a removed
region stay. The output has exactly the source's lines, and every token that
remains is at the same line and column — so a runtime error message, a
stack trace or a debugger breakpoint lines up with the `.luac` source
without a map.

Nothing is added: the output has no runtime library, no type checks and no
wrappers. It runs at the speed of the Lua you would have written by hand,
because it is that Lua.

## Targeting older Lua

You may write source using the syntax of the target or of any newer
version. When the source uses something the target lacks — `goto`, integer
division `//`, bitwise operators, `<const>` locals — the compiler rewrites it
into an equivalent the target has (§11.2). Rewrites keep lines where they
can. When a rewrite has to change the line structure, the source map
records where each output line came from.

Library headers follow the target: `<utf8.luah>` exists only for 5.3 and
later, and LuaJIT's `<bit.luah>`, `<ffi.luah>` and `<jit.luah>` only for
LuaJIT. Including a header the target lacks is an error.

## Bundling

A bundle packs a program and the modules it requires into one `.lua` file
(§11.3). Each module's compiled text goes into the bundle unchanged, inside
a function registered under the module's name, and `require` inside the
bundle finds registered modules first.

- Only `require` calls with a string literal are followed.
- Modules that have a header but no source in the project — plain Lua or
  native modules installed elsewhere — are left to the runtime's own
  `require`.
- Headers are not bundled.

## Source maps

An unbundled file needs no map: line *n* of the output is line *n* of the
source. A bundle comes with `<bundle>.map`, which records where each bundle
line came from (§11.4):

```json
{
  "version": 1,
  "bundle": "app.lua",
  "files": ["src/main.luac", "src/util.luac"],
  "lines": [null, [0, 1], [0, 2], [1, 1]]
}
```

`lines[i]` describes bundle line `i + 1`: `[file index, source line]`, or
`null` for a line the bundler added. Within a mapped line, columns are the
source's columns. A tool reads a runtime traceback such as
`app.lua:3: attempt to index a nil value`, looks up line 3, and reports
`src/main.luac:2` instead.
