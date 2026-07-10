-- DIFFER: from=5.2 targets=5.1
-- Lowering rule: gotos — forward goto to a "continue" label becomes a skip
-- flag wrapping the region between goto and label.
local i = 1
while i <= 9 do
  if i % 2 == 0 then goto continue end
  print("odd", i)
  ::continue::
  i = i + 1
end
print("end", i)
