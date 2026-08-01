-- The mirror of metafield_reach_else_of_dead_if, and the live twin the
-- matrix was missing: a literal-TRUE `if` runs its `then`, so its `else`
-- is the branch that never executes and must be pruned with it.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
if true then print(1) else c:reset() end
print("done")
