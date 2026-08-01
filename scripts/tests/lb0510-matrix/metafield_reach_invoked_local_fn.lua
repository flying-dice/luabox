-- The twin: the same body, actually called. The reached-bodies fixpoint
-- follows the call, so the colon call inside it counts.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
local function boom() return c:reset() end
print(boom())
