-- The same carrier with `c:reset()` actually reached. A colon method is
-- reachable only through an instance colon call, which the value derivations
-- already see -- and that call is the one that crashes.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:clear() self.n = 0 end
function Cache:reset() self:clear() end
local c = setmetatable({}, Cache)
print(c:reset())
