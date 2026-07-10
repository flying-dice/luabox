-- renderer: draw ASCII shapes to stdout. Run it with `luabox run start`.
package.path = "src/?.lua;" .. package.path

local Square = require("square")

local function main()
    local square = Square.new(4)
    print("A " .. square.side .. "x" .. square.side .. " square:")
    print(square:draw())
    print("area = " .. square:area() .. ", perimeter = " .. square:perimeter())
end

main()
