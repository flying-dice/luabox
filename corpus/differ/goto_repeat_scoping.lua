-- DIFFER: from=5.2 targets=5.1
-- Lowering rule: gotos — the repeat/until rewrite leans on repeat-scoping:
-- the until condition references a local declared *inside* the backward
-- region, which repeat/until (uniquely) keeps in scope for the condition.
local total = 0
::top::
local step = total + 1
total = total + step
print("step", step)
if step < 5 then goto top end
print("total", total)
