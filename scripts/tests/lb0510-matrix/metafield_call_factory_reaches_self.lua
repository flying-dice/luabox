-- The instance-side use lives inside the carrier's own `__call`: calling the
-- instance runs `self:build()`, which is the lookup `__index` would serve.
---@class Factory
local Factory = {}
function Factory:build() return 42 end
function Factory.__call(self) return self:build() end
local f = setmetatable({}, Factory)
print(f())
