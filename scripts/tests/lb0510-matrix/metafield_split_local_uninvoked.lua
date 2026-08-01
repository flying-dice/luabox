-- The uninvoked twin of the split-local shape: bound the same way, never
-- reached, so nothing is looked up through the metatable.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c
c = setmetatable({}, Cache)
print(type(c))
