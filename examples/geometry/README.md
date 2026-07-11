# geometry

The LuaCATS/`.d.lua` flagship. A library (`edition = "5.4"`) whose types live
in an ambient **definition package** — a `---@meta` file resolved by
`[types] defs` — while the implementations are ordinary Lua, annotated with
stock LuaLS tags (`---@class` / `---@field` / `---@param` / `---@return` /
`---@alias` / `---@enum`). No imports, no shape DSL.

> This example previously used `.luab` shape modules (TypeScript-adjacent
> `type` declarations with sealed, structural conformance checking). The
> `.luab` subsystem hasn't gone anywhere — it still lives elsewhere in the
> codebase, ahead of a planned wider drop. This conversion exists to show,
> honestly, what the plain-LuaCATS path looks like **today**: what works,
> what's silently permissive, and what's outright broken. Read to the end —
> some of this will surprise you.

```
geometry/
├── luabox.toml               # [types] defs = ["geometry"]
├── defs/geometry.d.lua        # ---@meta ambient class/alias declarations
├── src/circle.lua             # Shape carrier (class re-opened from defs)
├── src/rect.lua                # Shape carrier (class re-opened from defs)
├── src/shapes_data.lua         # ---@type literals, sealing, alias, enum demo
└── tests/geometry_test.lua
```

## The LuaCATS workflow

1. **Declare types in a `.d.lua` def package.** `defs/geometry.d.lua` is a
   `---@meta` module declaring `geometry.Point`, the interface-shaped
   `geometry.Shape`, `geometry.Drawable : geometry.Shape`, the carrier
   classes `geometry.Circle`/`geometry.Rect`, and the `geometry.Unit` alias.
   It's wired in with `[types] defs = ["geometry"]` — the file stem
   `geometry` resolves to `defs/geometry.d.lua` (same mechanism
   `love-asteroids-lite` uses for `defs/love2d.d.lua`).

2. **Re-open the class where it's implemented.** `src/circle.lua` declares
   `---@class geometry.Circle : geometry.Shape` again on `local Circle = {}`.
   luabox merges declarations of the same class name, so `self.radius`
   resolves to `number` from the field declared in the `.d.lua` file even
   though circle.lua doesn't repeat it. `self` inside `:` methods is
   inferred through the `__index` metatable chain — no extra annotation.

3. **Consume through standard annotations.** `---@type geometry.Point` on a
   table literal, `---@param`/`---@return` on functions — nothing new.

4. **Check it.**

```sh
luabox check        # 0 errors across 4 files
luabox fmt --check   # 5 files formatted
luabox lint          # 0 errors, 0 warnings
luabox test          # 9 passing tests
```

## What actually works (verified against the real binary)

- **`---@class` + `---@field`, `---@param`/`---@return`.** Full support,
  same as any LuaLS-aware editor.
- **`: Interface` conformance IS verified** (#107 — the strictness luals
  declares but trusts). A `---@class geometry.Circle : geometry.Shape`
  carrier must actually provide every member `geometry.Shape` declares, with
  a compatible signature. Delete `Circle:perimeter` from `src/circle.lua`
  and `luabox check` now reports (reproduced against this exact file, then
  reverted):
  ```
  error[LB0300]: `geometry.Circle` does not satisfy `geometry.Shape`: missing member `perimeter`
     --> src/circle.lua:19:4
     |
  19 | ---@class geometry.Circle : geometry.Shape
     |    ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ expected member `perimeter` of type `fun(self: unknown): number`
  ```
  Give a member the wrong type (e.g. `Circle:area` returning a `string`) and
  it is flagged the same way (`member \`area\` has the wrong type`). The check
  is `__index`-aware: a subclass that inherits a concrete base method through
  its metatable chain is **not** told to re-implement it, so classic
  inheritance stays clean. This is exactly the structural conformance the
  `.luab` shape modules used to be needed for — now on the plain-LuaCATS path.
- **Literal sealing.** `---@type geometry.Point` on a table literal enforces
  every non-optional field present and rejects unknown keys — `LB0300`
  ("missing `y`") / `LB0303` ("unknown field `z`"). This is **not** a
  `.luab`-only feature: a plain LuaCATS `---@class` under `[types] strict =
  true` gets the identical treatment. (LuaLS treats this as a soft
  `missing-fields` warning in most editors; here it's a real `check` error.)
  See the commented block in `src/shapes_data.lua`.
- **`---@alias` and `---@enum`.** Both are fully enforced. `geometry.Unit =
  "px"|"pt"` rejects any other string at an annotated position
  (`error[LB0300]: type mismatch: expected \`"px"|"pt"\`, found \`"cm"\``);
  `geometry.ShapeKind` (an `---@enum` over a real runtime table) rejects any
  value that isn't one of its members the same way.
- **`T[]` / `table<K,V>`.** Both check element/value types precisely
  (verified in the mission's scratch experiments, not shown in this
  project's own files, but exercised — see the mission report).
- **`---@meta` ambient globals**, including framework/DLL-style ones — see
  `../love-asteroids-lite/defs/love2d.d.lua` for that pattern; this
  project's own `defs/geometry.d.lua` is the same mechanism applied to a
  library's own types rather than a third-party API.

## What is silently permissive (verified, not avoided)

- **Field access is permissive.** `self.nope` inside a `geometry.Circle`
  method — a field declared nowhere on `geometry.Circle` or
  `geometry.Shape` — is not flagged either. Also confirmed live against this
  example (temporarily, then reverted); see the commented-out line in
  `src/circle.lua`.

## What is outright broken

- **Generic `---@class<T>`.** The moment a `---@field` (or any annotation)
  actually references the class's type parameter, luabox reports it as an
  unresolved name:
  ```
  error[LB0305]: unknown type name `T` in annotation
  ```
  `defs/geometry.d.lua` keeps a real, commented-out `geometry.Pair<T>`
  reaching exactly this error (uncomment it in an ordinary `src/*.lua` file
  to reproduce — see the comment for the sharper nuance: the identical
  declaration placed in a `.d.lua` def file does *not* raise this
  diagnostic at all, because ambient/defs content isn't self-validated the
  way an ordinary checked file is; but the parameterized field still
  silently resolves to `unknown` wherever it's read downstream, so the type
  safety is gone either way — just without a diagnostic pointing at the
  cause). `src/shapes_data.lua` uses a concrete, non-generic `geometry.Pair`
  instead — the actual, working-today alternative.
- **`---@generic` function type parameters lower to `unknown`.** A
  `---@generic T` / `---@param value T` / `---@return T` identity function's
  return value doesn't retain the argument's real type — it becomes the
  literal type `unknown`, which then fails against *any* concrete
  annotation:
  ```
  error[LB0300]: type mismatch: expected `number`, found `unknown`
  ```
  (Confirmed in the mission's scratch experiments; not exercised inside this
  project's own files because there's no natural call site for it here —
  see `../renderer` and the mission report for where a generic function
  would otherwise have been reached for.)

`: Interface` conformance is now checked (see above); real generics are
still slated to land with the `.luab` drop epic (#84 etc.) — until then,
this is the accurate picture.

## Constructors under LuaCATS

```lua
---@param radius number
---@return geometry.Circle
function Circle.new(radius)
    return setmetatable({ radius = radius }, Circle)
end
```

The `---@param` type flows into the body; the `setmetatable(literal,
Circle)` result is checked against the declared `---@return geometry.Circle`
— literal freshness (sealing) applies at that return position exactly as it
does at a `---@type` binding.
