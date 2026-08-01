-- The same, with the construction bound to a local inside the factory.
---@class Counter
local Counter = {}
Counter.__mode = "k"
function Counter:value() return self.n end
function Counter.new(n)
  local instance = setmetatable({ n = n }, Counter)
  return instance
end
local c = Counter.new(1)
print(c:value())
