-- DIFFER: from=5.2 targets=5.1
-- Lowering rule: gotos — conditional backward goto becomes repeat/until.
local i = 0
::top::
i = i + 1
print("i", i)
if i < 5 then goto top end
print("done", i)
