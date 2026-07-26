Feature: luabox check — class hierarchies and structural assignability
  SPEC.md §3: `---@class Child: Parent` chains field lookup through the
  parent, however deep. Assignability is structural and one-way — a child
  satisfies a slot typed as its parent, but never the reverse — and a shortfall
  names the field that is missing. Unannotated `__index` OOP resolves through
  the same machinery, so a hand-rolled prototype chain typechecks without a
  single annotation.

  # --- annotated `: Parent` chains ----------------------------------------

  Scenario: field lookup walks the whole inheritance chain
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class A
      ---@field a number
      ---@class B: A
      ---@field b number
      ---@class C: B
      ---@field c number

      ---@param x C
      ---@return number
      local function total(x)
        return x.a + x.b + x.c
      end
      return total
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: a field no ancestor declares is undefined on the most-derived class
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class A
      ---@field a number
      ---@class B: A
      ---@field b number
      ---@class C: B
      ---@field c number

      ---@param x C
      local function get(x)
        return x.nope
      end
      return get
      """
    When I run "luabox check"
    Then the command fails
    And diagnostic LB0306 is reported naming field "nope"
    And stdout contains "undefined field `nope` on `C`"

  Scenario: a child value satisfies a parameter typed as the parent
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class A
      ---@field a number
      ---@class B: A
      ---@field b number

      ---@param x A
      local function use(x) end

      ---@type B
      local child = { a = 1, b = 2 }
      use(child)
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: a parent value does not satisfy a parameter typed as the child
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class A
      ---@field a number
      ---@class B: A
      ---@field b number

      ---@param x B
      local function use(x) end

      ---@type A
      local parent = { a = 1 }
      use(parent)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "expected `B`, found `A`: missing `b`"

  # --- unannotated `__index` prototype chains -----------------------------

  Scenario: a hand-rolled prototype chain resolves inherited methods
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      local Base = {}
      Base.__index = Base

      function Base.new()
        return setmetatable({ hits = 0 }, Base)
      end

      function Base:bump()
        self.hits = self.hits + 1
        return self
      end

      local b = Base.new()
      local n = b:bump().hits
      print(n)
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: a method chained off an inferred receiver keeps its result type
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      local Counter = {}
      Counter.__index = Counter

      function Counter.new()
        return setmetatable({ n = 0 }, Counter)
      end

      function Counter:inc()
        self.n = self.n + 1
        return self
      end

      function Counter:get()
        return self:inc().n
      end

      local c = Counter.new()
      ---@type string
      local bad = c:get()
      return bad
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
    And stdout contains "expected `string`, found"

  Scenario: a class table declared with `---@class` still typechecks its own methods
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@class Shape
      ---@field sides integer
      local Shape = {}
      Shape.__index = Shape

      ---@return integer
      function Shape:count()
        return self.sides
      end

      ---@return integer
      function Shape:broken()
        return self.side
      end
      return Shape
      """
    When I run "luabox check"
    Then the command fails
    And diagnostic LB0306 is reported naming field "side"
