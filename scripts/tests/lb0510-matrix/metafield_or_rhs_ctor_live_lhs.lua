-- Round-8 FP: `x or ctor` evaluates the construction only when `x` is
-- falsy, which this pass cannot decide -- here `x` is a table, so the
-- construction never runs and the fallback's own `reset` answers.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local fallback = { reset = function() return 1 end }
local c = fallback or setmetatable({}, Cache)
print(c:reset())
