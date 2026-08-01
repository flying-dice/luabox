-- DISCLOSED false negative: the factory is reached by a *method* call
-- (`factory:make()`), and only plain calls of a named function or a carrier
-- field are resolved to a body.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local factory = {}
function factory:make() return setmetatable({}, Cache) end
local c = factory:make()
print(c:reset())
