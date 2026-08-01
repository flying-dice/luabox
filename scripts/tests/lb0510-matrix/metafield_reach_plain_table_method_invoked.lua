-- A colon call on an ordinary table reaches that table's method body, and
-- the `c:reset()` inside it is a use of Cache.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
local h = {}
function h:go() return c:reset() end
print(h:go())
