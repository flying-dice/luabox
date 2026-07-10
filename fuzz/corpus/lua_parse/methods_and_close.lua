function obj.ns:method(a, b, ...)
  local args = { ... }
  return self, select('#', ...)
end
obj = setmetatable({}, { __index = function(_, k) return k end })
obj:method 'lit' -- string call
obj:method { 1, 2 }
do local x <close> = open() end
return obj
