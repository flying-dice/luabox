-- The reached set is a fixpoint, not one hop: the chunk calls `f`, `f`
-- calls `g`, and the colon call in `g` is what runs.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
local function g() return c:reset() end
local function f() return g() end
print(f())
