-- Issue W: the factory returns the construction in its *second* slot, and the
-- name that takes it is past the end of the initialiser list.
-- `names.iter().zip(init)` dropped `c` outright and handed `n` a derivation
-- that belongs to it; both halves are wrong, and the file crashes.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local function make() return 1, setmetatable({}, Cache) end
local n, c = make()
print(n, c:reset())
