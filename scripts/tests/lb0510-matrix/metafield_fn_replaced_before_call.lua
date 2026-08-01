-- The twin, and the FALSE POSITIVE the same map produces the other way
-- round: the call happens before the replacement, so it enters the first
-- body -- but the map already holds the second, whose colon call never runs.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
local function run() return "first" end
run()
run = function() return c:reset() end
print(type(run))
