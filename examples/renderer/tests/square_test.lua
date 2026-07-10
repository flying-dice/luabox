package.path = "src/?.lua;" .. package.path

local Square = require("square")

describe("Square", function()
    it("draws an NxN block of hashes", function()
        assert.equal("##\n##", Square.new(2):draw())
    end)

    it("area is side squared", function()
        assert.equal(9, Square.new(3):area())
    end)

    it("perimeter is four sides", function()
        assert.equal(12, Square.new(3):perimeter())
    end)
end)
