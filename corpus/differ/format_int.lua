-- DIFFER: from=5.4 targets=5.3,5.2,5.1
-- Lowering rule: int_float — string.format("%d", …) draws a warn-tier LB0606
-- (integer/float divergence heuristic) but no rewrite; with integral values
-- the observable output must be identical everywhere.
local n = 42
print(string.format("%d items", n))
print(string.format("[%5d]", n))
print(string.format("%d/%d", 7, -3))
print(string.format("%x", 255))
print(string.format("%d", 6 * 7))
print("ok")
