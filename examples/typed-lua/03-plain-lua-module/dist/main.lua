


local csv = require("csv")



local function to_item(row     )
  local price          = tonumber(row[2])
  if price == nil then
    return nil
  end
  return { name = row[1], price = price }
end

local rows        = csv.parse("apple,0.5\npear,0.75\nbad,row\nplum,1.25")
local total         = 0
for line         , row      in ipairs(rows) do
  local item = to_item(row)
  if item then
    total = total + item.price
    print(string.format("%-6s %5.2f", item.name, item.price))
  else
    print("skipped line " .. line)
  end
end
print(string.format("%-6s %5.2f", "total", total))
