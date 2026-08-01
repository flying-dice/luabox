-- The uninvoked twin: the same carrier, with nothing calling the instance.
---@class Router
local Router = { __call = function(self) return self:route() end }
function Router:route() return "/" end
local r = setmetatable({}, Router)
print(type(r))
