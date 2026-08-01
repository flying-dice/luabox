-- DISCLOSED false negative: the instance is a generic-`for` variable, which
-- takes its value from an iterator this pass does not evaluate.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
for _, c in ipairs({ setmetatable({}, Cache) }) do print(c:reset()) end
