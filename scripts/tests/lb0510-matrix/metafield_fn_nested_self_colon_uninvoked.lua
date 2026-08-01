-- The uninvoked twin of the nested-closure `__call` miss.
---@class Factory
local Factory = {}
function Factory:build() return 42 end
function Factory.__call(self)
  local run = function() return self:build() end
  return run()
end
local f = setmetatable({}, Factory)
print(type(f))
