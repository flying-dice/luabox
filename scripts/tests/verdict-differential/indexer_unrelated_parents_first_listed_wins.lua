---@class IdxP1
---@field [string] number

---@class IdxP2
---@field [string] string

---@class IdxChild : IdxP1, IdxP2

---@type IdxChild
local c

---@param n number
local function want_number(n) end

want_number(c.anything)
