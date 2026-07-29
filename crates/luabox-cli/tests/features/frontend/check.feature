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

  # --- assignment compatibility (LB0300) ----------------------------------

  Scenario: a `---@type` local initialised with the wrong type is diagnosed
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type string
      local s = 42
      return s
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "type mismatch: expected `string`, found"

  Scenario: reassigning a `---@type` local to the wrong type is diagnosed
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type number
      local n = 1
      n = "nope"
      return n
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: an integer flows into a number but not the other way round
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type number
      local widened = 2

      ---@type integer
      local narrowed = 2.5

      return widened, narrowed
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "expected `integer`, found"

  Scenario: a table shape carrying an extra field still satisfies the class
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Point
      ---@field x number
      ---@field y number

      ---@param p Point
      local function use(p) end

      use({ x = 1, y = 2 })
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  # --- argument and return arity (LB0301, LB0304) -------------------------

  Scenario: too few arguments names the expected count
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param a number
      ---@param b number
      local function f(a, b) end

      f(1)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0301"
    And stdout contains "this function takes 2 arguments but 1 was supplied"

  Scenario: too many arguments points at the first extra one
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param a number
      local function f(a) end

      f(1, 2, 3)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0301"
    And stdout contains "unexpected extra argument"

  Scenario: returning fewer values than declared is diagnosed
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@return number, number
      local function f()
        return 1
      end
      return f
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0304"
    And stdout contains "expected 2 return values but 1 was supplied"

  Scenario: returning the wrong type is reported as a return mismatch
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@return number
      local function g()
        return "no"
      end
      return g
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0304"
    And stdout contains "return type mismatch: expected `number`, found"

  # --- annotation lowering (LB0305, LB0314) -------------------------------

  Scenario: an unknown type name in an annotation is diagnosed
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param x Wibble
      local function f(x) end
      return f
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0305"
    And stdout contains "unknown type name `Wibble` in annotation"
    And stdout contains "not a built-in, `---@class`, `---@alias`, or `---@enum` name"

  Scenario: a `---@class` naming an unknown parent is diagnosed
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Circle: Wibble
      local Circle = {}
      return Circle
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0305"

  Scenario: a self-referential alias is reported once, at its declaration
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@alias Loop Loop

      ---@param a Loop
      ---@param b Loop
      local function f(a, b) end
      return f
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0314"
    And stdout contains "`Loop` is a cyclic `---@alias`"
    And stdout contains "refers to itself, directly or through other aliases"

  Scenario: two aliases that define each other are a cycle too
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@alias A B
      ---@alias B A

      ---@param x A
      local function f(x) end
      return f
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0314"

  # --- `---@diagnostic` suppression ---------------------------------------

  Scenario: reading a field the class does not declare is diagnosed
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Point
      ---@field x number
      local Point = {}

      ---@param p Point
      local function use(p)
        return p.z
      end
      return Point, use
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0306"
    And stdout contains "undefined field `z` on `Point`"

  Scenario: `---@diagnostic disable-next-line` silences that one read
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Point
      ---@field x number
      local Point = {}

      ---@param p Point
      local function use(p)
        ---@diagnostic disable-next-line: undefined-field
        return p.z
      end
      return Point, use
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  # --- report formats and exit codes --------------------------------------

  Scenario: warnings alone never fail the command
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
    And stderr contains "check: 0 errors, 1 warnings in 1 files"

  Scenario: --format sarif emits a SARIF 2.1.0 report
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param n number
      local function double(n)
        return n * 2
      end

      double("nope")
      """
    When I run "luabox check --format sarif"
    Then the command fails
    And stdout is valid JSON
    And stdout contains "sarif-schema-2.1.0.json"
    And stdout contains "physicalLocation"

  Scenario: --format github emits workflow-command annotations
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param n number
      local function double(n)
        return n * 2
      end

      double("nope")
      """
    When I run "luabox check --format github"
    Then the command fails
    And stdout contains "::error file=src/main.lua,line=6,col=8::LB0300"

  Scenario: --format gitlab emits a code-quality report with a fingerprint
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param n number
      local function double(n)
        return n * 2
      end

      double("nope")
      """
    When I run "luabox check --format gitlab"
    Then the command fails
    And stdout is valid JSON
    And stdout contains "check_name"
    And stdout contains "fingerprint"

  # GitLab places a finding on the merge-request diff by
  # `location.lines.begin` and drops it when that line is not part of the
  # diff. A report that pins every finding to line 1 therefore parses, looks
  # plausible, and annotates nothing — so the lines are asserted per finding.
  Scenario: --format gitlab places each finding on its own source line
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param n number
      local function double(n)
        return n * 2
      end

      double("nope")
      double("also nope")
      """
    When I run "luabox check --format gitlab"
    Then the command fails
    And stdout is valid JSON
    And the gitlab report places a finding for "src/main.lua" on line 6
    And the gitlab report places a finding for "src/main.lua" on line 7
    And no gitlab finding sits on line 1

  # `--format` is a closed set clap owns (a `ValueEnum`), so an unknown one is
  # a malformed invocation — exit 2 with the possible values, like every other
  # bad flag value, rather than a hand-rolled message on the command's own
  # error path.
  Scenario: an unknown --format is a usage error listing the supported ones
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      return 1
      """
    When I run "luabox check --format xml"
    Then the command exits with code 2
    And stderr contains "invalid value 'xml' for '--format <FORMAT>'"
    And stderr contains "[possible values: human, json, sarif, github, gitlab]"

  Scenario: an unknown --target lists the supported dialects
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      return 1
      """
    When I run "luabox check --target 6.0"
    Then the command fails
    And stdout contains "LB1001"
    And stdout contains "unknown target `6.0`; expected one of: 5.1, 5.2, 5.3, 5.4, luajit"

  Scenario: the summary line always reports the file count
    Given a project with edition "5.4"
    And a file "src/a.lua" containing:
      """
      return 1
      """
    And a file "src/b.lua" containing:
      """
      return 2
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors, 0 warnings in 2 files"

  Scenario: a project without a manifest checks on the default edition
    Given an empty directory
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
    And stderr contains "check: 0 errors, 1 warnings in 1 files"
