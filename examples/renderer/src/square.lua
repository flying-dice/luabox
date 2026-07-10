-- Square: our Drawable carrier. Implements the `Drawable` trait imported from
-- the geometry dependency's exported shape module.

---@use geometry
---@use render

-- Square implements Drawable. The trait's supertrait is Shape, so we must also
-- provide area + perimeter — `luabox check` enforces the whole obligation.
-- `---@struct Square` binds the carrier to the struct: `self.side` types as
-- the declared `integer`, and the constructor's `setmetatable` literal is
-- sealed-checked against the struct's fields.
---@struct Square
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
        rows[#rows + 1] = string.rep("#", self.side)
    end
    return table.concat(rows, "\n")
end

---@param side integer
---@return Square
function Square.new(side)
    return setmetatable({ side = side }, Square)
end

return Square
