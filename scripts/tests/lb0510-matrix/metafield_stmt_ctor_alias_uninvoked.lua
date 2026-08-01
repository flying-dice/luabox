-- The twin.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local t = {}
local u = t
setmetatable(u, Cache)
print(type(t))
