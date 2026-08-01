-- The uninvoked twin of the global-bound instance.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
store = setmetatable({}, Cache)
print(type(store))
