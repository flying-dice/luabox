-- Circle is a class carrier: a table with an __index metatable whose methods
-- make it a `geometry.Shape` *structurally* — there is no binding tag and no
-- conformance declaration. The constructor's `---@return geometry.Circle` is
-- where the instance literal is checked against the type (try removing
-- `radius` from the literal to see the error), and the test suite asserts
-- Shape conformance positionally with a plain `---@type geometry.Shape`.
local Circle = {}
Circle.__index = Circle

---@return number
function Circle:area()
    return math.pi * self.radius ^ 2
end

---@return number
function Circle:perimeter()
    return 2 * math.pi * self.radius
end

---@param radius number
---@return geometry.Circle
function Circle.new(radius)
    return setmetatable({ radius = radius }, Circle)
end

-- Positional conformance assertion (SHAPES-V2.md): a `---@type` binding is
-- an assertion by construction. Delete `Circle:perimeter` above and this is
-- the line where `luabox check` reports it — naming the missing member.
---@type geometry.Shape
local _ = Circle

return Circle
