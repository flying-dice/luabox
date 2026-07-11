-- Square: our Drawable carrier. `geometry.Drawable` comes from the vendored
-- copy in defs/geometry.d.lua (a stopgap for a real gap — see that file and
-- the project README: cross-package LuaCATS type sharing doesn't work
-- today, unlike `.luab` shape modules).
--
-- `---@class render.Square : geometry.Drawable` reopens the class declared
-- in defs/render.d.lua (`side: integer`, extends Drawable) — same
-- merge-by-name idiom as ../geometry/src/circle.lua.
--
-- CONFORMANCE (#107): as with geometry's carriers, this `: geometry.Drawable`
-- IS now verified — luabox checks that Square provides every member the
-- Drawable interface chain declares (area/perimeter/my_static from
-- geometry.Shape, plus draw) with compatible signatures. Delete one and
-- `luabox check` reports LB0300 at this `---@class` line. See
-- ../geometry/README.md for the exact error text.
---@class render.Square : geometry.Drawable
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
