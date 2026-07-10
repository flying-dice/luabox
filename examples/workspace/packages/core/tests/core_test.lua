package.path = "src/?.lua;" .. package.path

local core = require("core")

describe("core", function()
    it("adds numbers", function()
        assert.equal(5, core.add(2, 3))
    end)

    it("joins strings", function()
        assert.equal("a-b-c", core.join({ "a", "b", "c" }, "-"))
    end)
end)
