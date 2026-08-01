-- `boom` escapes into `pcall`, so its body is not reached -- and `pcall`
-- swallows the error the call would raise, so the file exits 0 anyway. Both
-- columns agree by different routes.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
local function boom() return c:reset() end
print(pcall(boom))
