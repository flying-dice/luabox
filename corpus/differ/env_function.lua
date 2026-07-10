-- DIFFER: from=5.2 targets=5.1
-- Lowering rule: env — `local _ENV = t` as a function-body preamble becomes
-- setfenv(1, t): subsequent global reads/writes in the function go to t.
local realprint = print
local function fill(env)
  local _ENV = env
  x = 1
  y = x + 2
end

local t = {}
fill(t)
realprint("t.x", t.x)
realprint("t.y", t.y)
realprint("global x is", tostring(x))
realprint("ok")
