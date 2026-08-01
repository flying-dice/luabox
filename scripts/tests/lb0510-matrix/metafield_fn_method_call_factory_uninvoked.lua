-- The uninvoked twin of the method-call factory miss.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local factory = {}
function factory:make() return setmetatable({}, Cache) end
local c = factory:make()
print(type(c))
