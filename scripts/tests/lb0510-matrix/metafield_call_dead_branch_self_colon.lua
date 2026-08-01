-- The last of the round-7 disclosed approximations, closed by the same
-- literal-condition prune: a `self:m()` inside `if false then` within the
-- __call body is not a lookup, and lua5.4 runs the file.
---@class Factory
local Factory = {}
function Factory:build() return 42 end
function Factory.__call(self)
  if false then return self:build() end
  return 0
end
local f = setmetatable({}, Factory)
print(f())
