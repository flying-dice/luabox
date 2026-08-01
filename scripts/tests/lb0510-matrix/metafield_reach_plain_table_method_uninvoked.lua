-- The twin: the method is declared and only its type is asked for.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
local h = {}
function h:go() return c:reset() end
print(type(h.go))
