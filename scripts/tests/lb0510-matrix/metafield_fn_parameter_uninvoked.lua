-- The uninvoked twin of the parameter miss.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local function use(c) return type(c) end
print(use(setmetatable({}, Cache)))
