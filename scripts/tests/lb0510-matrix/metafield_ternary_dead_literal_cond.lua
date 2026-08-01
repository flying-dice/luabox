-- The ternary `cond and ctor or other` with a literal-FALSE cond. Lua never
-- evaluates the construction: `false and _` is `false`, and `false or
-- other` is `other`. The name holds the fallback, and the fallback answers.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local other = { reset = function() return 1 end }
local c = false and setmetatable({}, Cache) or other
print(c:reset())
