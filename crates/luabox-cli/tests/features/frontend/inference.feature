Feature: rich table inference — tables never degrade to bare `table`
  SPEC.md §3 hard requirement: the checker infers structural table shapes
  without annotations — per-field shapes from constructors and subsequent
  assignments, `setmetatable`/`__index` metatable chains (so idiomatic
  `Class.__index = Class` OOP typechecks unannotated), and typed
  `pairs`/`ipairs` iteration. Reads of provably absent fields on inferred
  shapes are LB0306; inferred types flowing into annotated slots keep the
  ordinary LB0300 mismatch code.

  Scenario: clean unannotated OOP typechecks in strict mode
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      local Circle = {}
      Circle.__index = Circle

      function Circle.new(radius)
        local o = setmetatable({}, Circle)
        o.radius = radius or 0
        return o
      end

      function Circle:area()
        return 3.14 * self.radius * self.radius
      end

      local c = Circle.new(2)
      local area = c:area()
      print(area)
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: an unannotated OOP field typo is caught in strict mode
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      local Circle = {}
      Circle.__index = Circle

      function Circle.new(radius)
        local o = setmetatable({}, Circle)
        o.radius = radius or 0
        return o
      end

      function Circle:area()
        return 3.14 * self.radiuss * self.radiuss
      end
      """
    When I run "luabox check"
    Then the command fails
    And diagnostic LB0306 is reported naming field "radiuss"

  Scenario: a misspelled method call is caught through the metatable chain
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      local Stack = {}
      Stack.__index = Stack

      function Stack.new()
        return setmetatable({ items = {} }, Stack)
      end

      function Stack:push(v)
        self.items[#self.items + 1] = v
      end

      local s = Stack.new()
      s:psuh(1)
      """
    When I run "luabox check"
    Then the command fails
    And diagnostic LB0306 is reported naming field "psuh"

  Scenario: ipairs iteration variables are typed from the inferred array part
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param s string
      local function shout(s) end

      local nums = { 1, 2, 3 }
      for _, n in ipairs(nums) do
        shout(n)
      end
      """
    When I run "luabox check"
    Then the command fails
    And diagnostic LB0300 is reported

  Scenario: an inferred shape extended by assignments satisfies a class parameter
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Point
      ---@field x number
      ---@field y number

      ---@param p Point
      ---@return number
      local function sum(p)
        return p.x + p.y
      end

      local pt = {}
      pt.x = 1
      pt.y = 2
      print(sum(pt))
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: warn-mode projects downgrade inference diagnostics to warnings
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      local Circle = {}
      Circle.__index = Circle

      function Circle:area()
        return 3.14 * self.radiuss
      end
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "warning[LB0306]"

  Scenario: reading an undeclared field on a declared class is undefined-field (#90)
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Point
      ---@field x number
      ---@field y number
      local Point = {}
      Point.__index = Point

      function Point:shift()
        return self.nope
      end
      """
    When I run "luabox check"
    Then the command fails
    And diagnostic LB0306 is reported naming field "nope"
    And stdout contains "undefined field `nope` on `Point`"

  Scenario: a `---@type Class` local's undeclared field read is undefined-field
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Point
      ---@field x number
      ---@field y number

      ---@type Point
      local p = { x = 1, y = 2 }
      local ok = p.x
      local bad = p.nope
      print(ok, bad)
      """
    When I run "luabox check"
    Then the command fails
    And diagnostic LB0306 is reported naming field "nope"

  Scenario: `---@diagnostic disable: undefined-field` suppresses the read rule
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@diagnostic disable: undefined-field
      ---@class Point
      ---@field x number
      local Point = {}
      Point.__index = Point

      function Point:shift()
        return self.nope
      end
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: a class with an indexer stays open to undeclared field reads
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Bag
      ---@field size number
      ---@field [string] boolean

      ---@type Bag
      local b = { size = 1 }
      local x = b.anything
      print(x)
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: a declared `---@operator` result types an expression (#114)
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Vec
      ---@operator add(Vec): Vec
      ---@operator mul(number): Vec

      ---@type Vec
      local a
      ---@type Vec
      local b
      ---@type Vec
      local sum = a + b
      ---@type Vec
      local scaled = 2 * a
      print(sum, scaled)
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: misusing a `---@operator` result is caught (#114)
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Vec
      ---@operator add(Vec): Vec

      ---@type Vec
      local a
      ---@type Vec
      local b
      ---@type string
      local s = a + b
      print(s)
      """
    When I run "luabox check"
    Then the command fails
    And diagnostic LB0300 is reported
