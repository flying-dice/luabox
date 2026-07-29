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

  Scenario: a typo'd [lint] rule id is reported instead of silently doing nothing
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [lint]
      unused-locl = "allow"
      """
    And a file "src/main.lua" containing:
      """
      local x = 1
      return 0
      """
    When I run "luabox lint"
    # A warning, not an error: the entry is inert, so the exit code is unchanged.
    Then the command succeeds
    And stdout contains "LB1004"
    And stdout contains "unknown lint rule id `unused-locl` in `[lint]`"
    And stdout contains "did you mean `unused-local`?"
    And stdout contains "this `[lint]` entry has no effect"
    # ...and the rule the entry failed to silence is still firing.
    And stdout contains "LB0501"

  Scenario: a typo'd [lint] tier name is nudged back at the tier
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [lint]
      pedantics = "warn"
      """
    And a file "src/main.lua" containing:
      """
      return 0
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout contains "unknown lint rule id `pedantics` in `[lint]`"
    And stdout contains "did you mean `pedantic`?"

  Scenario: a correctly spelled [lint] rule id produces no config warning
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
    And stdout does not contain "LB1004"
    And stderr contains "0 errors, 0 warnings"

  Scenario: a typo'd global read is flagged as undefined-global
    Given a file "src/main.lua" containing:
      """
      prnit("hello")
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout contains "LB0509"
    And stdout contains "undefined-global"

  Scenario: a [lint] globals allow-list entry silences undefined-global
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [lint]
      globals = ["acme"]
      """
    And a file "src/main.lua" containing:
      """
      acme.init()
      """
    When I run "luabox lint"
    Then the command succeeds
    And stderr contains "0 errors, 0 warnings"

  # --- shadowed-local (suspicious, LB0503) --------------------------------

  Scenario: a local shadowing an enclosing binding is flagged
    Given a file "src/main.lua" containing:
      """
      local value = 1
      local function outer()
        local value = 2
        return value
      end
      return value, outer
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout contains "LB0503"
    And stdout contains "`value` shadows a binding from an enclosing scope"

  Scenario: the shadow diagnostic points at the outer declaration too
    Given a file "src/main.lua" containing:
      """
      local value = 1
      local function outer()
        local value = 2
        return value
      end
      return value, outer
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout contains "outer `value` declared here"

  Scenario: re-declaring a local in the same block is idiomatic, not a shadow
    Given a file "src/main.lua" containing:
      """
      local function same()
        local x = 1
        local x = x + 1
        return x
      end
      return same
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout does not contain "LB0503"

  # --- empty-then (suspicious, LB0508) ------------------------------------

  Scenario: an `if` branch with an empty body is flagged
    Given a file "src/main.lua" containing:
      """
      local t = {}
      if t then
      end
      return t
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout contains "LB0508"
    And stdout contains "this `if` branch has an empty body"

  Scenario: an empty `elseif` branch is flagged as well
    Given a file "src/main.lua" containing:
      """
      local t = {}
      if t then
        return 1
      elseif t then
      end
      return t
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout contains "LB0508"

  Scenario: a comment in the branch documents the intent and silences the rule
    Given a file "src/main.lua" containing:
      """
      local t = {}
      if t then
        -- deliberately nothing: the caller already handled it
      end
      return t
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout does not contain "LB0508"

  # --- explicit-nil-compare-truthiness (style, LB0505) --------------------

  Scenario: `x ~= nil` on a type that cannot be false is flagged
    Given a file "src/main.lua" containing:
      """
      ---@param name string
      local function greet(name)
        if name ~= nil then
          return name
        end
        return ""
      end
      return greet
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout contains "LB0505"
    And stdout contains "`name ~= nil` is equivalent to `name` here"

  Scenario: `x == nil` is reported as the negated truthiness test
    Given a file "src/main.lua" containing:
      """
      ---@param name string
      local function shout(name)
        if name == nil then
          return ""
        end
        return name
      end
      return shout
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout contains "`name == nil` is equivalent to `not name` here"

  Scenario: a boolean can be false, so its nil comparison is not equivalent
    Given a file "src/main.lua" containing:
      """
      ---@param flag boolean
      local function pick(flag)
        if flag ~= nil then
          return 1
        end
        return 0
      end
      return pick
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout does not contain "LB0505"

  Scenario: an unannotated binding has an unknown type, so the rule stays silent
    Given a file "src/main.lua" containing:
      """
      local function guard(value)
        if value ~= nil then
          return value
        end
        return nil
      end
      return guard
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout does not contain "LB0505"

  Scenario: --fix rewrites the nil comparison to plain truthiness
    Given a file "src/main.lua" containing:
      """
      ---@param name string
      local function greet(name)
        if name ~= nil then
          return name
        end
        return ""
      end
      return greet
      """
    When I run "luabox lint --fix"
    Then the command succeeds
    And "src/main.lua" contains "if name then"
    And "src/main.lua" does not contain "~= nil"

  # --- pairs-on-array (perf, LB0507) --------------------------------------

  Scenario: `pairs` over an array-typed value is flagged
    Given a file "src/main.lua" containing:
      """
      ---@param items string[]
      local function each(items)
        for _, v in pairs(items) do
          print(v)
        end
      end
      return each
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout contains "LB0507"
    And stdout contains "`pairs` over an array hashes keys and loses order; use `ipairs`"

  Scenario: `pairs` over a positional-only table literal is flagged
    Given a file "src/main.lua" containing:
      """
      local function lit()
        for _, v in pairs({ 1, 2, 3 }) do
          print(v)
        end
      end
      return lit
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout contains "LB0507"

  Scenario: `pairs` over a string-keyed table is the right iterator
    Given a file "src/main.lua" containing:
      """
      ---@param lookup table<string, string>
      local function each(lookup)
        for k, v in pairs(lookup) do
          print(k, v)
        end
      end
      return each
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout does not contain "LB0507"

  Scenario: --fix rewrites `pairs` to `ipairs`
    Given a file "src/main.lua" containing:
      """
      ---@param items string[]
      local function each(items)
        for _, v in pairs(items) do
          print(v)
        end
      end
      return each
      """
    When I run "luabox lint --fix"
    Then the command succeeds
    And "src/main.lua" contains "in ipairs(items) do"

  # --- concat-in-loop (perf, LB0506) --------------------------------------

  Scenario: growing a loop-carried string with `..` is flagged as quadratic
    Given a file "src/main.lua" containing:
      """
      local function join(items)
        local out = ""
        for i = 1, #items do
          out = out .. items[i]
        end
        return out
      end
      return join
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout contains "LB0506"
    And stdout contains "string built with `..` in a loop is quadratic"

  Scenario: the rule suggests table.concat rather than offering a fix
    Given a file "src/main.lua" containing:
      """
      local function join(items)
        local out = ""
        for i = 1, #items do
          out = out .. items[i]
        end
        return out
      end
      return join
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout contains "join once with `table.concat`"

  Scenario: an accumulator declared inside the loop is not loop-carried
    Given a file "src/main.lua" containing:
      """
      local function inner(items)
        for i = 1, #items do
          local part = ""
          part = part .. items[i]
          print(part)
        end
      end
      return inner
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout does not contain "LB0506"

  # --- unused-param (pedantic, LB0502) ------------------------------------

  Scenario: unused-param is a pedantic rule and is off by default
    Given a file "src/main.lua" containing:
      """
      ---@param a number
      ---@param b number
      local function add(a, b)
        return a
      end
      return add
      """
    When I run "luabox lint"
    Then the command succeeds
    And stderr contains "0 errors, 0 warnings"

  Scenario: enabling the pedantic tier turns unused-param on
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [lint]
      pedantic = "warn"
      """
    And a file "src/main.lua" containing:
      """
      ---@param a number
      ---@param b number
      local function add(a, b)
        return a
      end
      return add
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout contains "LB0502"
    And stdout contains "unused parameter `b`"

  Scenario: an `_`-prefixed parameter is deliberately unused
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [lint]
      pedantic = "warn"
      """
    And a file "src/main.lua" containing:
      """
      ---@param _unused number
      local function skip(_unused) end
      return skip
      """
    When I run "luabox lint"
    Then the command succeeds
    And stderr contains "0 errors, 0 warnings"

  Scenario: the implicit `self` of a method is exempt
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [lint]
      pedantic = "warn"
      """
    And a file "src/main.lua" containing:
      """
      local M = {}
      function M:tag()
        return "M"
      end
      return M
      """
    When I run "luabox lint"
    Then the command succeeds
    And stderr contains "0 errors, 0 warnings"

  Scenario: --fix prefixes an unused parameter with an underscore
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [lint]
      pedantic = "warn"
      """
    And a file "src/main.lua" containing:
      """
      ---@param a number
      ---@param b number
      local function add(a, b)
        return a
      end
      return add
      """
    When I run "luabox lint --fix"
    Then the command succeeds
    And "src/main.lua" contains "local function add(a, _b)"
    And stderr contains "(1 fixed)"

  # --- [lint] level resolution -------------------------------------------

  Scenario: a [lint] tier set to deny fails the command
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [lint]
      suspicious = "deny"
      """
    And a file "src/main.lua" containing:
      """
      local value = 1
      local function outer()
        local value = 2
        return value
      end
      return value, outer
      """
    When I run "luabox lint"
    Then the command fails
    And stdout contains "error[LB0503]"
    And stderr contains "1 errors, 0 warnings"

  Scenario: a rule-id entry is more specific than its tier entry
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [lint]
      suspicious = "deny"
      shadowed-local = "allow"
      """
    And a file "src/main.lua" containing:
      """
      local value = 1
      local function outer()
        local value = 2
        return value
      end
      return value, outer
      """
    When I run "luabox lint"
    Then the command succeeds
    And stderr contains "0 errors, 0 warnings"

  Scenario: a tier set to allow silences every rule in it
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [lint]
      perf = "allow"
      """
    And a file "src/main.lua" containing:
      """
      local function join(items)
        local out = ""
        for i = 1, #items do
          out = out .. items[i]
        end
        return out
      end
      return join
      """
    When I run "luabox lint"
    Then the command succeeds
    And stderr contains "0 errors, 0 warnings"

  Scenario: the style tier can be promoted to deny as well
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [lint]
      style = "deny"
      """
    And a file "src/main.lua" containing:
      """
      ---@param name string
      local function greet(name)
        if name ~= nil then
          return name
        end
        return ""
      end
      return greet
      """
    When I run "luabox lint"
    Then the command fails
    And stdout contains "error[LB0505]"

  Scenario: an unknown tier or level keyword is rejected by the manifest
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [lint]
      unused-local = "shout"
      """
    And a file "src/main.lua" containing:
      """
      local x = 1
      return 0
      """
    When I run "luabox lint"
    Then the command fails
    And stderr contains "unknown lint level for `unused-local` `shout` (valid: allow, warn, deny)"

  Scenario: every finding names its rule id and the ignore that would silence it
    Given a file "src/main.lua" containing:
      """
      local t = {}
      if t then
      end
      return t
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout contains "empty-then lint (suspicious); silence with `---@luabox-ignore empty-then <reason>`"

  # --- --fix mechanics ----------------------------------------------------

  Scenario: --fix applies every machine-applicable fix in one run
    Given a file "src/main.lua" containing:
      """
      local unused_one = 1
      local unused_two = 2
      return 0
      """
    When I run "luabox lint --fix"
    Then the command succeeds
    And "src/main.lua" equals:
      """
      local _unused_one = 1
      local _unused_two = 2
      return 0
      """
    And stderr contains "(1 fixed)"

  Scenario: --fix never rewrites a file that does not parse
    Given a file "src/broken.lua" containing:
      """
      local t = { 1, 2
      """
    When I run "luabox lint --fix"
    Then the command fails
    And stdout contains "LB0001"
    And "src/broken.lua" equals:
      """
      local t = { 1, 2
      """
    And stderr contains "(0 fixed)"

  Scenario: --fix reports nothing fixed when every finding is advisory
    Given a file "src/main.lua" containing:
      """
      local parts = ""
      for i = 1, 10 do
        parts = parts .. i
      end
      return parts
      """
    When I run "luabox lint --fix"
    Then the command succeeds
    And stderr contains "(0 fixed)"

  Scenario: --fix converges — a second run has nothing left to do
    Given a file "src/main.lua" containing:
      """
      local unused_one = 1
      return 0
      """
    And I run "luabox lint --fix"
    When I run "luabox lint --fix"
    Then the command succeeds
    And stderr contains "(0 fixed)"
    And "src/main.lua" equals:
      """
      local _unused_one = 1
      return 0
      """

  # --- the known-globals baseline -----------------------------------------

  Scenario: a test file may use the busted-style harness globals
    Given a file "tests/spec.lua" containing:
      """
      describe("a thing", function()
        it("works", function() end)
      end)
      return 0
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout does not contain "LB0509"

  Scenario Outline: the test-file conventions all widen the globals baseline
    Given a file "<path>" containing:
      """
      before_each(function() end)
      after_each(function() end)
      test("x", function() end)
      return 0
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout does not contain "LB0509"

    Examples: SPEC.md §11 test-file shapes
      | path                    |
      | tests/spec.lua          |
      | src/tests/deep/spec.lua |
      | src/thing_test.lua      |
      | src/thing.test.lua      |

  Scenario: an ordinary source file gets no such exemption
    Given a file "src/notatest.lua" containing:
      """
      describe("a thing", function() end)
      return 0
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout contains "LB0509"
    And stdout contains "read of undefined global `describe`"

  Scenario: a global declared by a `[types] defs` package is known to lint
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [types]
      defs = ["mylib"]
      """
    And a file "defs/mylib.d.lua" containing:
      """
      ---@meta

      ---@type fun(name: string)
      ambient_helper = nil
      """
    And a file "src/main.lua" containing:
      """
      ambient_helper("x")
      return 0
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout does not contain "LB0509"

  Scenario: a defs package may be a directory of `.d.lua` files
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [types]
      defs = ["mylib"]
      """
    And a file "defs/mylib/one.d.lua" containing:
      """
      ---@meta

      ---@type fun()
      helper_one = nil
      """
    And a file "defs/mylib/nested/two.d.lua" containing:
      """
      ---@meta

      ---@type fun()
      helper_two = nil
      """
    And a file "src/main.lua" containing:
      """
      helper_one()
      helper_two()
      return 0
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout does not contain "LB0509"

  Scenario: a defs package that cannot be resolved falls back to the stdlib baseline
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [types]
      defs = ["missing-package"]
      """
    And a file "src/main.lua" containing:
      """
      print(pairs)
      undeclared_thing()
      return 0
      """
    When I run "luabox lint"
    Then the command succeeds
    And stdout contains "read of undefined global `undeclared_thing`"
