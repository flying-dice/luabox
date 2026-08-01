-- `x and ctor` with a falsy `x`: the name holds `false`, not the instance,
-- and the colon call fails anyway. Seeding the right operand of `and` is
-- sound on both sides of the guard -- lua5.4 crashes either way.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local ready = false
local c = ready and setmetatable({}, Cache)
print(c:reset())
