-- `false or ctor` -- the left operand is a literal Lua treats as false, so
-- the construction IS the value of the expression. Following the left
-- operand alone missed it.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = false or setmetatable({}, Cache)
print(c:reset())
