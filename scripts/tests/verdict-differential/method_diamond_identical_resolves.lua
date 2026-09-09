---@class MethodBase
local T = {}
function T:m()
  return "s"
end

---@class MethodMidA : MethodBase
---@class MethodMidB : MethodBase
---@class MethodDiamond : MethodMidA, MethodMidB

---@type MethodDiamond
local c

---@param s string
local function want_string(s) end

want_string(c:m())
