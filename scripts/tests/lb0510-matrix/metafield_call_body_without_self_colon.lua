-- The instance *is* called, and the `__call` it dispatches to reaches no
-- method -- the colon call lives in a `Cache:reset` nothing invokes. Asking
-- whether *any* attached body contains a self colon call would report this;
-- asking the `__call` body specifically does not, and `lua5.4` agrees.
---@class Cache
local Cache = {}
Cache.__call = function(self) return 1 end
function Cache:clear() self.n = 0 end
function Cache:reset() self:clear() end
local c = setmetatable({}, Cache)
print(c())
