-- DISCLOSED false negative, the cost of the row above: the same shape with
-- a falsy left operand really does bind the construction, and crashes.
-- Following only the left operand is the FN-biased half of the trade.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local fallback = nil
local c = fallback or setmetatable({}, Cache)
print(c:reset())
