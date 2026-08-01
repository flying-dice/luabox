-- The __call chain through an alias of the instance: `g` is `f`, so `g()`
-- is the same dispatch. Pinned so the reached-bodies work cannot quietly
-- break the chain the previous round closed.
---@class Factory
local Factory = {}
function Factory:build() return 42 end
function Factory.__call(self) return self:build() end
local f = setmetatable({}, Factory)
local g = f
print(g())
