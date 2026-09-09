
---@type Foo
local a
---@type Foo
local b

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(a + b)
