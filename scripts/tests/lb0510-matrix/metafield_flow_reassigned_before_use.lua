-- DISCLOSED false positive: derivation is flow-insensitive. `c` holds an
-- instance of Cache on one line and a plain table on the next, and the pass
-- records both against the one name -- so the colon call on the *second*
-- value is counted against the first.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
c = { reset = function() return 1 end }
print(c:reset())
