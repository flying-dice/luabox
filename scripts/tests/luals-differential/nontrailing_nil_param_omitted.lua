---@param p number|nil
---@param q string
local function g(p, q) end

-- p is well-typed (a number), so the only way this call can be flagged is
-- the omitted, non-trailing q. g("only") would ALSO diag through a
-- p-is-a-string type mismatch even with the arity rule deleted (F20,
-- measured) -- g(1) is the isolating call.
g(1)
