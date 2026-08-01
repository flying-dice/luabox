-- `Store` and `Cache` both declare `get`. The only `:get()` in the file lands
-- on a `Store` instance, and `Store` is wired -- `Cache` must not borrow it.
---@class Cache
local Cache = {}
Cache.__mode = "k"
function Cache:get() return 1 end

---@class Store
local Store = {}
Store.__index = Store
function Store:get() return 2 end

local s = setmetatable({}, Store)
local c = setmetatable({}, Cache)
print(s:get(), type(c))
