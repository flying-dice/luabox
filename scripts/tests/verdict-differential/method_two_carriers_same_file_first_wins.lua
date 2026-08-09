
---@class Foo
local F1 = {}
function F1:m()
  return 1
end

---@class Foo
local F2 = {}
function F2:m()
  return "s"
end

---@type Foo
local f

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(f:m())
