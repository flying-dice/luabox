-- A `__mode` carrier whose method reaches `self`, and nothing reaches the
-- method. Presence of `self:clear()` in a body no call site enters is not a
-- lookup: round 7 measured this firing on a program that runs fine.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:clear() self.n = 0 end
function Cache:reset() self:clear() end
local c = setmetatable({}, Cache)
print(type(c))
