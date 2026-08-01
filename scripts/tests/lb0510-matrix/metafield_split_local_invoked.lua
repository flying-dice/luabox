-- The construction is bound by an assignment, not by the `local` that
-- declares the name. Seeding value bindings only from `Stmt::Local` left this
-- silent on a crash (Shockwave round 7).
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c
c = setmetatable({}, Cache)
print(c:reset())
