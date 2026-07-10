-- DIFFER: from=5.3 targets=5.2,5.1
-- Lowering rule: floor_div — a // b becomes math.floor(a / b). Negative
-- operands are the interesting cases (floor, not truncate). Operands are
-- integers with integer results so 5.3's integer printing and 5.1's
-- %.14g double printing agree.
print(7 // 2)             -- 3
print(-7 // 2)            -- -4 (floors toward -inf, not toward 0)
print(7 // -2)            -- -4
print(-7 // -2)           -- 3
print(0 // 5)             -- 0
print(100000000000 // 7)  -- 14285714285 (well below 2^53)
print(10 // 3 // 2)       -- 1 (chained, innermost first)
print((5 + 7) // 4)       -- 3 (parenthesized operand)
print("ok")
