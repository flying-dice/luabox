
---@class P1
---@operator add(P1): number
---@class P2
---@operator add(P2): string
---@class C : P1, P2
---@type C
local a
---@type C
local b

---@param n number
local function want_number(n) end

want_number(a + b)
