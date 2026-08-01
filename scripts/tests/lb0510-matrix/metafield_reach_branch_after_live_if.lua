-- Symmetry, continued: once a literal-true branch is taken, every later
-- `elseif` arm is dead too -- Lua tests them in order and stops at the
-- first truthy one.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
if true then print(1) elseif true then c:reset() end
print("done")
