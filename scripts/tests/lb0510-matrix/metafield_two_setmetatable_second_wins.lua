-- DISCLOSED false positive, and the same flow-insensitivity: `setmetatable`
-- REPLACES the metatable, so only the second call is in effect when the
-- method is looked up. One finding per site is right in general -- each
-- call is a real construction -- but the superseded first one is reported
-- against a lookup the winning metatable serves.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
---@class Store
local Store = {}
Store.__mode = "k"
Store.__index = Store
function Store:reset() return 1 end
local t = {}
setmetatable(t, Cache)
setmetatable(t, Store)
print(t:reset())
