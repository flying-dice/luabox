-- The live twin: the same two statements the other way round, where the
-- colon call is reached before the chunk returns.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
c:reset()
do return end
print("unreachable")
