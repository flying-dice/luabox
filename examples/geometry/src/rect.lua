-- Rect: the second Shape carrier. Same idiom as circle.lua — plain Lua,
-- standard annotations, structural conformance checked wherever a
-- `geometry.Shape` is demanded.
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

---@param width number
---@param height number
---@return geometry.Rect
function Rect.new(width, height)
    return setmetatable({ width = width, height = height }, Rect)
end

-- Positional conformance assertion — see circle.lua for the full note.
---@type geometry.Shape
local _ = Rect

return Rect
