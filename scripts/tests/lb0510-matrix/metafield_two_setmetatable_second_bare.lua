-- The twin, the other way round: the winning metatable is the one with no
-- `__index`, so the lookup really does fail. The wired carrier is settled
-- and silent; the finding is the second call's.
---@class Store
local Store = {}
Store.__mode = "k"
Store.__index = Store
function Store:reset() return 1 end
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local t = {}
setmetatable(t, Store)
setmetatable(t, Cache)
print(t:reset())
