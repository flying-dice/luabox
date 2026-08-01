-- The uninvoked twin of the `or`-fallback binding.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache) or {}
print(type(c))
