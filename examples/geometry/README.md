# geometry

The `.luab` shape DSL flagship. A library (`edition = "5.4"`) whose types live
in a **shape module** — Rust struct/trait syntax in a separate `.luab` file —
while the implementations live in ordinary Lua, bound with `---@` tags.

```
geometry/
├── luabox.toml
├── shapes/geometry.luab        # structs, traits, a supertrait, a generic
├── src/circle.lua            # ---@impl Shape for Circle
├── src/rect.lua              # ---@impl Shape for Rect
├── src/shapes_data.lua       # ---@struct bindings + sealed-checking demo
└── tests/geometry_test.lua
```

## The shape workflow

1. **Declare types in `.luab`.** `shapes/geometry.luab` declares structs
   (`Point`, `Circle`, `Rect`), a generic `Pair<T>`, a trait `Shape`, and a
   supertrait `Drawable: Shape`. It has no bodies — the parser rejects them
   ("implementations live in .lua"). It is analyser-only: never required at
   runtime, never emitted by `build`/`bundle`.

2. **Point the manifest at it.** `[types] shape-paths = ["shapes"]` tells the
   resolver where to find `.luab` modules for `---@use`.

3. **Bind Lua to shapes with tags:**
   - `---@use geometry` — import the shape module (file top).
   - `---@struct Point` — bind a table literal to a struct. The literal is
     **sealed-checked** against the fields.
   - `---@impl Shape for Circle` — assert conformance. Every trait fn must be
     present with a compatible signature; `:` vs `.` receivers must match
     `self`; extra inherent methods are fine.

4. **Check it.** `luabox check` runs both front-ends — LuaCATS annotations and
   `.luab` shapes — through one type IR.

```sh
luabox check        # 0 errors across .lua + .luab
luabox fmt --check  # .luab files are formatted too (4-space, trailing commas)
luabox lint
luabox test         # exercises the Circle/Rect/Point API on your Lua runtime
```

## Sealed checking (what *would* error)

Shapes are sealed: a missing non-optional field or an unknown key is a hard
error at **every** strictness level (the `---@struct` tag is itself the
opt-in). `src/shapes_data.lua` keeps these as commented illustrations so the
project stays green. Bind `local p = { x = 0 }` to `Point` and `luabox check`
reports:

```
error[LB2001]: missing non-optional field `y` on a value bound to struct `Point`
  --> src/main.lua:4:11
  |
4 | local p = { x = 0 }
  |           ^^^^^^^^^ `Point` requires `y: number`
```

Add an undeclared key and you get the dual diagnostic:

```
error[LB2002]: unknown key `z` on sealed struct `Point`
  --> src/main.lua:4:27
  |
4 | local p = { x = 0, y = 0, z = 0 }
  |                           ^^^^^ `Point` declares no field `z`
```

Delete `Circle:perimeter` and the trait coherence check fires instead:
`error[LB2003]: incomplete ---@impl Shape for Circle: missing perimeter`.

## Exporting shapes to dependents

`[types] shapes = ["geometry"]` marks the `geometry` module as **exported**.
A downstream package that depends on this one can then `---@use geometry` and
get the same sealed checking across the package boundary. See `../renderer`,
which consumes these shapes and implements `Drawable` for its own type.

## A note on constructors and strict types

`Circle.new(radius)` is bound through `---@impl` (not `---@struct`), so its
`setmetatable` call is not sealed against the struct — that keeps ordinary
parameterised constructors ergonomic. The sealed-literal checks are best
shown on concrete data literals, which is exactly what `shapes_data.lua` does.
