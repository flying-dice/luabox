-- DISCLOSED false negative: the `__call` reaches the method through a closure
-- nested inside it, and only the metamethod's own body is scanned for a colon
-- call on its receiver.
---@class Factory
local Factory = {}
function Factory:build() return 42 end
function Factory.__call(self)
  local run = function() return self:build() end
  return run()
end
local f = setmetatable({}, Factory)
print(f())
