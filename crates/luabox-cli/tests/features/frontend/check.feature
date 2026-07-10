Feature: luabox check — annotation-driven typecheck (P0 MVP)
  SPEC.md §3 + §14 — `luabox check` typechecks the annotated subset:
  call sites of `---@param`/`---@return` functions, table literals
  against `---@class` shapes (field-level diagnostics), `---@type`
  locals, and return statements. Strictness comes from the manifest:
  `[types] strict = true` reports errors (nonzero exit), otherwise
  warnings (exit zero — warnings never fail the command).

  Scenario: an annotated call mismatch fails a strict project
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param n number
      local function double(n)
        return n * 2
      end

      double("nope")
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "error[LB0300]"

  Scenario: the same mismatch only warns without strict types
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param n number
      local function double(n)
        return n * 2
      end

      double("nope")
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "warning[LB0300]"

  Scenario: a table literal missing a required field names the field
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Point
      ---@field x number
      ---@field y number

      ---@param p Point
      local function use(p) end

      use({ x = 1 })
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0302"
    And stdout contains "missing required field `y`"

  Scenario: a table literal field the class does not declare is diagnosed
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Point
      ---@field x number
      ---@field y number

      ---@param p Point
      local function use(p) end

      use({ x = 1, y = 2, z = 3 })
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0303"
    And stdout contains "unknown field `z`"

  Scenario: a clean annotated project exits zero
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Greeter
      ---@field name string
      ---@field excited? boolean

      ---@param g Greeter
      ---@return string
      local function greet(g)
        return g.name
      end

      greet({ name = "world", excited = true })
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors, 0 warnings"

  Scenario: a parse error is reported as LB0001
    Given a project with edition "5.4"
    And a file "src/broken.lua" containing:
      """
      local = 5
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0001"

  Scenario: --format json emits machine-parseable output
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param n number
      local function double(n)
        return n * 2
      end

      double("nope")
      """
    When I run "luabox check --format json"
    Then the command fails
    And stdout is valid JSON
    And stdout contains "LB0300"
