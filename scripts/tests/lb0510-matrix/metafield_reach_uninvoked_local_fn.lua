-- Round-8 FP: the colon call lives in a `local function` no call site
-- enters, so it never executes. Measured 1 finding against a clean run.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
local function boom() return c:reset() end
print(type(boom))
