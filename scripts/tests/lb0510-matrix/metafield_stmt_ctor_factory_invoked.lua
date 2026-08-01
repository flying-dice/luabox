-- The statement form inside a factory. Linking the first argument makes
-- `t` derived, which is what the return-a-derived-local factory logic needs.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
function Cache.new()
  local t = {}
  setmetatable(t, Cache)
  return t
end
local c = Cache.new()
print(c:reset())
