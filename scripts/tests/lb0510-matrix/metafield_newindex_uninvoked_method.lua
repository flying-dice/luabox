-- A `__newindex` write guard, with a helper nothing calls on an instance.
---@class Guard
local Guard = {}
Guard.__newindex = function(t, k, v) rawset(t, k, v) end
function Guard:reject() return false end
local g = setmetatable({}, Guard)
g.x = 1
print(g.x)
