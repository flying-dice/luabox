# renderer

Depends on `../geometry` across a **package boundary** and conforms to its
`Drawable` type with its own carrier. An application (`edition = "5.1"`, so it
runs end-to-end on a stock Lua 5.1) that draws ASCII shapes to stdout.

```
renderer/
├── luabox.toml            # [dependencies] geometry = { path = "../geometry" }
├── luabox.lock             # committed — this is an app, not a library
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

## Cross-package LuaCATS typing — the library model (#108)

luabox shares LuaCATS types across a package boundary the way
lua-language-server shares a **`workspace.library`**: a dependency's own
`[types] defs` files join the consuming package's ambient scope automatically.
`../geometry`'s manifest publishes `[types] defs = ["geometry"]`, so simply
depending on `geometry` here makes every class it declares
(`geometry.Shape`, `geometry.Drawable`, `geometry.Point`, ...) referenceable
and checkable in renderer — no imports, no `require`-for-types, and **no
hand-vendored copy**. Class names are a single global namespace across the
workspace and its libraries, exactly as in luals; `geometry.Drawable` resolves
here the same way a local class would.

Resolution walks **direct** dependencies only, one level deep (a dependency's
*own* dependencies' defs do not transit — matching the manifest resolver's
precedent). If two packages in scope declared the same class name, luabox is
stricter than luals's silent merge: it reports `LB0307` (a warning) at the
losing declaration and the project-local declaration wins.

### What this does — and doesn't — carry across the boundary

- **Does:** a dependency's `---@class`/`---@alias`/`---@enum` become
  referenceable (`---@class render.Square : geometry.Drawable`,
  `---@type geometry.Point`), and a dependency's def-declared **global/module
  API** (e.g. `function geometry.point(x, y)` on a global table) is called
  with full param/return checking — the same way the stdlib and `love2d` defs
  already are.
- **Doesn't:** it does **not** type a `local geo = require("geometry")` module
  *return* value. Cross-file `require` resolution is a separate epic (#85);
  the value from `require` is still `unknown` to the checker. This example
  types the shared **declarations** (the interface renderer conforms to), not
  a required module handle.

> Historical note: this example used to hand-vendor a copy of geometry's
> classes in `defs/geometry.d.lua` with a "cross-package sharing doesn't work"
> stopgap comment. That file is gone — #108 makes the dependency's own defs
> reach renderer automatically.

## The carrier

`src/square.lua` declares:

```lua
---@class render.Square : geometry.Drawable
local Square = {}
```

`geometry.Drawable` (and its parent `geometry.Shape`) are ambient here through
the dependency, so this single annotation is checked: conformance to
`Drawable` (`area` + `perimeter` + `my_static` + `draw`) IS verified in
renderer, exactly as it is inside `../geometry`. This example implements all
four members; delete any one and `luabox check` reports `LB0300` at the
`---@class` line, e.g.:

```
error[LB0300]: `render.Square` does not satisfy `geometry.Drawable`: missing member `draw`
   --> src/square.lua:...:4
   |
   | ---@class render.Square : geometry.Drawable
   |    ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ expected member `draw` of type `fun(self: unknown): string`
```

```sh
luabox check        # 0 errors — cross-package conformance verified
luabox fmt --check
luabox lint
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

- `../geometry` declares its types in its own ambient `[types] defs` package.
  Those defs are its **published type surface**: any package that depends on
  geometry gets them ambiently, the luals workspace-library model.
- `renderer` addresses `../geometry`'s types directly by their qualified names
  — no copy, no re-declaration. `defs/render.d.lua` holds only renderer's own
  `render.Square`, declared as an extension of the dependency's
  `geometry.Drawable`.
