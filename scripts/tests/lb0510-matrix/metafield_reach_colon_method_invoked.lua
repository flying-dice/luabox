-- A carrier colon-method body is reached when an *instance* colon call to
-- that method happens in a reached body: `r:go()` enters `Runner.go`, whose
-- `c:reset()` is the lookup that crashes. `Runner` is wired, so the one
-- finding is against `Cache`.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
---@class Runner
local Runner = {}
Runner.__index = Runner
local c = setmetatable({}, Cache)
function Runner:go() return c:reset() end
local r = setmetatable({}, Runner)
print(r:go())
