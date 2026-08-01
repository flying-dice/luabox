-- The same guard through a binding: reading `c.reset` into a name is not
-- a call, whatever is done with the name afterwards.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
local r = c.reset
print(type(r))
