---@use geometry

---@struct Rect
---@impl Shape for Rect
local Rect = {}
Rect.__index = Rect

function Rect:area()
    return self.width * self.height
end

function Rect:perimeter()
    return 2 * (self.width + self.height)
end

---@param width number
---@param height number
---@return Rect
function Rect.new(width, height)
    return setmetatable({ width = width, height = height }, Rect)
end

return Rect
