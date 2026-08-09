---@class HBase
---@field id number

---@class HSub : HBase
---@field name string

---@type HSub
local s

---@param n number
local function want_number(n) end
---@param t string
local function want_string(t) end

want_number(s.id)
want_string(s.name)
