package.path = "src/?.lua;" .. package.path

-- CROSS-FILE REQUIRE (#85): each `require` below is typed from the required
-- module's annotations, so this test file is checked with the same rigor as
-- the modules themselves. `Circle`/`Rect` are their carriers' export tables;
-- `Circle.new(2)` types as `geometry.Circle` (its `---@return`), so
-- `Circle.new(2):area()` resolves the inherited `geometry.Shape` method and
-- types as `number`. A method the class does not declare —
-- `Circle.new(2):nonexistent()` — is a real `luabox check` error (LB0306) at
-- the call site here, not silently `unknown` (verified against the binary,
-- then reverted). Likewise `data.origin.x` and `data.ShapeKind.Circle` below
-- type through `shapes_data`'s export. The module `return` value evaluates to
-- the required file's export type, matching lua-language-server.
local Circle = require("circle")
local Rect = require("rect")
local data = require("shapes_data")

describe("Circle", function()
    it("area is pi r squared", function()
        assert.equal(math.pi * 4, Circle.new(2):area())
    end)

    it("perimeter is 2 pi r", function()
        assert.equal(2 * math.pi * 3, Circle.new(3):perimeter())
    end)
end)

describe("Rect", function()
    it("area is width times height", function()
        assert.equal(12, Rect.new(3, 4):area())
    end)

    it("perimeter is twice the half-perimeter", function()
        assert.equal(14, Rect.new(3, 4):perimeter())
    end)
end)

describe("shape data", function()
    it("origin sits at 0,0", function()
        assert.equal(0, data.origin.x)
        assert.equal(0, data.origin.y)
    end)

    it("optional labels are preserved", function()
        assert.equal("top-right", data.corner.label)
    end)

    it("a Pair holds both members", function()
        assert.equal(640, data.dimensions.first)
        assert.equal(480, data.dimensions.second)
    end)

    it("a point may carry its coordinate unit", function()
        assert.equal("px", data.corner.unit)
    end)

    it("ShapeKind enum members round-trip through describe_kind", function()
        assert.equal("a circle", data.describe_kind(data.ShapeKind.Circle))
        assert.equal("a rect", data.describe_kind(data.ShapeKind.Rect))
    end)
end)
