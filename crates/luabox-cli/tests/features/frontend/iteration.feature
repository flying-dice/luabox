Feature: luabox check — typed iteration
  SPEC.md §3: a `for ... in` loop gets its variables typed from the iterator
  it drives. `pairs` yields the key/value union of whatever shape it is given
  — a constructor-inferred shape, a `---@type` table literal type, a
  `---@class`, or an indexer type — and the raw `next, t` form is understood
  as the same thing. `ipairs` and the numeric `for` type their variables too,
  and a tuple type indexes positionally.

  # --- `pairs` ------------------------------------------------------------

  Scenario: `pairs` over a constructor-inferred shape types both loop variables
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param k string
      ---@param v number
      local function want(k, v) end

      local t = { a = 1, b = 2 }
      for k, v in pairs(t) do
        want(v, k)
      end
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains exactly 2 occurrence of "LB0300"

  Scenario: `pairs` over a table literal type types both loop variables
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param k string
      ---@param v number
      local function want(k, v) end

      ---@type { a: number, b: number }
      local t = { a = 1, b = 2 }

      for k, v in pairs(t) do
        want(v, k)
      end
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains exactly 2 occurrence of "LB0300"

  Scenario: `pairs` over an indexer type takes the key and value from the indexer
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param k string
      ---@param v boolean
      local function want(k, v) end

      ---@type { [string]: boolean }
      local t

      for k, v in pairs(t) do
        want(v, k)
      end
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains exactly 2 occurrence of "LB0300"

  Scenario: `pairs` over a `---@class` value types the loop from its fields
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Conf
      ---@field a number
      ---@field b number

      ---@param k string
      ---@param v number
      local function want(k, v) end

      ---@type Conf
      local c
      for k, v in pairs(c) do
        want(v, k)
      end
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: the raw `next, t` iterator form is understood like `pairs`
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type { a: number }
      local t = { a = 1 }

      ---@param k string
      ---@param v number
      local function want(k, v) end

      for k, v in next, t do
        want(v, k)
      end
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: `pairs` over a union of shapes yields the union of their entries
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type { a: number }|{ [string]: boolean }
      local u

      ---@param n number
      local function want(n) end

      for k, v in pairs(u) do
        want(k)
      end
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: a correctly ordered `pairs` loop is clean
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type { [string]: boolean }
      local flags

      ---@param k string
      ---@param v boolean
      local function want(k, v) end

      for k, v in pairs(flags) do
        want(k, v)
      end
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  # --- numeric `for` and tuple indexing -----------------------------------

  Scenario: a numeric `for` variable is an integer
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param s string
      local function want(s) end

      for i = 1, 10 do
        want(i)
      end
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "expected `string`, found `integer`"

  Scenario: a tuple type is indexed positionally, each slot with its own type
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type [number, string]
      local pair

      ---@param s string
      local function want(s) end

      want(pair[2])
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: the wrong tuple slot is a mismatch like any other
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type [number, string]
      local pair

      ---@param s string
      local function want(s) end

      want(pair[1])
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "expected `string`, found `number`"
