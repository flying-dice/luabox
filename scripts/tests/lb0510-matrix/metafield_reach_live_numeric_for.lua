-- The live twin: one iteration is enough to reach the method.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
for _ = 1, 1 do c:reset() end
print("done")
