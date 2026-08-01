-- DISCLOSED false negative: `boom` escapes as an argument, and argument
-- values are not propagated into callee bodies, so nothing links `apply`'s
-- `f()` back to it. The body really is entered, and the file crashes.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
local function boom() return c:reset() end
local function apply(f) return f() end
print(apply(boom))
