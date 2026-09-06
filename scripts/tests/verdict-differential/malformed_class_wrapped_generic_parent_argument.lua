-- The POSITIVE counterpart to malformed_class_generic_parent_argument: the
-- same unreadable type argument, with one wrapper around it. `GenWrapBase[]`
-- is an array OF the class, not the class -- it contributes no parent at all,
-- so `b` is never inherited (LB0306 on the read) and the `?` the parser could
-- not read is reported (LB0321). Looking THROUGH the wrapper to the name
-- inside it silences both halves of that; this row is what fails when it does.
---@class GenWrapBase
---@field b number

---@class GenWrapChild : GenWrapBase<?>[]

---@type GenWrapChild
local c

---@param n number
local function want_number(n) end

want_number(c.b)
