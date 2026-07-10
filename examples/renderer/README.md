# renderer

Consumes the `../geometry` library across a **package boundary** and
conforms to geometry's `Drawable` type with its own carrier. An application
(`edition = "5.1"`, so it runs end-to-end on a stock Lua 5.1) that draws
ASCII shapes to stdout.

```
renderer/
├── luabox.toml           # [dependencies] geometry = { path = "../geometry" }
├── luabox.lock           # committed — this is an app, not a library
├── shapes/render.luab    # our own `type Square`
├── src/square.lua        # carrier with top `---@type geometry.Drawable`
└── src/main.lua          # draws a square with `luabox run start`
```

## Install first

The dependency must be resolved before checking or running:

```sh
luabox install      # writes luabox.lock; the path dep is used in place
```

`luabox.lock` is committed here because renderer is an application. (Libraries
like `../geometry` don't commit a lockfile.)

## Crossing the package boundary

geometry's manifest names a type entrypoint (`[types] entry =
"shapes/geometry.luab"`). Its `export type` declarations mount here under the
package name — `geometry.Shape`, `geometry.Drawable` — with **no import**:
the scope is ambient (SHAPES-V2.md). We declare our own `type Square` in
`shapes/render.luab` and put the conformance on the carrier declaration in
`src/square.lua`:

```lua
---@type geometry.Drawable
local Square = {}
```

Because `Drawable = Shape & { draw(self): string }` is an intersection, that
single annotation verifies the whole accumulated carrier — **area + perimeter
+ draw** (plus `my_static`), across every method added anywhere in the file.
Drop any one and `luabox check` reports the gap at the annotation, naming the
member.
The type declares `side: integer`, so `string.rep("#", self.side)` in `draw`
typechecks against the stdlib's `integer` count parameter, and the
constructor's declared `---@return render.Square` is satisfied by its
`setmetatable({ side = side }, Square)` result.

```sh
luabox check        # 0 errors — cross-package types resolve and seal
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

- `../geometry` **declares and exports** the types (via `[types] entry`).
- `renderer` **consumes** them by fully-qualified name and adds a new
  conformer (`Square`) — structural conformance working across a
  dependency edge.
