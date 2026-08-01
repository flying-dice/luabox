-- `nil` is falsy too, and is a literal the collector can decide.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
if nil then c:reset() end
print("done")
