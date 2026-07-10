-- Circle is a class carrier: a table with an __index metatable whose methods
-- make it a `geometry.Shape` *structurally* — there is no binding tag and no
-- conformance declaration. The constructor's `---@return geometry.Circle` is
-- where the instance literal is checked against the type (try removing
-- `radius` from the literal to see the error); it also ties the carrier to
-- the instance type, so `self` in the methods below types as `geometry.Circle`
-- and `self.radius` resolves as its declared `number`.
--
-- The `---@type geometry.Shape` on the declaration verifies the whole
-- accumulated carrier (area, perimeter, my_static) against Shape — deferred to
-- everything Circle becomes, not the empty `{}`. Delete any member and
-- `luabox check` names it here.
---@type geometry.Shape
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
