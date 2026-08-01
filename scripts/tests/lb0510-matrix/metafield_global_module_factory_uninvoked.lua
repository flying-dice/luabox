-- The twin.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
M = {}
function M.new() return setmetatable({}, Cache) end
local c = M.new()
print(type(c))
