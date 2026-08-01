-- DISCLOSED false negative: the instance lives in a table field. This pass
-- tracks names, not table contents, so `box.c` derives nothing. Silent on a
-- crash -- the accepted direction for the metafield arm.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local box = { c = setmetatable({}, Cache) }
print(box.c:reset())
