Feature: luabox check — unions, enums, and exhaustive dispatch
  SPEC.md §3: a union of literal types (usually behind a `---@alias`) and an
  `---@enum` table are *finite* types. Values outside the domain are LB0300;
  an `if`/`elseif` chain that dispatches on a finite type, covers only some of
  it, and has no `else` is LB0315 — the discriminated-union exhaustiveness
  check. The analysis is deliberately narrow so it never guesses: anything but
  a straight `x == <literal>` chain on one discriminant is left alone.

  # --- finite domains -----------------------------------------------------

  Scenario: a value outside a literal-union alias is a mismatch
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@alias Color "r"|"g"|"b"

      ---@param c Color
      local function paint(c) end

      paint("r")
      paint("purple")
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains 'found `"purple"`'

  Scenario: an `---@enum` names the domain its members define
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@enum Level
      local Level = { debug = 1, info = 2 }

      ---@param l Level
      local function log(l) end

      log(Level.debug)
      log(3)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "expected `Level`, found `3`"

  Scenario: a member the `---@enum` table does not define cannot be read
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@enum Level
      local Level = { debug = 1, info = 2 }
      return Level.verbose
      """
    When I run "luabox check"
    Then the command fails
    And diagnostic LB0306 is reported naming field "verbose"

  # --- exhaustiveness (LB0315) --------------------------------------------

  Scenario: an `if` chain that misses a member of a finite alias is non-exhaustive
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@alias Color "r"|"g"|"b"

      ---@param c Color
      ---@return string
      local function name(c)
        if c == "r" then
          return "red"
        elseif c == "g" then
          return "green"
        end
        return "?"
      end
      return name
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0315"
    And stdout contains 'non-exhaustive `if` on `c`: `"b"` not handled'
    And stdout contains "the chain dispatches on a finite type and has no `else`"

  Scenario: an uncovered `---@enum` case is named as `Enum.member`
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@enum Dir
      local Dir = { up = 1, down = 2, left = 3, right = 4 }

      ---@param d Dir
      local function step(d)
        if d == Dir.up then
        elseif d == Dir.down then
        end
      end
      return Dir, step
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0315"
    And stdout contains "`Dir.left`, `Dir.right` not handled"

  Scenario: covering every member makes the chain exhaustive
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@alias Color "r"|"g"

      ---@param c Color
      ---@return string
      local function name(c)
        if c == "r" then
          return "red"
        elseif c == "g" then
          return "green"
        end
        return "?"
      end
      return name
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: an `else` clause acknowledges a deliberately partial chain
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@alias Color "r"|"g"|"b"

      ---@param c Color
      ---@return string
      local function name(c)
        if c == "r" then
          return "red"
        else
          return "other"
        end
      end
      return name
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: an open-ended discriminant is not a finite dispatch
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param s string
      local function f(s)
        if s == "a" then
        elseif s == "b" then
        end
      end
      return f
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout does not contain "LB0315"

  Scenario: exhaustiveness rides the strictness ladder like its siblings
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@alias Color "r"|"g"|"b"

      ---@param c Color
      ---@return string
      local function name(c)
        if c == "r" then
          return "red"
        elseif c == "g" then
          return "green"
        end
        return "?"
      end
      return name
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "warning[LB0315]"
