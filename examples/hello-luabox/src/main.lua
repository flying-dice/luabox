-- hello-luabox: the 60-second tour of the toolchain.
--
-- One annotated function, checked by `luabox check`, formatted by
-- `luabox fmt`, linted by `luabox lint`, tested by `luabox test`, and
-- run by `luabox run`. Everything you need to feel the workflow.

--- Build a friendly greeting for `name`.
---@param name string
---@return string
local function greet(name)
    return "Hello, " .. name .. ", from luabox!"
end

print(greet("world"))

return { greet = greet }
