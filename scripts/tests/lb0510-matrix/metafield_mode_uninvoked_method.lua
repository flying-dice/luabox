-- `__mode` is a weak-table hint; `Cache:reset` is declared and never invoked
-- on an instance. Nothing is looked up through the metatable.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local store = setmetatable({}, Cache)
store[{}] = 1
print(type(store))
