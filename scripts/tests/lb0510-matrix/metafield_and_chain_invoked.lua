-- `guard and setmetatable(...)` evaluates to the construction whenever it
-- evaluates to anything else, so the right operand is what the name holds.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local ready = true
local c = ready and setmetatable({}, Cache)
print(c:reset())
