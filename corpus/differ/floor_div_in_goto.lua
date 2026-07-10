-- DIFFER: from=5.3 targets=5.1
-- Cross-rule composition: a backward goto whose condition contains // — the
-- repeat/until rewrite must render its condition *through* the expression
-- rules (until not (math.floor(n / 8) > 0)).
local n = 100
::halve::
n = n // 2
print(n)
if n // 8 > 0 then goto halve end
print("final", n)
