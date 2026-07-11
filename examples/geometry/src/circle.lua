-- Circle is a class carrier: a table with an __index metatable, typed with
-- plain LuaCATS. `---@class geometry.Circle : geometry.Shape` reopens the
-- class declared in ../defs/geometry.d.lua (`radius: number`, extends
-- Shape) — luabox merges the two declarations by name, so `self.radius`
-- below resolves to `number` even though this file repeats only the class
-- name and its extension, not the field. `self` inside `:` methods is
-- inferred as `geometry.Circle` through the `__index` metatable chain, no
-- extra annotation needed (SPEC.md).
--
-- CONFORMANCE (#107): the `: geometry.Shape` here IS verified. luabox checks
-- that this carrier provides every member geometry.Shape declares
-- (area/perimeter/my_static) with a compatible signature — comment out
-- `Circle:perimeter` below and run `luabox check` and it now reports:
--   error[LB0300]: `geometry.Circle` does not satisfy `geometry.Shape`: missing member `perimeter`
--    --> src/circle.lua:19:4
--    | ---@class geometry.Circle : geometry.Shape
--    |    ^^^ expected member `perimeter` of type `fun(self: unknown): number`
-- The check is `__index`-aware, so a subclass inheriting a concrete base
-- method through its metatable chain is not wrongly asked to re-implement it.
-- This is the structural conformance luabox's `.luab` shape modules used to
-- be needed for — now on the plain-LuaCATS path (verified against the real
-- binary while building this example).
---@class geometry.Circle : geometry.Shape
local Circle = {}
Circle.__index = Circle

---@return number
function Circle:area()
    return math.pi * self.radius ^ 2
end

---@return number
function Circle:perimeter()
    return 2 * math.pi * self.radius
end

-- FIELD READS (#90): reads are checked too. `self.nope` — a field declared
-- nowhere on geometry.Circle or geometry.Shape — is luals' `undefined-field`,
-- an error under `strict`. Add `local _ = self.nope` inside `Circle:area`
-- above and `luabox check` reports (reproduced against this exact file, then
-- reverted):
--   error[LB0306]: undefined field `nope` on `geometry.Circle`
--      --> src/circle.lua:29:15
--      |
--   29 |     local _ = self.nope
--      |               ^^^^^^^^^ `geometry.Circle` declares no field `nope`
--      --> src/circle.lua:23:4
--      |
--   23 | ---@class geometry.Circle : geometry.Shape
--      |    --------------------------------------- `geometry.Circle` declared here
-- A class with an indexer (`---@field [string] T`) stays open; a genuinely
-- dynamic read can opt out with `---@diagnostic disable: undefined-field`.

-- The static member of geometry.Shape: no `self`, written with `.` and
-- called as `Circle.my_static()`. All these shapes live in 2D.
---@return number
function Circle.my_static()
    return 2
end

---@param radius number
---@return geometry.Circle
function Circle.new(radius)
    return setmetatable({ radius = radius }, Circle)
end

return Circle
