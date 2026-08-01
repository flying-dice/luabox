-- The link is on evaluation, not on statement position: the same call used
-- as an expression still makes its first argument an instance.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local t = {}
print(setmetatable(t, Cache))
print(t:reset())
