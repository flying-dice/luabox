Feature: luabox lint — type-informed lint rules (clippy analog)
  SPEC.md §9 — `luabox lint` runs type-informed rules over the shared
  parse/HIR/type machinery. Tiers set default severity: correctness denies
  (nonzero exit), suspicious/perf/style warn (exit zero), pedantic is off.
  `---@luabox-ignore rule-id reason` suppresses a rule (reason mandatory —
  a bare tag is itself a diagnostic). `[lint]` in the manifest overrides
  levels, and `--fix` applies machine-applicable fixes to disk.

  Background:
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"
      """

  Scenario: an unused local is flagged (style, warning)
    Given a file "src/main.lua" containing:
      """
      local x = 1
      return 0
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout contains "LB0501"
    And stdout contains "unused-local"

  Scenario: a missing `local` is flagged as a global write
    Given a file "src/main.lua" containing:
      """
      counter = 0
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout contains "LB0504"
    And stdout contains "global-write"

  Scenario: `---@luabox-ignore` with a reason suppresses the rule
    Given a file "src/main.lua" containing:
      """
      ---@luabox-ignore unused-local kept for a future refactor
      local x = 1
      return 0
      """
    When I run "luabox lint"
    Then the command succeeds
    And stderr contains "0 errors, 0 warnings"

  Scenario: an ignore without a reason is itself a diagnostic
    Given a file "src/main.lua" containing:
      """
      ---@luabox-ignore unused-local
      local x = 1
      return 0
      """
    When I run "luabox lint"
    Then the command fails
    And stdout contains "LB0500"

  Scenario: --fix rewrites an unused local and a second pass is clean
    Given a file "src/main.lua" containing:
      """
      local unused = 1
      return 0
      """
    When I run "luabox lint --fix"
    Then the command succeeds
    And "src/main.lua" contains "_unused"
    When I run "luabox lint"
    Then the command succeeds
    And stderr contains "0 errors, 0 warnings"

  Scenario: a `---@meta` definition file is exempt from global-write and unused-local
    Given a file "src/defs.lua" containing:
      """
      ---@meta
      local scaffold = {}
      love = {}
      """
    When I run "luabox lint"
    Then the command succeeds
    And stderr contains "0 errors, 0 warnings"

  Scenario: a [lint] allow entry silences a rule
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [lint]
      unused-local = "allow"
      """
    And a file "src/main.lua" containing:
      """
      local x = 1
      return 0
      """
    When I run "luabox lint"
    Then the command succeeds
    And stderr contains "0 errors, 0 warnings"
