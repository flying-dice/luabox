-- DIFFER: from=5.4 targets=5.3,5.2,5.1
-- Lowering rule: attribs — <const> is dropped to a plain local (reassignment
-- is a compile-time LB0602, so a clean file lowers to identical behaviour).
local limit <const> = 10
local base <const> = 100
local sum = 0
for i = 1, limit do
  sum = sum + i
end
print("sum", sum + base)
print("ok")
