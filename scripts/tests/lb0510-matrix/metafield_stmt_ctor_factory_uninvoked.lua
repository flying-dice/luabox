-- The twin: the factory is called and its result is only type-checked.
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
print(type(c))
