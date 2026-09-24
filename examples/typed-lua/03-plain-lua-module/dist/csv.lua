-- csv: an existing plain-Lua module. It is not rewritten; csv.luah beside it
-- declares its interface, and Typed Lua code uses it through that header.
local M = {}

function M.split(line, sep)
  local fields = {}
  for field in (line .. sep):gmatch("(.-)" .. sep) do
    fields[#fields + 1] = field
  end
  return fields
end

function M.parse(text)
  local rows = {}
  for line in text:gmatch("[^\n]+") do
    rows[#rows + 1] = M.split(line, ",")
  end
  return rows
end

return M
