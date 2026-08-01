-- The other side of the loop prune: `repeat` tests *after* the body, so
-- the body always executes whatever the condition says.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
repeat c:reset() until true
