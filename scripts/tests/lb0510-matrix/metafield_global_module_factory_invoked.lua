-- The module-table factory on a *global* table. Functions attached to a
-- table were keyed by binding, and a global name has no binding, so
-- `M.new()` resolved to no body and the instance was never derived.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
M = {}
function M.new() return setmetatable({}, Cache) end
local c = M.new()
print(c:reset())
