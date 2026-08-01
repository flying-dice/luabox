-- Round-8 FP: `if false then` never runs. Literal-condition pruning only --
-- this is the one dead-code shape the collector can decide by looking.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:reset() self.n = 0 end
local c = setmetatable({}, Cache)
if false then c:reset() end
print("done")
