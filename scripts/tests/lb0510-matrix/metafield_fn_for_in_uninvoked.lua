-- The uninvoked twin of the for-in miss.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
for _, c in ipairs({ setmetatable({}, Cache) }) do print(type(c)) end
