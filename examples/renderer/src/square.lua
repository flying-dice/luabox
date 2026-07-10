-- Square: our Drawable carrier. `geometry.Drawable` comes from the vendored
-- copy in defs/geometry.d.lua (a stopgap for a real gap — see that file and
-- the project README: cross-package LuaCATS type sharing doesn't work
-- today, unlike `.luab` shape modules).
--
-- `---@class render.Square : geometry.Drawable` reopens the class declared
-- in defs/render.d.lua (`side: integer`, extends Drawable) — same
-- merge-by-name idiom as ../geometry/src/circle.lua.
--
-- NOTE (gap): as with geometry's carriers, this `: geometry.Drawable` is
-- NOT verified — luabox does not check that Square actually implements
-- area/perimeter/my_static/draw. See ../geometry/README.md for a live,
-- verified demonstration of that gap (delete a method, 0 errors).
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
