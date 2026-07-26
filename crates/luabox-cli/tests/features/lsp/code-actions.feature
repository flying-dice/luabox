Feature: luabox lsp — code actions
  SPEC.md §8/§9 — the editor's code-action menu carries two families from the
  same request: every machine-applicable `luabox lint` fix as a `quickfix`
  (with the edit that applies it and the diagnostic it resolves), and the
  type-driven refactors — add-missing-field, annotate-local, generate-class,
  and the dot/colon method conversions. Each is conservative: offered only
  where the node shape is unambiguous, so an accepted edit always leaves the
  file parsing. A request that overlaps nothing offers nothing.

  Background:
    Given a project with edition "5.4"

  # --- lint quick-fixes ---------------------------------------------------

  Scenario: a fixable lint offers a quickfix carrying its edit
    Given a file "main.lua" containing:
      """
      local unused = 1
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 0 of "main.lua"
    Then a quickfix is offered
    And the quickfix resolves LB0501
    And the quickfix writes "_unused" at 0:6 to 0:12

  Scenario: a file with nothing to fix offers no quickfix
    Given a file "main.lua" containing:
      """
      return 1
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 0 of "main.lua"
    Then no quickfix is offered

  Scenario: a bare caret on the finding is enough to offer its fix
    Given a file "main.lua" containing:
      """
      local unused = 1
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions at 0:6 in "main.lua"
    Then a quickfix is offered
    And the quickfix writes "_unused" at 0:6 to 0:12

  Scenario: a caret on another line does not offer the fix
    Given a file "main.lua" containing:
      """
      local unused = 1
      return 1
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions at 1:0 in "main.lua"
    Then no quickfix is offered

  Scenario: a quickfix rewrites pairs over an array literal to ipairs
    Given a file "main.lua" containing:
      """
      local sum = 0
      for _, v in pairs({ 1, 2 }) do sum = sum + v end
      return sum
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 1 of "main.lua"
    Then a quickfix is offered
    And the quickfix resolves LB0507
    And the quickfix writes "ipairs" at 1:12 to 1:17

  Scenario: a quickfix rewrites a redundant nil comparison to plain truthiness
    Given a file "main.lua" containing:
      """
      ---@type string
      local name = "x"
      if name ~= nil then print(name) end
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 2 of "main.lua"
    Then a quickfix is offered
    And the quickfix resolves LB0505
    And the quickfix writes "name" at 2:3 to 2:14

  Scenario: a rule the tier defaults leave off offers its fix once enabled
    Given a project with edition "5.4" and lint rule "unused-param" set to "warn"
    And a file "main.lua" containing:
      """
      local function f(a) return 1 end
      return f
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 0 of "main.lua"
    Then a quickfix is offered
    And the quickfix resolves LB0502
    And the quickfix writes "_a" at 0:17 to 0:18

  Scenario: a rule silenced by the manifest offers no quickfix
    Given a project with edition "5.4" and lint rule "unused-local" set to "allow"
    And a file "main.lua" containing:
      """
      local unused = 1
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 0 of "main.lua"
    Then no quickfix is offered

  Scenario: lint quick-fixes and type refactors come from the same request
    Given a file "main.lua" containing:
      """
      local unusedOne = 1
      local unusedTwo = 2
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions from 0:0 to 1:19 in "main.lua"
    Then the offered actions are titled "unused-local: apply fix | unused-local: apply fix | Annotate `unusedOne` with inferred type"

  # --- add-missing-field --------------------------------------------------

  Scenario: a missing required field is inserted into a single-line table
    Given a strict project with edition "5.4"
    And a file "main.lua" containing:
      """
      ---@class Point
      ---@field x number
      ---@field y number

      ---@param p Point
      local function use(p) end
      use({ x = 1 })
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 6 of "main.lua"
    Then the offered actions are titled "Add missing field `y`"
    And the action titled "Add missing field `y`" is a "quickfix"
    And the action titled "Add missing field `y`" resolves LB0302
    And the action titled "Add missing field `y`" makes 1 edits
    And the action titled "Add missing field `y`" edits "main.lua"
    And edit 0 of the action titled "Add missing field `y`" spans 6:11 to 6:12
    And applying the action titled "Add missing field `y`" produces:
      """
      ---@class Point
      ---@field x number
      ---@field y number

      ---@param p Point
      local function use(p) end
      use({ x = 1,
          y = nil, -- TODO
      })
      """

  Scenario: an empty table offers one insert per missing field
    Given a strict project with edition "5.4"
    And a file "main.lua" containing:
      """
      ---@class Point
      ---@field x number
      ---@field y number

      ---@param p Point
      local function use(p) end
      use({})
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 6 of "main.lua"
    Then the offered actions are titled "Add missing field `x` | Add missing field `y`"
    And the action titled "Add missing field `x`" resolves LB0302
    And the action titled "Add missing field `y`" resolves LB0302
    And edit 0 of the action titled "Add missing field `x`" spans 6:5 to 6:5
    And applying the action titled "Add missing field `x`" produces:
      """
      ---@class Point
      ---@field x number
      ---@field y number

      ---@param p Point
      local function use(p) end
      use({
          x = nil, -- TODO
      })
      """

  Scenario: a table already laid out one field per line keeps that indent
    Given a strict project with edition "5.4"
    And a file "main.lua" containing:
      """
      ---@class Point
      ---@field x number
      ---@field y number

      ---@param p Point
      local function use(p) end
      use({
            x = 1,
      })
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 7 of "main.lua"
    Then the action titled "Add missing field `y`" makes 1 edits
    And edit 0 of the action titled "Add missing field `y`" spans 7:12 to 8:0
    And applying the action titled "Add missing field `y`" produces:
      """
      ---@class Point
      ---@field x number
      ---@field y number

      ---@param p Point
      local function use(p) end
      use({
            x = 1,
            y = nil, -- TODO
      })
      """

  Scenario: a table literal that already satisfies the class offers no insert
    Given a strict project with edition "5.4"
    And a file "main.lua" containing:
      """
      ---@class Point
      ---@field x number
      ---@field y number

      ---@param p Point
      local function use(p) end
      use({ x = 1, y = 2 })
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 6 of "main.lua"
    Then no action titled "Add missing field `y`" is offered
    And the reply is an empty list

  # --- annotate-local-from-inference --------------------------------------

  Scenario: an unannotated local is offered its inferred type annotation
    Given a file "main.lua" containing:
      """
      local n = 42
      print(n)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 0 of "main.lua"
    Then the offered actions are titled "Annotate `n` with inferred type"
    And the action titled "Annotate `n` with inferred type" is a "refactor.rewrite"
    And the action titled "Annotate `n` with inferred type" references no diagnostic
    And the action titled "Annotate `n` with inferred type" makes 1 edits
    And edit 0 of the action titled "Annotate `n` with inferred type" spans 0:0 to 0:0
    And applying the action titled "Annotate `n` with inferred type" produces:
      """
      ---@type integer
      local n = 42
      print(n)
      """

  Scenario: the inserted annotation matches the statement's indentation
    Given a file "main.lua" containing:
      """
      local function run()
        local total = "x"
        return total
      end
      return run
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 1 of "main.lua"
    Then applying the action titled "Annotate `total` with inferred type" produces:
      """
      local function run()
        ---@type string
        local total = "x"
        return total
      end
      return run
      """

  Scenario: an already annotated local is left alone
    Given a file "main.lua" containing:
      """
      ---@type number
      local n = 42
      print(n)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 1 of "main.lua"
    Then no action titled "Annotate `n` with inferred type" is offered
    And the reply is an empty list

  Scenario: a multi-name local is ambiguous, so nothing is offered
    Given a file "main.lua" containing:
      """
      local a, b = 1, 2
      print(a, b)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 0 of "main.lua"
    Then the reply is an empty list

  Scenario: a local with no inferable initializer is left alone
    Given a file "main.lua" containing:
      """
      local thing = unknown_global()
      print(thing)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 0 of "main.lua"
    Then no action titled "Annotate `thing` with inferred type" is offered

  # --- generate-class-from-literal ----------------------------------------

  Scenario: a table literal generates a class block from its named entries
    Given a file "main.lua" containing:
      """
      local cfg = { count = 1, label = "x" }
      print(cfg.count)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 0 of "main.lua"
    Then an action titled "Generate `---@class Cfg` from table literal" is offered
    And the action titled "Generate `---@class Cfg` from table literal" is a "refactor.rewrite"
    And the action titled "Generate `---@class Cfg` from table literal" makes 1 edits
    And edit 0 of the action titled "Generate `---@class Cfg` from table literal" spans 0:0 to 0:0
    And applying the action titled "Generate `---@class Cfg` from table literal" produces:
      """
      ---@class Cfg
      ---@field count integer
      ---@field label string
      local cfg = { count = 1, label = "x" }
      print(cfg.count)
      """

  Scenario: a table with no named entries has no fields to declare
    Given a file "main.lua" containing:
      """
      local list = { 1, 2, 3 }
      print(list)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 0 of "main.lua"
    Then no action titled "Generate `---@class List` from table literal" is offered

  Scenario: a table local already carrying a class annotation is left alone
    Given a file "main.lua" containing:
      """
      ---@class Config
      local cfg = { count = 1 }
      print(cfg)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 1 of "main.lua"
    Then the reply is an empty list

  # --- dot/colon conversion -----------------------------------------------

  Scenario: a dotted function with an explicit self becomes a colon method
    Given a file "main.lua" containing:
      """
      local T = {}
      function T.m(self, x) return x end
      return T
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 1 of "main.lua"
    Then the offered actions are titled "Convert `.` function to `:` method (drop explicit self)"
    And the action titled "Convert `.` function to `:` method (drop explicit self)" is a "refactor.rewrite"
    And the action titled "Convert `.` function to `:` method (drop explicit self)" makes 2 edits
    And edit 0 of the action titled "Convert `.` function to `:` method (drop explicit self)" writes ":"
    And edit 0 of the action titled "Convert `.` function to `:` method (drop explicit self)" spans 1:10 to 1:11
    And edit 1 of the action titled "Convert `.` function to `:` method (drop explicit self)" writes ""
    And edit 1 of the action titled "Convert `.` function to `:` method (drop explicit self)" spans 1:13 to 1:19
    And applying the action titled "Convert `.` function to `:` method (drop explicit self)" produces:
      """
      local T = {}
      function T:m(x) return x end
      return T
      """

  Scenario: a colon method becomes a dotted function with an explicit self
    Given a file "main.lua" containing:
      """
      local T = {}
      function T:m(x) return x end
      return T
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 1 of "main.lua"
    Then the offered actions are titled "Convert `:` method to `.` function with explicit self"
    And the action titled "Convert `:` method to `.` function with explicit self" makes 2 edits
    And edit 0 of the action titled "Convert `:` method to `.` function with explicit self" writes "."
    And edit 0 of the action titled "Convert `:` method to `.` function with explicit self" spans 1:10 to 1:11
    And edit 1 of the action titled "Convert `:` method to `.` function with explicit self" writes "self, "
    And edit 1 of the action titled "Convert `:` method to `.` function with explicit self" spans 1:13 to 1:13
    And applying the action titled "Convert `:` method to `.` function with explicit self" produces:
      """
      local T = {}
      function T.m(self, x) return x end
      return T
      """

  Scenario: a colon method with no parameters gains a lone self
    Given a file "main.lua" containing:
      """
      local T = {}
      function T:m() return 1 end
      return T
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 1 of "main.lua"
    Then edit 1 of the action titled "Convert `:` method to `.` function with explicit self" writes "self"
    And edit 1 of the action titled "Convert `:` method to `.` function with explicit self" spans 1:13 to 1:13
    And applying the action titled "Convert `:` method to `.` function with explicit self" produces:
      """
      local T = {}
      function T.m(self) return 1 end
      return T
      """

  Scenario: a dotted function whose first parameter is not self is not a method
    Given a file "main.lua" containing:
      """
      local T = {}
      function T.m(x) return x end
      return T
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 1 of "main.lua"
    Then the reply is an empty list

  Scenario: a bare function name has no separator to flip
    Given a file "main.lua" containing:
      """
      function f(x) return x end
      return f
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 0 of "main.lua"
    Then the reply is an empty list
