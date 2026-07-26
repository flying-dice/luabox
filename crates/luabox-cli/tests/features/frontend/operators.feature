Feature: luabox check — operator result types
  SPEC.md §3: every Lua operator has a result type the checker knows, so an
  expression never degrades to `any` just because an operator appears in it.
  Comparisons and `not` yield `boolean`, `#` yields `integer`, `..` yields
  `string`, the bitwise operators yield `integer`. A `---@class` can declare
  its own results with `---@operator`, which is how metamethod-backed
  arithmetic on user types typechecks.

  # --- built-in operator results ------------------------------------------

  Scenario Outline: <operator> produces a `<result>`
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type <result>
      local ok = <expression>
      ---@type <wrong>
      local bad = <expression>
      return ok, bad
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "expected `<wrong>`"

    Examples: comparison, logical, length, concatenation, bitwise
      | operator          | expression | result  | wrong   |
      | a `<` comparison  | 1 < 2      | boolean | number  |
      | `not`             | not 1      | boolean | number  |
      | `#` on a table    | #{ 1, 2 }  | integer | string  |
      | `#` on a string   | #"abc"     | integer | string  |
      | `..`              | 1 .. "a"   | string  | number  |
      | a bitwise `&`     | 3 & 1      | integer | string  |
      | a bitwise `~`     | 3 ~ 1      | integer | string  |
      | a shift           | 1 << 4     | integer | string  |

  Scenario: an arithmetic result is not a string, however it is spelled
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type string
      local bad = 3 // 2
      return bad
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  # --- `---@operator` on a class ------------------------------------------

  Scenario: an `---@operator call` makes a class value callable with a known result
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Callable
      ---@operator call(number): string

      ---@type Callable
      local c
      ---@type string
      local s = c(1)
      return s
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: unary `---@operator` declarations type `#` and `-` on a class
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Vec
      ---@operator len: number
      ---@operator unm: Vec
      ---@operator concat(Vec): Vec

      ---@type Vec
      local v
      ---@type number
      local n = #v
      ---@type Vec
      local neg = -v
      ---@type Vec
      local joined = v .. v
      return n, neg, joined
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: the full arithmetic family can be declared on one class
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class M
      ---@operator sub(M): M
      ---@operator div(M): number
      ---@operator idiv(M): M
      ---@operator mod(M): M
      ---@operator pow(M): M

      ---@type M
      local a
      ---@type M
      local b
      ---@type M
      local difference = a - b
      ---@type number
      local ratio = a / b
      ---@type M
      local remainder = a % b
      return difference, ratio, remainder
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: an undeclared operator result does not silently become the class
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Vec
      ---@operator add(Vec): Vec

      ---@type Vec
      local a
      ---@type Vec
      local b
      ---@type Vec
      local bad = a / b
      return bad
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
