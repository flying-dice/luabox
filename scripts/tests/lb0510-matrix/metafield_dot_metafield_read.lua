-- A metafield read on an instance is not an instance-side use either --
-- `__mode` is not served by `__index`.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
print(type(c.__mode))
