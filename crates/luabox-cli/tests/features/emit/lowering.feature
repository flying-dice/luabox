Feature: Cross-dialect lowering transforms
  SPEC.md §2.1: `luabox build` rewrites constructs the ship target lacks —
  integer division, bitwise operators, `goto`, `<const>`/`<close>`, `_ENV`,
  and the LuaJIT `bit` library — emitting a tree-shaken `__luabox_rt`
  prelude only when a helper is actually used. Each rule is gated on the
  (edition, target) pair, so a transform that is unnecessary never runs.
  Divergence that cannot be reproduced faithfully is a warning (LB0606,
  LB0603); divergence that cannot be reproduced at all is a hard error
  (LB0601, LB0602, LB0603, LB0604, LB0605) and no file is written.

  # --- integer division (floor_div) ---------------------------------------

  Scenario: `//` becomes a floored division for a double-only target
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local q = 7 // 2
      print(q)
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "local q = math.floor(7 / 2)"
    And the emitted output contains no "//"

  Scenario: nested `//` lowers innermost-first without adding parentheses
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local q = 100 // 5 // 2
      print(q)
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "math.floor(math.floor(100 / 5) / 2)"

  Scenario: lowering `//` warns once about integer-semantics divergence
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local q = 7 // 2
      print(q)
      """
    When I run "luabox build"
    Then the command succeeds
    And stdout contains "LB0606"
    And stdout contains "`//` lowered to `math.floor` division for Lua 5.1"

  Scenario: a `//`-only file needs no runtime prelude
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local q = 7 // 2
      print(q)
      """
    When I run "luabox build"
    Then the command succeeds
    And the emitted output contains no "__luabox_rt"

  Scenario Outline: `//` is lowered for every double-only target
    Given a project with edition "5.4" targeting "<target>"
    And a file "src/main.lua" containing:
      """
      local q = 7 // 2
      print(q)
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "math.floor(7 / 2)"

    Examples:
      | target |
      | 5.1    |
      | 5.2    |
      | luajit |

  Scenario: a 5.3 target keeps `//` natively
    Given a project with edition "5.4" targeting "5.3"
    And a file "src/main.lua" containing:
      """
      local q = 7 // 2
      print(q)
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "7 // 2"

  # --- bitwise operators (bitops, polyfill) -------------------------------

  Scenario Outline: every bitwise operator maps onto a runtime helper
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local a, b = 5, 3
      print(<source>)
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "<lowered>"

    Examples:
      | source | lowered                |
      | a & b  | __luabox_rt.band(a, b) |
      | a ~ b  | __luabox_rt.bxor(a, b) |
      | a << b | __luabox_rt.shl(a, b)  |
      | a >> b | __luabox_rt.shr(a, b)  |
      | ~a     | __luabox_rt.bnot(a)    |

  # Bitwise OR gets its own scenario: `|` is the Gherkin Examples-table cell
  # separator, so it cannot ride the outline above.
  Scenario: bitwise OR maps onto the bor helper
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local a, b = 5, 3
      print(a | b)
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "__luabox_rt.bor(a, b)"

  Scenario: the prelude is tree-shaken to the helpers actually used
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local a = 5
      print(a & 3)
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "function M.band(a, b)"
    And "dist/src/main.lua" does not contain "function M.bor"
    And "dist/src/main.lua" does not contain "function M.close_scope"

  Scenario: `~=` is inequality, not a bitwise operator
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local a, b = 5, 3
      print(a ~= b)
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "a ~= b"
    And the emitted output contains no "__luabox_rt"

  Scenario: a 5.2 target borrows the stdlib bit32 library
    Given a project with edition "5.4" targeting "5.2"
    And a file "src/main.lua" containing:
      """
      local a = 5
      print(a & 3)
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "M.band = bit32.band"

  Scenario: a LuaJIT target borrows its native bit library
    Given a project with edition "5.4" targeting "luajit"
    And a file "src/main.lua" containing:
      """
      local a = 5
      print(a & 3)
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "return bit.band(a, b)"

  # --- integer/float divergence heuristics (int_float) --------------------

  Scenario: an integer literal beyond 2^53 warns about double precision
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local big = 9007199254740993
      print(big)
      """
    When I run "luabox build"
    Then the command succeeds
    And stdout contains "LB0606"
    And stdout contains "the integer literal `9007199254740993` exceeds 2^53"

  Scenario: an integer literal exactly at 2^53 is still exact
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local big = 9007199254740992
      print(big)
      """
    When I run "luabox build"
    Then the command succeeds
    And stdout does not contain "LB0606"

  Scenario: `string.format` with `%d` warns about integer formatting
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local n = 3
      print(string.format("count=%d", n))
      """
    When I run "luabox build"
    Then the command succeeds
    And stdout contains "LB0606"
    And stdout contains "`string.format` with `%d` behaves differently on a double-only target"

  Scenario: an escaped `%%d` is a literal percent, not a directive
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      print(string.format("100%%d"))
      """
    When I run "luabox build"
    Then the command succeeds
    And stdout does not contain "LB0606"

  # --- variable attributes (attribs) --------------------------------------

  Scenario: `<const>` is erased at zero cost
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local limit <const> = 10
      print(limit)
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "local limit = 10"
    And the emitted output contains no "<const>"
    And the emitted output contains no "__luabox_rt"

  Scenario: assigning to a `<const>` is a hard error
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local limit <const> = 10
      limit = 20
      print(limit)
      """
    When I run "luabox build"
    Then the command fails
    And diagnostic LB0602 is reported
    And stdout contains "assignment to constant `limit` (declared `<const>`)"
    And the file "dist/src/main.lua" does not exist

  Scenario: `<close>` becomes a pcall scope-exit wrapper
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local h <close> = setmetatable({}, { __close = function() end })
      print(h)
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "__luabox_rt.close_scope(h, function()"
    And "dist/src/main.lua" contains "local ok, err = pcall(body)"
    And "dist/src/main.lua" contains "mt.__close(v, err)"
    And the emitted output contains no "<close>"

  Scenario: lowering `<close>` warns that the fidelity is lossy
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local h <close> = setmetatable({}, { __close = function() end })
      print(h)
      """
    When I run "luabox build"
    Then the command succeeds
    And stdout contains "LB0603"
    And stdout contains "`<close>` lowered via a pcall scope-exit wrapper"

  Scenario: acknowledging the lossy lowering silences the warning
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      ---@luabox-allow lossy-lowering
      local h <close> = setmetatable({}, { __close = function() end })
      print(h)
      """
    When I run "luabox build"
    Then the command succeeds
    And stdout does not contain "LB0603"

  Scenario: a `<close>` sharing its `local` with other names cannot be lowered
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local a, h <close> = 1, setmetatable({}, {})
      print(a, h)
      """
    When I run "luabox build"
    Then the command fails
    And diagnostic LB0603 is reported
    And stdout contains "cannot lower `<close>` in a multi-name `local`"

  Scenario: a `return` after a `<close>` cannot cross the wrapper boundary
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local function use()
        local h <close> = setmetatable({}, {})
        return h
      end
      return use
      """
    When I run "luabox build"
    Then the command fails
    And diagnostic LB0603 is reported
    And stdout contains "which cannot cross the pcall wrapper's function boundary"

  Scenario: a 5.3 target lowers attributes but leaves 5.3 operators alone
    Given a project with edition "5.4" targeting "5.3"
    And a file "src/main.lua" containing:
      """
      local limit <const> = 10
      local q = 7 // 2
      local b = 5 & 3
      print(limit, q, b)
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "local limit = 10"
    And "dist/src/main.lua" contains "7 // 2"
    And "dist/src/main.lua" contains "5 & 3"

  # --- _ENV (env) ---------------------------------------------------------

  Scenario: a chunk-level `local _ENV` becomes setfenv
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local sandbox = { print = print }
      local _ENV = sandbox
      print("sandboxed")
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "setfenv(1, sandbox)"
    And the emitted output contains no "_ENV"

  Scenario: reading `_ENV` becomes getfenv
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local t = _ENV
      print(t)
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "local t = getfenv(1)"

  Scenario: an `_ENV` declared in a nested block is not lowerable
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      do
        local _ENV = { print = print }
        print(1)
      end
      """
    When I run "luabox build"
    Then the command fails
    And diagnostic LB0604 is reported
    And stdout contains "this `_ENV` declaration is not lowerable"

  Scenario: an `_ENV` parameter is not lowerable
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local function f(_ENV)
        return 1
      end
      return f
      """
    When I run "luabox build"
    Then the command fails
    And diagnostic LB0604 is reported
    And stdout contains "an `_ENV` parameter cannot be lowered to setfenv/getfenv"

  Scenario: `_ENV` is left alone for a target that has it
    Given a project with edition "5.4" targeting "5.3"
    And a file "src/main.lua" containing:
      """
      local sandbox = { print = print }
      local _ENV = sandbox
      print("sandboxed")
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "local _ENV = sandbox"

  # --- goto restructuring (gotos) -----------------------------------------

  Scenario: a forward goto becomes a skip flag
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      for i = 1, 3 do
        if i == 2 then goto continue end
        print(i)
        ::continue::
      end
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "local __luabox_skip_1 = false"
    And "dist/src/main.lua" contains "if not __luabox_skip_1 then"
    And the emitted output contains no "goto"

  Scenario: an unconditional backward goto becomes an infinite loop
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local function run(work)
        ::again::
        work()
        goto again
      end
      return run
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "while true do"
    And the emitted output contains no "goto"

  Scenario: an unreferenced label is deleted
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      ::dead::
      print(1)
      """
    When I run "luabox build"
    Then the command succeeds
    And the emitted output contains no "::"

  Scenario: a goto nested deeper than the label's block is irreducible
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local function f()
        do
          goto out
        end
        ::out::
        return 1
      end
      return f
      """
    When I run "luabox build"
    Then the command fails
    And diagnostic LB0601 is reported
    And stdout contains "the goto is nested deeper than an `if` branch directly in the label's block"

  Scenario: the lossy-lowering escape hatch does not apply to goto
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      ---@luabox-allow lossy-lowering
      while true do
        goto out
      end
      ::out::
      print("after")
      """
    When I run "luabox build"
    Then the command fails
    And diagnostic LB0601 is reported
    And stdout contains "`---@luabox-allow lossy-lowering` does not apply to goto"

  Scenario: goto survives untouched for a 5.2 target
    Given a project with edition "5.4" targeting "5.2"
    And a file "src/main.lua" containing:
      """
      for i = 1, 3 do
        if i == 2 then goto continue end
        print(i)
        ::continue::
      end
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "goto continue"

  # --- LuaJIT extensions (jit_ext) ----------------------------------------

  Scenario: a bit member access is rewritten onto the runtime helper
    Given a project with edition "luajit" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local x = bit.band(0xF0, 0x0F)
      print(x)
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "__luabox_rt.band(0xF0, 0x0F)"

  Scenario: a first-class reference to a bit member is rewritten too
    Given a project with edition "luajit" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local f = bit.bor
      print(f(1, 2))
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "local f = __luabox_rt.bor"

  Scenario: requiring the bit module aliases the whole runtime table
    Given a project with edition "luajit" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local bitlib = require("bit")
      print(bitlib.bor(1, 2))
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "local bitlib = __luabox_rt"
    And "dist/src/main.lua" contains "function M.tohex(x, n)"

  Scenario: ffi has nothing to polyfill and is a hard error
    Given a project with edition "luajit" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local ffi = require("ffi")
      return ffi
      """
    When I run "luabox build"
    Then the command fails
    And diagnostic LB0605 is reported
    And stdout contains "`ffi` cannot be lowered: C data has no Lua 5.1 polyfill"
    And the file "dist/src/main.lua" does not exist

  Scenario: an undocumented bit member has no polyfill
    Given a project with edition "luajit" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local x = bit.frobnicate(1)
      return x
      """
    When I run "luabox build"
    Then the command fails
    And diagnostic LB0605 is reported
    And stdout contains "`bit.frobnicate` has no polyfill"

  Scenario: a 64-bit LuaJIT number literal has no double representation
    Given a project with edition "luajit" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local n = 1LL
      return n
      """
    When I run "luabox build"
    Then the command fails
    And diagnostic LB0605 is reported
    And stdout contains "64-bit/imaginary cdata boxes have no double representation"

  Scenario: bit is native when LuaJIT is also the target
    Given a project with edition "luajit" targeting "luajit"
    And a file "src/main.lua" containing:
      """
      local x = bit.band(0xF0, 0x0F)
      print(x)
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "bit.band(0xF0, 0x0F)"
    And the emitted output contains no "__luabox_rt"
