---@type GenericBound<number>
local b = require("generic_bound_mod")

---@param n number
local function want(n) end

want(b.item)
