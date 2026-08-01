-- A function attached to the carrier as a plain field, reached by
-- `Cache.run()` -- the fn_of_field edge the call graph already tracked.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
function Cache.run() return c:reset() end
print(Cache.run())
