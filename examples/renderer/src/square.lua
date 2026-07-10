-- Square: our Drawable carrier. Implements the `Drawable` trait imported from
-- the geometry dependency's exported shape module.

---@use geometry
---@use render

-- Square implements Drawable. The trait's supertrait is Shape, so we must also
-- provide area + perimeter — `luabox check` enforces the whole obligation.
---@impl Drawable for Square
local Square = {}
Square.__index = Square

function Square:area()
    return self.side * self.side
end

function Square:perimeter()
    return 4 * self.side
end

function Square:draw()
    local rows = {}
    for _row = 1, self.side do
        local cells = {}
        for _col = 1, self.side do
            cells[#cells + 1] = "#"
        end
        rows[#rows + 1] = table.concat(cells)
    end
    return table.concat(rows, "\n")
end

---@param side number
function Square.new(side)
    return setmetatable({ side = side }, Square)
end

return Square
