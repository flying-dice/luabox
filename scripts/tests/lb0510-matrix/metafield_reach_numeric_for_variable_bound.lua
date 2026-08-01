-- DISCLOSED false positive, and the bound of the numeric-for prune: the
-- upper bound is a *name*, so the header is not decided -- the same
-- literal-only limit the `if` guard carries.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
local n = 0
for _ = 1, n do c:reset() end
print("done")
