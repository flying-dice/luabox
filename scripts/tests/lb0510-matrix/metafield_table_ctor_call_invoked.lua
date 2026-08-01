-- The `__call` declared in the carrier's own table constructor, reached by a
-- plain call on the instance: `r()` runs `Router.__call(r)`, which runs
-- `r:route()`, which is the lookup `__index` would have served.
---@class Router
local Router = { __call = function(self) return self:route() end }
function Router:route() return "/" end
local r = setmetatable({}, Router)
print(r())
