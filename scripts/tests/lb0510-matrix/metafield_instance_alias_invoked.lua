-- `local w = v` names the same table, so `w:length()` is a use of `v`.
---@class Vec
local Vec = {}
function Vec.__tostring(v) return "vec" end
function Vec:length() return 0 end
local v = setmetatable({}, Vec)
local w = v
print(w:length())
