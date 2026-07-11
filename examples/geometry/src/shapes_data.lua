-- Plain data annotated with LuaCATS `---@type`. The classes come from
-- ../defs/geometry.d.lua (ambient via `[types] defs = ["geometry"]`) — no
-- imports, addressed by plain name.

-- A Point is plain data — no methods. `---@type geometry.Point` checks the
-- literal against the declared class: every non-optional field must be
-- present, and no unknown keys are allowed.
--
-- Under `[types] strict = true` these literal checks are enforced (verified
-- against the real binary — see the mission report). The `LuaLS`-style
-- `missing-fields`/excess-field lints most editors treat as soft warnings
-- are here real `luabox check` errors.
---@type geometry.Point
local origin = { x = 0, y = 0 }

-- The optional `label` and `unit` fields may be provided or omitted.
-- `unit` is typed `geometry.Unit` — a closed `---@alias` union
-- (`"px"|"pt"`) — and IS enforced: see the commented-out bad value below.
---@type geometry.Point
local corner = { x = 1, y = 1, label = "top-right", unit = "px" }

-- OPERATOR OVERLOADS (#114): `geometry.Point` declares `---@operator add`
-- (see ../defs/geometry.d.lua), so `corner + origin` types as a
-- `geometry.Point` and binds cleanly to the annotated local below. If the
-- overload were not applied the expression would degrade to `unknown` and
-- `unknown -> geometry.Point` would error under `strict` — a clean check
-- proves the declared result flowed. Rebinding it to a `---@type string`
-- would be a real `luabox check` LB0300 (verified against the real binary).
---@type geometry.Point
local translated = corner + origin

-- geometry.Pair is a REAL generic class (#84): `geometry.Pair<number>`
-- substitutes `number` for the parameter `T`, so both `first` and `second`
-- are checked as numbers here. A string in either field would be a real
-- `luabox check` error (see ../README.md for the exact message).
---@type geometry.Pair<number>
local dimensions = { first = 640, second = 480 }

-- ---------------------------------------------------------------------------
-- Sealed checking, demonstrated. Each line below WOULD be an error under
-- `luabox check` (this project sets `[types] strict = true`). They are
-- commented out so the project stays green — uncomment one to see it fire:
--
--   ---@type geometry.Point
--   local missing = { x = 0 }
--       -- error[LB0300]: type mismatch: expected `geometry.Point`, found
--       --                `{ x: 0 }`: missing `y`
--
--   ---@type geometry.Point
--   local extra = { x = 0, y = 0, z = 0 }
--       -- error[LB0303]: unknown field `z` in table literal
--
--   ---@type geometry.Point
--   local bad_unit = { x = 0, y = 0, unit = "cm" }
--       -- error[LB0300]: type mismatch: expected `"px"|"pt"`, found `"cm"`
--       -- (the alias union IS enforced — this is a real error, not a gap)
-- ---------------------------------------------------------------------------

-- geometry.ShapeKind: a real runtime table tagged `---@enum`, demonstrating
-- that construct too. Enum member values ARE type-checked at annotated
-- positions (see tests/geometry_test.lua).
---@enum geometry.ShapeKind
local ShapeKind = { Circle = "circle", Rect = "rect" }

---Describe a shape kind as a short phrase. Demonstrates `---@param`/
---`---@return` alongside the enum.
---
-- Incidental finding: `"a " .. kind` (concatenating a string literal
-- directly with an `---@enum`-typed value) infers as `unknown`, not
-- `string`, and fails the `---@return string` check below even though
-- every enum member is itself a string. `tostring(kind)` sidesteps it (its
-- stdlib return type is a plain `string`, unaffected by the enum). Not one
-- of the three named gaps in the mission report, but discovered building
-- this example — worth knowing if you hit the same thing.
---@param kind geometry.ShapeKind
---@return string
local function describe_kind(kind)
    return "a " .. tostring(kind)
end

return {
    origin = origin,
    corner = corner,
    translated = translated,
    dimensions = dimensions,
    ShapeKind = ShapeKind,
    describe_kind = describe_kind,
}
