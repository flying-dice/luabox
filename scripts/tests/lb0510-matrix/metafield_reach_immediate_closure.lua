-- An immediately-invoked function expression is its own callee: the body
-- is named by the call itself, so it is reached.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
print((function() return c:reset() end)())
