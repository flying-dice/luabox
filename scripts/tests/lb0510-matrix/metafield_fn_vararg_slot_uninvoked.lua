-- The uninvoked twin of the vararg-slot miss.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local function use(...)
  local a, c = ...
  return a, type(c)
end
print(use(1, setmetatable({}, Cache)))
