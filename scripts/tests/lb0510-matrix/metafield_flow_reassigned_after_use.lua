-- The twin: the same three lines with the call in the middle, where the
-- colon call really does land on the instance.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
print(c:reset())
c = { reset = function() return 1 end }
