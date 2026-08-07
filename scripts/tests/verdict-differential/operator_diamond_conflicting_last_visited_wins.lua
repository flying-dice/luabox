
---@class Base<V>
---@operator add(Base): V
---@class A : Base<number>
---@class B : Base<string>
---@class C : A, B
---@type C
local a
---@type C
local b

---@param n number
local function want_number(n) end

want_number(a + b)
