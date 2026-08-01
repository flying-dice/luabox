-- DISCLOSED false negative: the instance arrives as a parameter. Argument
-- values are not propagated into callee bodies.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local function use(c) return c:reset() end
print(use(setmetatable({}, Cache)))
