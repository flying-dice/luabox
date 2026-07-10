-- Tests run on whatever Lua interpreter `luabox test` resolves (your PATH
-- lua, a pinned toolchain, or LUABOX_LUA). We make src/ requirable by
-- prepending it to package.path — the runner launches from the project root.
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
