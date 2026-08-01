-- The guard on the row above: reading the method off the CARRIER is a
-- direct table read that no metatable serves, so `m(c)` is an ordinary
-- call and runs. Only a read off a *derived value* is the failed lookup.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() return 1 end
local c = setmetatable({}, Cache)
local m = Cache.reset
print(m(c))
