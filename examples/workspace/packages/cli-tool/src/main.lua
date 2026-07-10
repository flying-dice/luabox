-- cli-tool: a small command that depends on the `core` package.
-- Make both this package's src/ and core's src/ requirable at runtime.
package.path = "src/?.lua;../core/src/?.lua;" .. package.path

local core = require("core")

local function main()
    local total = core.add(2, 3)
    print(core.join({ "2 + 3", tostring(total) }, " = "))
end

main()
