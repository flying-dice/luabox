package.path = "src/?.lua;" .. package.path

local core = require("core")

describe("core", function()
    it("adds numbers", function()
        assert.equal(5, core.add(2, 3))
    end)

    it("joins strings", function()
        assert.equal("a-b-c", core.join({ "a", "b", "c" }, "-"))
    end)

    it("first_or returns the first element when present", function()
        assert.equal("a", core.first_or({ "a", "b" }, "z"))
    end)

    it("first_or falls back to default when the list is empty", function()
        assert.equal("z", core.first_or({}, "z"))
    end)
end)
