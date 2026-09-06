-- The negative half of the malformed-header axis: an unreadable TYPE
-- ARGUMENT on a parent whose NAME reads fine. `GenParentBase` resolves and
-- its member is inherited, so nothing here is dropped and nothing is
-- reported -- a regression that walks back into the argument list reports
-- LB0321 on this line and flips the row.
---@class GenParentBase
---@field b number

---@class GenParentChild : GenParentBase<?>

---@type GenParentChild
local c

---@param n number
local function want_number(n) end

want_number(c.b)
