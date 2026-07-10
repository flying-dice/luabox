-- DIFFER: from=5.1 targets=5.1
-- Identity pair: from == to is byte-identity in luabox-lower, so this file
-- proves the harness end-to-end (resolve, run, compare) on any machine that
-- has a single Lua 5.1 — the local-dev baseline.
local t = {}
for i = 1, 5 do
  t[#t + 1] = i * i
end
print(table.concat(t, ","))

local ok, err = pcall(function()
  error("expected failure", 0)
end)
print("pcall", ok, err)
print("done")
