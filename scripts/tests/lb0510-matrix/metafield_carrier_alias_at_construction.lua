-- DISCLOSED false negative: the metatable argument is an *alias* of the
-- carrier. The class annotation is looked up on the binding the argument
-- names, and `mt` carries none -- so the rule never reaches its gates.
-- Rooting the lookup through the alias map would also root a reassigned
-- alias (`mt = {}`), which is a false positive, so this stays disclosed.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local mt = Cache
local c = setmetatable({}, mt)
print(c:reset())
