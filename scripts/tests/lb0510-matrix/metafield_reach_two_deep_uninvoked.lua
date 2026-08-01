-- The twin: the same two-deep chain with nothing at the top of it.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
local function g() return c:reset() end
local function f() return g() end
print(type(f))
