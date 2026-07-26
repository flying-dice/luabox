Feature: luabox check — flow narrowing and explicit type overrides
  SPEC.md §3: the checker follows control flow, so a value's type inside a
  branch is narrower than its declared type. `type(x) == "..."`, plain
  truthiness, `x ~= nil` and `or`-defaulting all narrow; the narrowing is
  undone when the branches merge. Where flow cannot prove what the author
  knows, `---@cast` (statement form) and `--[[@as T]]` (expression form)
  override the inferred type outright.

  # --- `type(x) == "..."` -------------------------------------------------

  Scenario: a `type(x) ==` test narrows the union inside the branch
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param x string|number
      ---@return string
      local function f(x)
        if type(x) == "string" then
          return x
        end
        return tostring(x)
      end
      return f
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: the narrowed type is the one that flows on, not the declared union
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param x string|number
      local function f(x)
        if type(x) == "string" then
          ---@type number
          local n = x
          return n
        end
        return 0
      end
      return f
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "expected `number`, found `string`"

  Scenario: the narrowing is undone once the branches merge again
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param s string
      local function want(s) end

      ---@param x string|number
      local function f(x)
        if type(x) == "string" then
          want(x)
        end
        want(x)
      end
      return f
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains exactly 1 occurrence of "LB0300"
    And stdout contains "expected `string`, found `string|number`"

  # --- nil and truthiness -------------------------------------------------

  Scenario: an optional parameter that is never narrowed cannot satisfy a non-optional return
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param x string?
      ---@return string
      local function f(x)
        return x
      end
      return f
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0304"
    And stdout contains "expected `string`, found `string|nil`"

  Scenario Outline: <shape> removes `nil` from an optional
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param x string?
      ---@return string
      local function f(x)
        <body>
      end
      return f
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

    Examples: the narrowing forms the checker understands
      | shape             | body                                   |
      | a truthiness test | if x then return x end return ""       |
      | an explicit `~= nil` test | if x ~= nil then return x end return "" |
      | an `or` default   | return x or "default"                  |

  # --- `---@cast` ---------------------------------------------------------

  Scenario: `---@cast` replaces the flow-inferred type for the rest of the block
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param s string
      local function want(s) end

      ---@param x string|number
      local function f(x)
        ---@cast x string
        want(x)
      end
      return f
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: a `---@cast` is taken at its word, so a later mismatch is still reported
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param x string|number
      ---@return number
      local function f(x)
        ---@cast x string
        return x
      end
      return f
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0304"
    And stdout contains "expected `number`, found `string`"

  Scenario: `---@cast x -nil` subtracts a member rather than replacing the type
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param x string|nil
      ---@return string
      local function f(x)
        ---@cast x -nil
        return x
      end
      return f
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  # --- `--[[@as T]]` ------------------------------------------------------

  Scenario: an inline `--[[@as T]]` retypes a single expression at its use site
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param n number
      local function f(n) end

      local raw = "5"
      f(raw --[[@as number]])
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: an inline `--[[@as T]]` that disagrees with the slot is still checked
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param n number
      local function f(n) end

      local raw = 5
      f(raw --[[@as string]])
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "expected `number`, found `string`"

  # --- compound conditions ------------------------------------------------

  Scenario: an `and` condition narrows both of its operands in the true branch
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param a string?
      ---@param b string?
      ---@return string
      local function f(a, b)
        if a and b then
          return a .. b
        end
        return ""
      end
      return f
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: an `or` condition narrows both operands in the *false* branch
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param a string?
      ---@return string
      local function f(a)
        if a == nil or #a == 0 then
          return ""
        end
        return a
      end
      return f
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: `not` inverts which branch the narrowing applies to
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param a string?
      ---@return string
      local function f(a)
        if not a then
          return ""
        end
        return a
      end
      return f
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported
