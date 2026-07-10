-- core: a tiny shared library used by other packages in the workspace.

local core = {}

---Add two numbers.
---@param a number
---@param b number
---@return number
function core.add(a, b)
    return a + b
end

---Join a list of strings with a separator.
---@param parts string[]
---@param sep string
---@return string
function core.join(parts, sep)
    return table.concat(parts, sep)
end

return core
