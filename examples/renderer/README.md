# renderer

Consumes the `../geometry` library across a **package boundary** and
implements geometry's `Drawable` trait for its own type. An application
(`edition = "5.1"`, so it runs end-to-end on a stock Lua 5.1) that draws
ASCII shapes to stdout.

```
renderer/
├── luabox.toml           # [dependencies] geometry = { path = "../geometry" }
├── luabox.lock           # committed — this is an app, not a library
├── shapes/render.luab      # `use geometry;` + our own `struct Square`
├── src/square.lua        # ---@impl Drawable for Square
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

geometry's manifest exports its shape module with `[types] shapes =
["geometry"]`. That makes tier-3 resolution work: our `shapes/render.luab` says
`use geometry;` and gets geometry's `Shape` and `Drawable` traits in scope,
even though they were declared in a different package. We then declare our own
`struct Square` and assert `impl Drawable for Square;`.

In `src/square.lua`, `---@impl Drawable for Square` binds the carrier. Because
`Drawable: Shape` (Shape is a supertrait), `luabox check` requires Square to
implement **area + perimeter + draw** — the full obligation across both
traits. Drop any one and check reports the gap.

```sh
luabox check        # 0 errors — cross-package shapes resolve and seal
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

- `../geometry` **declares and exports** the shapes.
- `renderer` **imports** them (`use geometry;`) and adds a new conformer
  (`Square`) — the trait/impl model working across a dependency edge.
