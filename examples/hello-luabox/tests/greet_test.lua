-- Written busted-style; run with your deployment environment's own test
-- tooling (e.g. `busted` from the project root). We make src/ requirable by
-- prepending it to package.path.
package.path = "src/?.lua;" .. package.path

local hello = require("main")

describe("greet", function()
    it("addresses the caller by name", function()
        assert.equal("Hello, world, from luabox!", hello.greet("world"))
    end)

    it("is a pure function of its argument", function()
        assert.equal(hello.greet("Ada"), hello.greet("Ada"))
    end)
end)
