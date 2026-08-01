-- DISCLOSED false negative: the instance arrives in a `...` slot. The name is
-- paired with the right slot of the right expression -- what is missing is
-- any knowledge of what a vararg holds.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local function use(...)
  local a, c = ...
  return a, c:reset()
end
print(use(1, setmetatable({}, Cache)))
