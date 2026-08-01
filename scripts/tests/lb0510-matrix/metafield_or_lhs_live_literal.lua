-- The twin: a literal-truthy left operand is the value, and the
-- construction is never evaluated at all.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = "x" or setmetatable({}, Cache)
print(c:upper())
