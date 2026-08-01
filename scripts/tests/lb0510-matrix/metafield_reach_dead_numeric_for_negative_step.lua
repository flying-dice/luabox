-- The same with an explicit step: counting down from 1 to 10 is zero
-- iterations, and the step is a unary minus on a literal.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
for _ = 1, 10, -1 do c:reset() end
print("done")
