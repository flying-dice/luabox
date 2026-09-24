-- The smallest Typed Lua program: typed locals, a typed function, a loop.


local function greet(name        , times         )
  return string.rep("hello " .. name .. "! ", times)
end

local names           = { "ada", "grace", "linus" }
for i         , name         in ipairs(names) do
  print(i, greet(name, 2))
end
