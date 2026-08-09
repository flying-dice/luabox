---@class MBase
---@field id number

---@class MSub : MBase
---@field name string

---@type MSub
local s

---@param t string
local function want_string(t) end

want_string(s.id)
