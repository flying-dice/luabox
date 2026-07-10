-- DIFFER: from=5.4 targets=5.1
-- Lowering rule: gotos — two forward gotos to one label nest their skip-flag
-- wrappers; both skip paths and the fall-through path are exercised.
local n = 0
while n < 6 do
  n = n + 1
  if n % 2 == 0 then goto continue end
  if n % 3 == 0 then goto continue end
  print("keep", n)
  ::continue::
end
print("end", n)
