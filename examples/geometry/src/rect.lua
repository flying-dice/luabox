-- Rect: the second Shape carrier. Same idiom as circle.lua — plain Lua,
-- standard annotations, structural conformance checked wherever a
-- `geometry.Shape` is demanded.
--
-- The `---@type geometry.Shape` verifies the whole accumulated carrier (area,
-- perimeter, my_static) against Shape — deferred to everything Rect becomes,
-- not the empty `{}`.
---@type geometry.Shape
local Rect = {}
Rect.__index = Rect

---@return number
function Rect:area()
    return self.width * self.height
end

---@return number
function Rect:perimeter()
    return 2 * (self.width + self.height)
end

-- The static member of geometry.Shape: no `self`, called as
-- `Rect.my_static()`.
---@return number
function Rect.my_static()
    return 2
end

---@param width number
---@param height number
---@return geometry.Rect
function Rect.new(width, height)
    return setmetatable({ width = width, height = height }, Rect)
end

return Rect
