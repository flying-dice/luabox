-- Circle is a class carrier: a table with an __index metatable, typed with
-- plain LuaCATS. `---@class geometry.Circle : geometry.Shape` reopens the
-- class declared in ../defs/geometry.d.lua (`radius: number`, extends
-- Shape) — luabox merges the two declarations by name, so `self.radius`
-- below resolves to `number` even though this file repeats only the class
-- name and its extension, not the field. `self` inside `:` methods is
-- inferred as `geometry.Circle` through the `__index` metatable chain, no
-- extra annotation needed (SPEC.md).
--
-- NOTE (gap): the `: geometry.Shape` here is NOT verified. luabox does not
-- check that Circle actually implements area/perimeter/my_static — comment
-- out `Circle:perimeter` below and run `luabox check`: it still reports
-- 0 errors. A rigorous checker would report something like:
--   error: `geometry.Circle` does not satisfy `geometry.Shape`: missing `perimeter`
-- Today it passes silently. This is the trade luabox's `.luab` shape modules
-- exist to avoid (structural conformance IS checked there — see ../renderer
-- for the .luab-era version of this same example, and the mission report
-- for the exact commands used to confirm this gap against the real binary).
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
