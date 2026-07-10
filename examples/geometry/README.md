# geometry

The `.luab` shape-module flagship. A library (`edition = "5.4"`) whose types
live in a **shape module** — TypeScript-adjacent `type` declarations in a
separate `.luab` file — while the implementations live in ordinary Lua,
consumed through the **standard annotation positions** (`---@type` /
`---@param` / `---@return`). No imports, no binding tags (SHAPES-V2.md).

```
geometry/
├── luabox.toml               # [types] entry — the published type surface
├── shapes/geometry.luab      # type declarations, an intersection, a generic
├── src/circle.lua            # Shape carrier + positional assertion
├── src/rect.lua              # Shape carrier + positional assertion
├── src/shapes_data.lua       # ---@type bindings + sealed-checking demo
└── tests/geometry_test.lua
```

## The shape workflow

1. **Declare types in `.luab`.** `shapes/geometry.luab` declares object types
   (`Point`, `Circle`, `Rect`), a generic `Pair<T>`, a method-set type
   `Shape`, and an intersection `Drawable = Shape & { draw(self): string }`.
   It has no bodies — the parser rejects them ("implementations live in
   .lua"). It is analyser-only: never required at runtime, never emitted by
   `build`/`bundle`.

2. **Point the manifest at it.** `[types] shape-paths = ["shapes"]` makes
   every module under `shapes/` **ambient**: its types are addressable from
   any file by fully-qualified name, derived from the module's path —
   `shapes/geometry.luab` declares `geometry.Point`, `geometry.Shape`, …

3. **Consume through standard annotations.** There is nothing new to learn:
   - `---@type geometry.Point` on a table literal **sealed-checks** it:
     every non-optional field present, no undeclared keys.
   - `---@return geometry.Circle` on a constructor checks the
     `setmetatable(literal, Circle)` result at the return position.
   - `---@type geometry.Shape` on a carrier binding is a **conformance
     assertion by construction** — conformance is structural and positional,
     so the general mechanism covers the special case. No `impl`, no
     `---@impl`.

4. **Check it.** `luabox check` runs both front-ends — LuaCATS annotations and
   `.luab` types — through one type IR.

```sh
luabox check        # 0 errors across .lua + .luab
luabox fmt --check  # .luab files are formatted too (4-space, trailing commas)
luabox lint
luabox test         # exercises the Circle/Rect/Point API on your Lua runtime
```

## Sealed checking (what *would* error)

Object types are sealed. `src/shapes_data.lua` keeps these as commented
illustrations so the project stays green. Bind `local p = { x = 0 }` to
`geometry.Point` and `luabox check` reports:

```
error[LB0302]: missing required field `y` in table literal
```

Add an undeclared key and you get the dual diagnostic:

```
error[LB0303]: unknown field `z`
```

Delete `Circle:perimeter` and the positional conformance assertion in
`src/circle.lua` fires, naming the member:

```
error[LB0300]: type mismatch: expected `geometry.Shape`, found
`{ area: fun(): number, ... }`: missing `perimeter`
```

## Exporting types to dependents

`[types] entry = "shapes/geometry.luab"` names the **type entrypoint**
(TS-style, like package.json `"types"`): its `export type` declarations form
this package's published surface, mounted under the package name. A
downstream package that depends on this one addresses them as
`geometry.Shape`, `geometry.Point` — same names, no import. See
`../renderer`, which declares its own type and asserts it is a
`geometry.Drawable`.

## Constructors under strict types

`Circle.new(radius)` is plain Lua with standard annotations:

```lua
---@param radius number
---@return geometry.Circle
function Circle.new(radius)
    return setmetatable({ radius = radius }, Circle)
end
```

The `---@param` type flows into the body (so `radius` is a `number` inside
the literal), and the `setmetatable(literal, Circle)` result is checked
against the declared `---@return geometry.Circle` — the return position is
where the instance literal meets the type.
