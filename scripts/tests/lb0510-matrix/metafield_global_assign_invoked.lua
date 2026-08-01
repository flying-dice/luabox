-- A global holds the instance. There is no `BindingId` for a free name, so
-- the derivation is keyed on the name instead; the program is the `local`
-- spelling in every other respect, and crashes the same way.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
store = setmetatable({}, Cache)
print(store:reset())
