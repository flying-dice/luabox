::top::
local i = 0
while true do
  i = i + 1
  if i & 3 == 0 then goto top end
  repeat i = i // 2 until i < 1 or i ~ 5 == 0
  break
end
