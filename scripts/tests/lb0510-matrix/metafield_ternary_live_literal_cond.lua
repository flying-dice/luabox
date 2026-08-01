-- The twin: a literal-TRUE cond makes the construction the value of the
-- whole ternary, and the colon call on it fails.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local other = { reset = function() return 1 end }
local c = true and setmetatable({}, Cache) or other
print(c:reset())
