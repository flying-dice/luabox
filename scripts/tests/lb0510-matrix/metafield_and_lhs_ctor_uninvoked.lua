-- The twin.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache) and { reset = function() return 1 end }
print(type(c))
