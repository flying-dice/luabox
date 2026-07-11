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
---This is a REAL `---@generic` function (#84): wherever its signature is in
---scope, `T` is inferred from the argument types at the call site and flows
---through to the return type — `first_or({ 1, 2 }, 0)` types as `number`,
---`first_or(names, "?")` as `string`. Pin the result to a concrete `---@type`
---and it checks. This matches lua-language-server's `---@generic` semantics
---(ecosystem parity, verified against the real binary — see
---../../../geometry/README.md). (Cross-*package* signature sharing is a
---separate epic (#108); across packages the result is still `unknown` until
---that lands. Generic inference itself is done.)
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
