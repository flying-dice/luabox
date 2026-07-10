-- DIFFER: from=5.4 targets=5.3,5.2,5.1
-- Lowering rule: attribs — <close> becomes a __luabox_rt.close_scope pcall
-- wrapper. Two handles in one block must close in reverse declaration order,
-- interleaved correctly with the body; nil handles are ignored (5.4 permits
-- and ignores them).
local function tracker(name)
  return setmetatable({}, { __close = function() print("close", name) end })
end

do
  local a <close> = tracker("a")
  print("body 1")
  local b <close> = tracker("b")
  print("body 2")
  local quiet <close> = nil
  print("body 3")
end
print("after")
