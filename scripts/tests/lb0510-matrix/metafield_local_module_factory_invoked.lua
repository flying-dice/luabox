-- The control for the row above: the same module table as a local, which
-- has always resolved. The pair isolates the global-vs-local difference.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local M = {}
function M.new() return setmetatable({}, Cache) end
local c = M.new()
print(c:reset())
