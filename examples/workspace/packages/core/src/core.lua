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

---Return the first element of `list`, or `default` if it is empty.
---
---NOTE (gap): `---@generic` function type parameters are broken today — `T`
---does not flow through to the return type the way it should. `first_or`'s
---result actually types as `unknown`, not `T`, so a caller that pins the
---result to a concrete `---@type` gets a real (if confusing) type error —
---see ../../../geometry/README.md for the exact `luabox check` messages
---this produces, verified against the real binary.
---@generic T
---@param list T[]
---@param default T
---@return T
function core.first_or(list, default)
    if #list > 0 then
        return list[1]
    end
    return default
end

return core
