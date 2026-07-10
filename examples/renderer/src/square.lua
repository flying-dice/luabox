-- Square: our Drawable carrier. The Drawable type comes from the geometry
-- dependency's exported surface (`geometry.Drawable`) — no import needed,
-- the scope is ambient (SHAPES-V2.md).
--
-- Drawable = geometry.Shape & { draw }, so the `---@type geometry.Drawable`
-- on the declaration verifies the whole accumulated carrier — area, perimeter,
-- my_static, and draw — against everything Square becomes, not the empty `{}`.
---@type geometry.Drawable
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

-- The static member inherited through geometry.Drawable (= Shape & { draw }):
-- no `self`, called as `Square.my_static()`.
---@return number
function Square.my_static()
    return 2
end

---@param side integer
---@return render.Square
function Square.new(side)
    return setmetatable({ side = side }, Square)
end

return Square
