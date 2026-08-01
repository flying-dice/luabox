-- The twin: declared on the carrier, never called.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
function Cache.run() return c:reset() end
print(type(Cache.run))
