Feature: luabox check — cross-file require resolution (#85)
  SPEC.md §3 + §7 — `local M = require("mod")` is typed from the required
  module's annotations, so conformance-style usage is checked in consumer
  and test files, not just the module's own file. Resolution reuses the
  bundler's `require` path-mapping (project root, `src/`, `?/init.lua`);
  requires that resolve nowhere stay `unknown` and raise no diagnostic of
  their own, and require cycles are tolerated.

  Scenario: a required module's export type flows into the consumer
    Given a strict project with edition "5.4"
    And a file "src/geom.lua" containing:
      """
      local M = {}
      ---@param w number
      ---@param h number
      ---@return number
      function M.area(w, h)
        return w * h
      end
      return M
      """
    And a file "src/app.lua" containing:
      """
      ---@param s string
      local function want(s) end

      local geom = require("geom")
      want(geom.area(3, 4))
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: the same required module used correctly is clean
    Given a strict project with edition "5.4"
    And a file "src/geom.lua" containing:
      """
      local M = {}
      ---@param w number
      ---@param h number
      ---@return number
      function M.area(w, h)
        return w * h
      end
      return M
      """
    And a file "src/app.lua" containing:
      """
      ---@param n number
      local function want(n) end

      local geom = require("geom")
      want(geom.area(3, 4))
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors, 0 warnings"

  Scenario: a required class-returning module reports method misuse at the consumer
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["shapes"]
      """
    And a file "defs/shapes.d.lua" containing:
      """
      ---@meta
      ---@class Shape
      ---@field area fun(self): number
      """
    And a file "src/circle.lua" containing:
      """
      ---@class Shape
      local Circle = {}
      Circle.__index = Circle

      ---@return number
      function Circle:area()
        return 1
      end

      ---@param r number
      ---@return Shape
      function Circle.new(r)
        return setmetatable({}, Circle)
      end

      return Circle
      """
    And a file "tests/circle_test.lua" containing:
      """
      local Circle = require("circle")
      local s = Circle.new(2)
      local _ = s:bogus()
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0306"
    And stdout contains "bogus"

  Scenario: an inline class with NO defs types through require (workspace-global classes)
    Given a strict project with edition "5.4"
    And a file "src/circle.lua" containing:
      """
      ---@class Circle
      ---@field r number
      local Circle = {}
      Circle.__index = Circle
      ---@param r number
      ---@return Circle
      function Circle.new(r) return setmetatable({ r = r }, Circle) end
      ---@return number
      function Circle:area() return 3.14159 * self.r * self.r end
      return Circle
      """
    And a file "src/main.lua" containing:
      """
      local Circle = require("circle")
      ---@type number
      local a1 = Circle.new(2).r
      local c = Circle.new(2)
      ---@type number
      local a2 = c:area()
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors, 0 warnings"

  Scenario: misuse of an inline class through require reports one specific error
    Given a strict project with edition "5.4"
    And a file "src/circle.lua" containing:
      """
      ---@class Circle
      ---@field r number
      local Circle = {}
      Circle.__index = Circle
      ---@param r number
      ---@return Circle
      function Circle.new(r) return setmetatable({ r = r }, Circle) end
      ---@return number
      function Circle:area() return 3.14159 * self.r * self.r end
      return Circle
      """
    And a file "src/main.lua" containing:
      """
      local Circle = require("circle")
      local c = Circle.new(2)
      ---@type number
      local bad = c:bogus()
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0306"
    And stdout contains "bogus"
    And stderr contains "check: 1 errors, 0 warnings"

  Scenario: a class declared by both defs and a module file merges members
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["circle"]
      """
    And a file "defs/circle.d.lua" containing:
      """
      ---@meta
      ---@class Circle
      ---@field r number
      ---@field new fun(r: number): Circle
      ---@field area fun(self): number
      """
    And a file "src/circle.lua" containing:
      """
      ---@class Circle
      ---@field r number
      local Circle = {}
      Circle.__index = Circle
      ---@param r number
      ---@return Circle
      function Circle.new(r) return setmetatable({ r = r }, Circle) end
      ---@return number
      function Circle:area() return 3.14159 * self.r * self.r end
      return Circle
      """
    And a file "src/main.lua" containing:
      """
      local Circle = require("circle")
      ---@type number
      local a1 = Circle.new(2).r
      local c = Circle.new(2)
      ---@type number
      local a2 = c:area()
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors, 0 warnings"

  Scenario: an unresolved require stays unknown and raises no diagnostic
    Given a strict project with edition "5.4"
    And a file "src/app.lua" containing:
      """
      local M = require("does_not_exist")
      local _ = M
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors, 0 warnings"

  Scenario: a require cycle is tolerated
    Given a strict project with edition "5.4"
    And a file "src/a.lua" containing:
      """
      local B = require("b")
      local A = {}
      ---@return number
      function A.f()
        return 1
      end
      return A
      """
    And a file "src/b.lua" containing:
      """
      local A = require("a")
      local B = {}
      ---@return number
      function B.g()
        return 2
      end
      return B
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors, 0 warnings"

  Scenario: require resolves a package init.lua
    Given a strict project with edition "5.4"
    And a file "src/pkg/init.lua" containing:
      """
      local M = {}
      ---@param w number
      ---@param h number
      ---@return number
      function M.area(w, h)
        return w * h
      end
      return M
      """
    And a file "src/app.lua" containing:
      """
      ---@param s string
      local function want(s) end

      local pkg = require("pkg")
      want(pkg.area(3, 4))
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"
