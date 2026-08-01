-- The idiomatic statement-form constructor: `setmetatable(t, C)` in
-- statement position, where only the call's *result* used to be tracked and
-- the table it was applied to was not.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local t = {}
setmetatable(t, Cache)
print(t:reset())
