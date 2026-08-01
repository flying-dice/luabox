-- The twin -- same silence, here it is right.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local mt = Cache
local c = setmetatable({}, mt)
print(type(c))
