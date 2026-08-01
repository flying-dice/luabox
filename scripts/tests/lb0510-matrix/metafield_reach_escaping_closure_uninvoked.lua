-- The twin -- same silence, here it is right.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
local function boom() return c:reset() end
local function apply(f) return f() end
print(type(apply), type(boom))
