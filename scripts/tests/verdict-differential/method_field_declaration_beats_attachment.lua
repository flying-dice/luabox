
---@class Foo
---@field m fun(): string
local F = {}
function F.m()
  return 1
end

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(F.m())
