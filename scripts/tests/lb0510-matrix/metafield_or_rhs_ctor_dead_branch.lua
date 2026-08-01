-- Shockwave's measured round-8 FP, which two independent mechanisms now
-- silence: the operand `or` actually evaluates, and the literal-false
-- branch prune. Either alone is enough; both are pinned separately above.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local fallback = nil
local c = fallback or setmetatable({}, Cache)
if false then c:reset() end
print("done")
