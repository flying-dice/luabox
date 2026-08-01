-- The chunk returns the closure to whoever required it. Nothing in this
-- file enters the body, so nothing in this file performs the lookup.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
local function boom() return c:reset() end
return boom
