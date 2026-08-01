-- The first-argument link is alias-rooted: `u` and `t` name one table, so
-- constructing through `u` makes `t` an instance.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local t = {}
local u = t
setmetatable(u, Cache)
print(t:reset())
