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

-- Also NOTE (gap): field access is permissive on a declared class. Uncomment
-- the line below — `self.nope` is not declared on geometry.Circle or
-- geometry.Shape anywhere — and `luabox check` still reports 0 errors:
--
--   print(self.nope)

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
