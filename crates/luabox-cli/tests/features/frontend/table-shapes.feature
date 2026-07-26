Feature: luabox check — table constructors and ambient global tables
  SPEC.md §3: a table constructor contributes a structural shape however its
  entries are spelled — positional items, `name = v` fields, and bracketed
  `["name"] = v` / `[1] = v` keys all fold into the same shape, while a
  genuinely dynamic key contributes nothing checkable rather than poisoning
  the shape. The same folding applies to a global table built up statement by
  statement (`mylib.sub.CONST = ...`), which is how a Lua library declares its
  ambient surface.

  # --- bracketed keys in a constructor ------------------------------------

  Scenario: a bracketed string key names the field it fills
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Conf
      ---@field name string
      ---@field count number

      ---@param c Conf
      local function use(c) end

      use({ ["name"] = "x", ["count"] = 1 })
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: a bracketed string key is checked against the field it names
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Conf
      ---@field name string
      ---@field count number

      ---@param c Conf
      local function use(c) end

      use({ ["name"] = 1, ["count"] = 1 })
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "expected `string`, found `1`"

  Scenario: a bracketed numeric key contributes to the array part
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type string[]
      local names = { [1] = "x", [2] = "y" }
      return names
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: a bracketed numeric key's value is checked against the element type
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@type string[]
      local names = { [1] = 1 }
      return names
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "expected `string`, found `1`"

  Scenario: a genuinely dynamic key contributes nothing checkable
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      local k = "computed"
      ---@type table<string, number>
      local t = { [k] = 1 }
      return t
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  # --- ambient global tables ----------------------------------------------

  Scenario: a global table's field takes the widened type of its literal
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      zlib = {}
      zlib.VERSION = "1.3.1"

      ---@param n number
      local function want(n) end

      want(zlib.VERSION)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "expected `number`, found"

  Scenario: nested global sub-tables fold through to the deepest field
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      cfg = {}
      cfg.a = {}
      cfg.a.b = {}
      cfg.a.b.n = 5

      ---@param s string
      local function want(s) end

      want(cfg.a.b.n)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "expected `string`, found"

  Scenario: a `---@class` on a global sub-table folds the field into the class
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class MyLib
      mylib = {}

      ---@class MyLibSub
      mylib.sub = {}
      mylib.sub.LEVEL = 3

      ---@param s string
      local function want(s) end

      want(mylib.sub.LEVEL)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "expected `string`, found"

  Scenario: a `---@type` on the assignment wins over the literal's own type
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      lib = {}
      ---@type string
      lib.name = nil

      ---@param n number
      local function want(n) end

      want(lib.name)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "expected `number`, found"
