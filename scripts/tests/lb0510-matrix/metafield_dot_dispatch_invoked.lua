-- Dot dispatch with an explicit self: `c.reset(c)` resolves `reset`
-- through the metatable exactly as `c:reset()` does, and fails the same way
-- (`attempt to call a nil value (field 'reset')`). It counts when the key
-- names a function attached to the carrier.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
print(c.reset(c))
