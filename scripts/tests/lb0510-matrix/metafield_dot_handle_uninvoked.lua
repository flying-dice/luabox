-- The twin: the same read, never called. The lookup yields nil and the
-- program runs.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
local m = c.reset
print(type(m))
