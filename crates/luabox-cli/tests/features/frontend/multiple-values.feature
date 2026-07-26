Feature: luabox check — varargs, multiple returns, and overload selection
  SPEC.md §3: Lua expressions can carry more than one value, and the checker
  tracks that. `---@param ... T` types every variadic argument; a multi-value
  return is destructured positionally by `local a, b = f()` and expands into a
  call's argument list; and `---@overload` declares extra signatures that are
  selected by the argument types at the call site.

  # --- varargs ------------------------------------------------------------

  Scenario: `---@param ...` types every variadic argument at the call site
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param ... number
      ---@return number
      local function sum(...)
        local total = 0
        for _, v in ipairs({ ... }) do
          total = total + v
        end
        return total
      end

      sum(1, 2, 3)
      sum("a")
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains 'expected `number`, found `"a"`'

  Scenario: a variadic function accepts any number of matching arguments
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param ... string
      local function log(...) end

      log()
      log("a")
      log("a", "b", "c")
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  # --- multiple returns ---------------------------------------------------

  Scenario: a multi-value return is destructured positionally
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@return number, string
      local function two()
        return 1, "a"
      end

      local first, second = two()
      ---@type string
      local wrong = first
      return wrong, second
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "expected `string`, found `number`"

  Scenario: a trailing call expands to fill the remaining arguments
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@return number, number
      local function two()
        return 1, 2
      end

      ---@param a number
      ---@param b number
      local function takes(a, b) end

      takes(two())
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: a function returning nothing cannot fill an annotated slot
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      local function nothing() end

      ---@type number
      local n = nothing()
      return n
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  # --- `---@overload` -----------------------------------------------------

  Scenario: an overload signature is selected by the argument type
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@overload fun(a: number): number
      ---@param a string
      ---@return string
      local function f(a)
        return a
      end

      ---@type number
      local n = f(1)
      ---@type string
      local s = f("x")
      return n, s
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: the selected overload's return type is the one that is checked
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@overload fun(a: number): number
      ---@param a string
      ---@return string
      local function f(a)
        return a
      end

      ---@type number
      local bad = f("x")
      return bad
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "expected `number`, found `string`"

  Scenario: several overloads can be declared on one function
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@overload fun(a: number): number
      ---@overload fun(a: boolean): boolean
      ---@param a string
      ---@return string
      local function f(a)
        return a
      end

      ---@type boolean
      local b = f(true)
      ---@type number
      local n = f(1)
      return b, n
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported
