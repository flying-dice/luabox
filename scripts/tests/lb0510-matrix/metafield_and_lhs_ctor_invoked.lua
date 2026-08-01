-- `ctor and x` evaluates to `x`: a table is truthy, so the construction is
-- discarded and the name holds the right operand. Not a use of Cache.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache) and { reset = function() return 1 end }
print(c:reset())
