-- `setmetatable(...) or fallback` evaluates to the construction (a table is
-- always truthy), so the name holds an instance.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache) or {}
print(c:reset())
