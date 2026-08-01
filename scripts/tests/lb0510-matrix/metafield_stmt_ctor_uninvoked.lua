-- The twin: constructed the same way, never called on.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local t = {}
setmetatable(t, Cache)
print(type(t))
