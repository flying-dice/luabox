
---@class P1
local T1 = {}
function T1:m()
  return 1
end

---@class C : P1

---@type C
local c
local y = c.m
