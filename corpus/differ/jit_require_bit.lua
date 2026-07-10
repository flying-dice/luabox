-- DIFFER: from=luajit targets=5.1
-- Lowering rule: jit_ext — `local bit = require("bit")` becomes a binding to
-- the whole __luabox_rt table (no member-level shaking once the module table
-- escapes into a local); calls through the local behave as under LuaJIT.
local b = require("bit")
print(b.band(0xFF, 0x0F))   -- 15
print(b.bor(0x10, 0x01))    -- 17
print(b.tohex(48879))       -- 0000beef
print(b.tobit(4294967295))  -- -1
print("ok")
