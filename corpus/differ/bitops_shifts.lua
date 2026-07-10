-- DIFFER: from=5.3 targets=5.2,5.1
-- Lowering rule: bitops — << and >> become __luabox_rt.shl/shr with Lua 5.3
-- shift semantics (negative counts shift the other way). Counts stay within
-- |n| < 32 and operands within 32 bits: the shims are 32-bit by design.
print(1 << 4)            -- 16
print(1 << 31)           -- 2147483648 (>31-bit result)
print(0xF << 28)         -- 4026531840 (>31-bit result)
print(0x80000000 >> 31)  -- 1
print(0xFFFFFFFF >> 4)   -- 268435455
print(5 << -2)           -- 1 (negative count == shift right)
print(3 >> -3)           -- 24 (negative count == shift left)
print(1 << 0)            -- 1
print(0xDEADBEEF >> 16)  -- 57005
print("ok")
