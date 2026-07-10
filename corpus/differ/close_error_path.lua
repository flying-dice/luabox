-- DIFFER: from=5.4 targets=5.3,5.2,5.1
-- Lowering rule: attribs — the <close> error path: when the scope tail raises,
-- __close still runs (receiving the error object) and the error is re-raised
-- unmodified. error(msg, 0) keeps the message position-free, so the assertion
-- lands on the exact-stdout axis rather than the coarser error-class axis.
local function work()
  local h <close> = setmetatable({}, {
    __close = function(_, err)
      print("closing, err:", tostring(err))
    end,
  })
  print("before boom")
  error("boom", 0)
end

local ok, err = pcall(work)
print("caught:", ok, tostring(err))
print("after")
