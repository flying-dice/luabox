-- DIFFER: from=5.3 targets=5.2,5.1
-- Lowering rule: polyfill — one module using many operators (some repeatedly)
-- gets exactly one tree-shaken __luabox_rt prelude serving all of them; a
-- duplicated or clobbered prelude would corrupt these results.
local a = 0xFF & 0x0F
local b = a | 0x30
local c = b ~ 0x05
local d = c << 2
local e = d >> 1
local f = 100 // 7
local g = (a & b) | (c & d)   -- reuse of band/bor after first use
print(a, b, c, d, e, f, g)
print("ok")
