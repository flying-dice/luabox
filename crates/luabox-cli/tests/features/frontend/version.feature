Feature: luabox check — `---@version` dialect gating
  SPEC.md §3: `---@version` declares the Lua versions a function is available
  in. The body is a comma-separated list of `5.1`/`5.2`/`5.3`/`5.4`/`JIT`,
  each optionally prefixed `>` or `<` (both inclusive). Calling a function
  whose version set excludes the project edition is LB0308 — always a
  warning, even in a strict project, because the annotation describes the
  wider ecosystem rather than a defect in this code.

  Scenario: calling a function gated to another edition warns
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@version 5.2
      local function legacy() end

      legacy()
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "LB0308"
    And stdout contains "`legacy` is defined in `5.2`, current is `5.4`"

  Scenario: the mismatch names the edition the symbol is unavailable in
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@version 5.2
      local function legacy() end

      legacy()
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "`legacy` is not available in edition `5.4`"

  Scenario: a version mismatch stays a warning in a strict project
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@version 5.2
      local function legacy() end

      legacy()
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "warning[LB0308]"
    And stderr contains "check: 0 errors, 1 warnings"

  Scenario: declaring a gated function is fine — only using it is diagnosed
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@version 5.2
      local function legacy()
        return 1
      end

      return 0
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: an inclusive lower bound admits every later edition
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@version >5.3
      local function modern() end

      modern()
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: an inclusive upper bound excludes a later edition
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@version <5.2
      local function ancient() end

      ancient()
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "LB0308"

  Scenario: JIT gates a function to the luajit edition
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@version JIT
      local function jitonly() end

      jitonly()
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "`jitonly` is defined in `luajit`, current is `5.4`"

  Scenario: luajit satisfies a 5.1 gate, since it is 5.1-compatible
    Given a project with edition "luajit"
    And a file "src/main.lua" containing:
      """
      ---@version 5.1
      local function classic() end

      classic()
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: a comma-separated set admits any listed edition
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@version 5.2, 5.4
      local function either() end

      either()
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: a version gate on a table member is honoured at its call site
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      local M = {}

      ---@version 5.2
      function M.legacy() end

      M.legacy()
      return M
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "LB0308"

  Scenario: an unrecognised version name gates nothing
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@version 5.9
      local function future() end

      future()
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: `---@diagnostic disable` suppresses the version warning file-wide
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@diagnostic disable: deprecated
      ---@version 5.2
      local function legacy() end

      legacy()
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: `disable-next-line` suppresses just the one call
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@version 5.2
      local function legacy() end

      ---@diagnostic disable-next-line: deprecated
      legacy()
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported
