---@class MethodP1
local T1 = {}
function T1:m()
  return 1
end

---@class MethodP2
local T2 = {}
function T2:m()
  return "s"
end

---@class MethodChild : MethodP1, MethodP2

---@type MethodChild
local c

---@param s string
local function want_string(s) end

want_string(c:m())
