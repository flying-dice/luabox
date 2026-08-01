-- `local m = c.reset` reads the method off the INSTANCE, which with no
-- `__index` yields nil -- so calling `m` fails exactly as `c.reset(c)` does.
-- The read alone is silent (metafield_dot_field_read_only); it is the call
-- that is the lookup.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
local m = c.reset
print(m(c))
