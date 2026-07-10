-- timemachine: write Lua 5.4, ship Lua 5.1.
--
-- Every modern feature below is LOWERED by `luabox build` / `luabox bundle`
-- to run unchanged on a stock Lua 5.1 interpreter — no 5.4 runtime required.
-- The manifest declares `edition = "5.4"` (what we write) and
-- `[build] target = "5.1"` (what we ship).

-- Integer division `//` (5.3+)  →  lowered to `math.floor(a / b)`.
---@param lo integer
---@param hi integer
---@return integer
local function midpoint(lo, hi)
    return (lo + hi) // 2
end

-- Bitwise operators (5.3+)  →  lowered to a tree-shaken `__luabox_rt` shim.
local READ = 1 << 0
local WRITE = 1 << 1
local EXEC = 1 << 2

---@param flags integer
---@param bit integer
---@return boolean
local function has(flags, bit)
    return (flags & bit) ~= 0
end

-- `<close>` to-be-closed variable (5.4)  →  lowered to a pcall scope wrapper
-- that invokes `__close` on scope exit.
---@param label string
local function scope_logger(label)
    return setmetatable({ label = label }, {
        __close = function(self)
            print("[close] " .. self.label)
        end,
    })
end

local function run()
    local _guard <close> = scope_logger("run")

    print("midpoint(0, 9) = " .. midpoint(0, 9))

    local perms = READ | WRITE
    print("READ?  " .. tostring(has(perms, READ)))
    print("WRITE? " .. tostring(has(perms, WRITE)))
    print("EXEC?  " .. tostring(has(perms, EXEC)))

    -- goto / labels (5.2+)  →  lowered to a `repeat ... until` back-edge.
    local sum = 0
    local i = 0
    ::again::
    i = i + 1
    sum = sum + i
    if i < 5 then
        goto again
    end
    print("sum(1..5) = " .. sum)
end

run()
