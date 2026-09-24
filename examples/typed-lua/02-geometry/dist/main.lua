



local vec = require("vec")
local shapes = require("shapes")

-- Each constructor is checked against the member of Shape its `kind` picks.
local scene          = {
  { kind = "circle", centre = vec.new(0, 0), r = 1 },
  { kind = "rect", origin = vec.new(2, 2), w = 3, h = 4 },
}

local v      = vec.add(vec.new(3, 0), vec.new(0, 4))
print("length", v:len())
print("scaled", v:scale(2):len())
print("total area", string.format("%.2f", shapes.total_area(scene)))

-- T is bound to Shape by `scene`; `shapes.area` fits (s: Shape) -> number.
local big         = shapes.largest(scene, shapes.area)
if big then
  print("largest", big.kind)
end
