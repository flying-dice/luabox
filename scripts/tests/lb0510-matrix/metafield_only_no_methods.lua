-- A pure operator metatable: a metafield, no methods, nothing invoked.
---@class Vec
local Vec = {}
function Vec.__tostring(v) return "vec" end
local v = setmetatable({}, Vec)
print(tostring(v))
