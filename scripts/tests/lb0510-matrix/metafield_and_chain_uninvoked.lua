-- The uninvoked twin of the `and`-guarded binding.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local ready = true
local c = ready and setmetatable({}, Cache)
print(type(c))
