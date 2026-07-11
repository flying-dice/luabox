# renderer

Depends on `../geometry` across a **package boundary** and (attempts to)
conform to its `Drawable` type with its own carrier. An application
(`edition = "5.1"`, so it runs end-to-end on a stock Lua 5.1) that draws
ASCII shapes to stdout.

```
renderer/
├── luabox.toml            # [dependencies] geometry = { path = "../geometry" }
├── luabox.lock             # committed — this is an app, not a library
├── defs/geometry.d.lua     # VENDORED STOPGAP — see below
├── defs/render.d.lua       # our own render.Square : geometry.Drawable
├── src/square.lua          # carrier with top ---@class render.Square : geometry.Drawable
└── src/main.lua            # draws a square with `luabox run start`
```

## Install first

The dependency must be resolved before checking or running:

```sh
luabox install      # writes luabox.lock; the path dep is used in place
```

`luabox.lock` is committed here because renderer is an application.

## Cross-package LuaCATS typing — the key finding

This example used to be typed with `.luab` shape modules, where a
dependency's exported types are automatically ambient in a consuming
package (`[types] entry`/`shape-paths`, resolved via
`resolve_dep_shape_exports` — SHAPES-V2.md). **Plain LuaCATS has no
equivalent mechanism today.** This was verified directly against the real
binary while converting this example, not assumed:

- A `---@class` declared in a dependency's own `.lua` source is invisible in
  a consumer, under any name (qualified or not) — even after `luabox
  install` resolves the path dependency into `luabox.lock`.
  `---@type <name>` at the consumer reports `error[LB0305]: unknown type
  name` regardless.
- Calling a cross-package function *does* run at runtime (`require` still
  works — this is a type-checking gap, not a module-resolution one), but
  the checker does not carry its declared parameter/return types across the
  boundary: a call that would be flagged as a type mismatch **inside** the
  dependency's own package checks clean when made from a consumer, because
  the checker never resolved the callee's signature at all.
- `[types] defs` — the mechanism this project now uses for its own local
  types — is explicitly local: it resolves entries *only* from
  `<this project's root>/defs/`, never into a dependency's tree (confirmed
  by reading `resolve_project_defs` in `luabox-cli`, and by testing: pointing
  a consumer's `defs` list at a name that only exists in a dependency's
  `defs/` produces `error[LB1002]: cannot resolve definition package`).

**The stopgap used here:** `defs/geometry.d.lua` is a hand-duplicated,
explicitly-labeled copy of the two classes renderer needs
(`geometry.Shape`, `geometry.Drawable`) from `../geometry/defs/geometry.d.lua`.
It is **not** kept in sync automatically — a real project doing this would
need a manual process (or code generation) to detect drift. But it IS now
strictly required for `luabox check` to pass: as of #107 an `---@class X :
Y` extends-clause where `Y` doesn't resolve raises `LB0305` at the parent
reference, exactly like the stricter positions (`---@type`, `---@param`,
`---@field`) always did. Delete this vendored file (and its `defs` manifest
entry) and `src/square.lua`'s `: geometry.Drawable` reports:

```
error[LB0305]: unknown type name `geometry.Drawable` in annotation
   --> src/square.lua:16:27
   |
16 | ---@class render.Square : geometry.Drawable
   |                           ^^^^^^^^^^^^^^^^^ not a built-in, `---@class`, `---@alias`, or `---@enum` name
```

## What this means for the carrier

`src/square.lua` declares:

```lua
---@class render.Square : geometry.Drawable
local Square = {}
```

Because `geometry.Drawable`'s definition (vendored, not shared) extends
`geometry.Shape`, this single annotation is checked — and as of #107 the
conformance to `Drawable` (area + perimeter + my_static + draw) IS verified
here, exactly as it is inside `../geometry`. This example implements all
four members; delete any one and `luabox check` reports `LB0300` at the
`---@class` line (`missing member \`draw\``, etc.) — see
`../geometry/README.md` for the exact error format.

```sh
luabox check        # 0 errors — conformance and extends-clauses verified
luabox fmt --check
luabox lint
luabox test
luabox run start    # → draws a 4x4 square of '#'
```

Expected output of `luabox run start`:

```
A 4x4 square:
####
####
####
####
area = 16, perimeter = 16
```

## Contrast with geometry

- `../geometry` declares its types in its own ambient `[types] defs`
  package — nothing is "exported" to dependents in the `.luab` sense.
- `renderer` cannot address `../geometry`'s types directly; it vendors a
  hand-synced copy instead. This is the honest cost of dropping `.luab`'s
  cross-package shape export for plain LuaCATS today. If/when the `.luab`
  drop epic lands equivalent cross-package sharing for LuaCATS, this
  vendored file should be deleted in favor of it.
