-- Pruning the `then` of a literal-false `if` must not prune its `else`,
-- which is exactly the branch that runs.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
if false then print(1) else c:reset() end
