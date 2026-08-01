-- DISCLOSED false positive: the guard is a *name*, not a literal, and the
-- prune is literal-condition only. Deeper dead-code reasoning is the bound
-- the __index-write side has carried since the rule shipped.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
local on = false
if on then c:reset() end
print("done")
