-- DISCLOSED false positive: the cond is a *name* bound to a literal one hop
-- back, and the operand rules read the expression rather than propagating
-- constants -- the same bound `metafield_reach_guard_variable_false` pins
-- for `if`.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local other = { reset = function() return 1 end }
local ready = false
local c = ready and setmetatable({}, Cache) or other
print(c:reset())
