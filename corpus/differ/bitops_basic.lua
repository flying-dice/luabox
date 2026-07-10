-- DIFFER: from=5.3 targets=5.2,5.1
-- Lowering rule: bitops — & | ~ (binary xor) and unary ~ become __luabox_rt
-- helper calls (bit32 backend on 5.2, pure-Lua bitpair core on 5.1).
--
-- Value ranges: negative operands and >31-bit results are covered. All
-- results stay within unsigned 32 bits: the shims are 32-bit by design
-- (documented LB0606 caveat), so unary ~ is masked with & 0xFFFFFFFF to pin
-- the observable value to the shim's 32-bit domain.
print(0xF0 & 0x3C)             -- 48
print(0xF0 | 0x0F)             -- 255
print(0xAA ~ 0xFF)             -- 85
print(~5 & 0xFFFFFFFF)         -- 4294967290 (bnot nested inside band)
print(-256 & 0xFFF)            -- 3840 (negative operand)
print(-1 & 0xFFFFFFFF)         -- 4294967295 (all bits, negative operand)
print(0x80000000 | 1)          -- 2147483649 (>31-bit result)
print(0xFFFFFFFF ~ 0xFF)       -- 4294967040 (>31-bit result)
print((0xF0 & 0x3C) | (0x0F ~ 0x05)) -- 58 (nested association)
print("ok")
