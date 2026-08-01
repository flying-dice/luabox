-- The twin: a literal-truthy left operand makes the right operand the
-- value of the `and`, so the construction is what the name holds.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = true and setmetatable({}, Cache)
print(c:reset())
