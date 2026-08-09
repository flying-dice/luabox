---@class Cx
---@field a string

---@class Cx
---@field a number

---@param n number
local function want_number(n) end

---@param c Cx
local function use(c) want_number(c.a) end

return use
