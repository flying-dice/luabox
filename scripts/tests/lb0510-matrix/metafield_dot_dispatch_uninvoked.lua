-- The twin, and the guard against counting bare field *reads*: the lookup
-- happens, yields nil, and nothing is called -- so the program runs.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
print(type(c.reset))
