-- The twin: `Runner.go` is declared on a constructed instance and never
-- called on one.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
---@class Runner
local Runner = {}
Runner.__index = Runner
local c = setmetatable({}, Cache)
function Runner:go() return c:reset() end
local r = setmetatable({}, Runner)
print(type(r))
