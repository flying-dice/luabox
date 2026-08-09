---@class VStrictBase<T>
---@field item T

---@class VStrictSub : VStrictBase
local s

---@param s2 string
local function want_string(s2) end

want_string(s.item)
