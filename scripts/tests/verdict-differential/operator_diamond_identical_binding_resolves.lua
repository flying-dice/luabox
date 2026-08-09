
---@class Base<V>
---@operator add(Base): V
---@class A : Base<number>
---@class B : Base<number>
---@class C : A, B
---@type C
local a
---@type C
local b

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(a + b)
