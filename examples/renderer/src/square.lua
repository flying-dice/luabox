-- Square: our Drawable carrier. The Drawable type comes from the geometry
-- dependency's exported surface (`geometry.Drawable`) — no import needed,
-- the scope is ambient (SHAPES-V2.md).
local Square = {}
Square.__index = Square

---@return number
function Square:area()
    return self.side * self.side
end

---@return number
function Square:perimeter()
    return 4 * self.side
end

---@return string
function Square:draw()
    local rows = {}
    for _row = 1, self.side do
        rows[#rows + 1] = string.rep("#", self.side)
    end
    return table.concat(rows, "\n")
end

---@param side integer
---@return render.Square
function Square.new(side)
    return setmetatable({ side = side }, Square)
end

-- Positional conformance: Drawable is `geometry.Shape & { draw }`, so this
-- single assertion carries the whole obligation — area, perimeter, and
-- draw. Delete any of them and `luabox check` names it here.
---@type geometry.Drawable
local _ = Square

return Square
