-- The construction method-called on the spot, no local in between.
---@class Vec
local Vec = {}
function Vec.__tostring(v) return "vec" end
function Vec:length() return 0 end
print(setmetatable({}, Vec):length())
