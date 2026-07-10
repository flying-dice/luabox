-- Rect: the second Shape carrier. Same idiom as circle.lua — plain LuaCATS,
-- the class reopened from ../defs/geometry.d.lua so `self.width`/
-- `self.height` resolve to `number` without repeating the fields here.
---@class geometry.Rect : geometry.Shape
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
