-- DIFFER: from=luajit targets=5.1
-- Lowering rule: jit_ext — LuaJIT's global bit.* library maps onto the
-- __luabox_rt signed family (tobit normalization): results are signed 32-bit
-- exactly as bit.* returns them, including negatives.
print(bit.band(0xF0, 0x3C))     -- 48
print(bit.bor(-16, 1))          -- -15 (negative operand, signed result)
print(bit.bxor(0xAA, 0xFF))     -- 85
print(bit.bnot(0))              -- -1 (signed)
print(bit.lshift(1, 31))        -- -2147483648 (sign bit set)
print(bit.rshift(-1, 28))       -- 15 (logical shift of negative)
print(bit.arshift(-8, 1))       -- -4 (arithmetic shift keeps sign)
print(bit.rol(0x12345678, 8))   -- 878082066
print(bit.ror(0x12345678, 8))   -- 2014520406
print(bit.bswap(0x12345678))    -- 2018915346
print(bit.tohex(-1))            -- ffffffff
print(bit.tohex(48879, 4))      -- beef
print(bit.tobit(2^32 + 5))      -- 5 (wraps)
print("ok")
