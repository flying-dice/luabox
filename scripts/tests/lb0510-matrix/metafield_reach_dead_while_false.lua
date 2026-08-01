-- `while false do` never enters its body -- the same literal-condition
-- prune as `if false then`.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
while false do c:reset() end
print("done")
