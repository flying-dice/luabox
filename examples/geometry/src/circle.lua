---@use geometry

-- Circle is a class carrier: a table with an __index metatable whose methods
-- are the impl of the `Shape` trait. `---@struct Circle` binds the carrier to
-- the struct (so `setmetatable(literal, Circle)` is sealed-checked and its
-- result types as a Circle instance), and `---@impl Shape for Circle` binds
-- it to the trait; `luabox check` then enforces that every trait fn is
-- present with a compatible signature (try deleting `perimeter` to see
-- error[LB2003]).
---@struct Circle
---@impl Shape for Circle
local Circle = {}
Circle.__index = Circle

function Circle:area()
    return math.pi * self.radius ^ 2
end

function Circle:perimeter()
    return 2 * math.pi * self.radius
end

---@param radius number
---@return Circle
function Circle.new(radius)
    return setmetatable({ radius = radius }, Circle)
end

return Circle
