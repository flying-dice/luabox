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

  # --- cross-file `---@alias` naming (#110) ---------------------------------

  Scenario: an `---@alias` declared in another file is named and enforced in a consumer
    Given a strict project with edition "5.4"
    And a file "src/ids.lua" containing:
      """
      ---@alias Id string
      local M = {}
      return M
      """
    And a file "src/main.lua" containing:
      """
      ---@param x Id
      local function use(x) end
      use(42)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: a cross-file `---@alias` used correctly is clean
    Given a strict project with edition "5.4"
    And a file "src/ids.lua" containing:
      """
      ---@alias Id string
      local M = {}
      return M
      """
    And a file "src/main.lua" containing:
      """
      ---@param x Id
      local function use(x) end
      use("ok")
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors, 0 warnings"

  Scenario: a cross-file `---@alias` of a workspace-global class resolves to the class
    Given a strict project with edition "5.4"
    And a file "src/widget.lua" containing:
      """
      ---@class Widget
      ---@field id number
      local M = {}
      return M
      """
    And a file "src/handles.lua" containing:
      """
      ---@alias Handle Widget
      local M = {}
      return M
      """
    And a file "src/main.lua" containing:
      """
      ---@param h Handle
      local function use(h) end
      use({})
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0302"

  # The CI half of the class-carrier edge (#56; the editor half is pinned in
  # tests/features/lsp/hover-require.feature). A module whose export is a
  # `---@class` *carrier* crosses the boundary as the class it carries — the
  # workspace-global identity, which is what luals resolves the require to —
  # so its members are typed and enforced exactly as the instance spelling
  # always was. This used to be the lenient row of the limitations table.
  Scenario: a class-carrier module's members cross the boundary typed
    Given a strict project with edition "5.4"
    And a file "src/point.lua" containing:
      """
      ---@class Point
      ---@field x number
      local P = {}
      return P
      """
    And a file "src/main.lua" containing:
      """
      ---@param n number
      local function want(n) end
      local p = require("point")
      want(p.x)
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: a class-carrier module's undeclared members are rejected
    Given a strict project with edition "5.4"
    And a file "src/point.lua" containing:
      """
      ---@class Point
      ---@field x number
      local P = {}
      return P
      """
    And a file "src/main.lua" containing:
      """
      local p = require("point")
      print(p.nope)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0306"

  # Naming the class directly is the way to get it enforced — class names are
  # workspace-global, so no `require` is needed for the *type*.
  Scenario: naming the carrier's class directly enforces its fields
    Given a strict project with edition "5.4"
    And a file "src/point.lua" containing:
      """
      ---@class Point
      ---@field x number
      local P = {}
      return P
      """
    And a file "src/main.lua" containing:
      """
      ---@param p Point
      local function use(p) return p.nope end
      return use
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0306"
    And stdout contains "undefined field `nope` on `Point`"

  # Round 3 review F53(claims): the PR's own claim is "`p.nope` is LB0306, in
  # both spellings, with or without a `require`" (docs/03-reference/
  # 02-limitations.md:788 asserts the same for the instance row). The two
  # existing `p.nope`/LB0306 scenarios above are both carrier-shaped (the
  # class tag directly precedes the returned local); neither pins the
  # *instance* spelling — a `---@type Point` annotation on a local that is
  # NOT the class's own carrier, only later returned. Both directions are
  # probed (round 3 review: "properties that only assert 'no diagnostic'
  # pass under every leniency bug").
  Scenario: an instance-typed module's declared members cross the boundary typed
    Given a strict project with edition "5.4"
    And a file "src/point.lua" containing:
      """
      ---@class Point
      ---@field x number

      ---@type Point
      local p = { x = 0 }
      return p
      """
    And a file "src/main.lua" containing:
      """
      ---@param n number
      local function want(n) end
      local p = require("point")
      want(p.x)
      """
    When I run "luabox check"
    Then the command succeeds
    And zero diagnostics are reported

  Scenario: an instance-typed module's undeclared members are rejected
    Given a strict project with edition "5.4"
    And a file "src/point.lua" containing:
      """
      ---@class Point
      ---@field x number

      ---@type Point
      local p = { x = 0 }
      return p
      """
    And a file "src/main.lua" containing:
      """
      local p = require("point")
      print(p.nope)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0306"

  # --- chaos gaps reachable against the export/carrier boundary, round 3
  # review F80 -----------------------------------------------------------

  Scenario: a require cycle between two class carriers is tolerated
    # F80: the existing cycle pin (above, "a require cycle is tolerated")
    # uses plain-table modules, so the class-graph walk `collect_class`
    # added for parent resolution is never entered cyclically. Here each
    # carrier names the OTHER's class as its parent, and each module
    # requires the other — measured to terminate rather than hang or crash,
    # via the same `seen` recursion guard `collect_class` already carries.
    Given a strict project with edition "5.4"
    And a file "src/a.lua" containing:
      """
      local B = require("b")
      ---@class A : B
      local M = {}
      return M
      """
    And a file "src/b.lua" containing:
      """
      local A = require("a")
      ---@class B : A
      local M = {}
      return M
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors, 0 warnings"

  Scenario: a module requiring itself does not hang the checker
    # F80: the degenerate fixpoint of the cycle above.
    Given a strict project with edition "5.4"
    And a file "src/self.lua" containing:
      """
      local M = require("self")
      ---@class Loopy
      ---@field one string
      local N = {}
      return N
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors, 0 warnings"

  Scenario: a zero-member class exported as a carrier rejects every read
    # F80: every existing carrier fixture declares at least one member.
    # Nothing pinned whether an empty carrier's every field read is LB0306
    # (the class-identity rule) or lenient (nothing to enforce).
    Given a strict project with edition "5.4"
    And a file "src/empty.lua" containing:
      """
      ---@class Empty
      local M = {}
      return M
      """
    And a file "src/main.lua" containing:
      """
      local e = require("empty")
      print(e.anything)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0306"

  Scenario: a same-file duplicate carrier exported through require unions both halves
    # F80: the same-file duplicate-carrier union is pinned in-file
    # (`duplicate_class_merge.rs::a_duplicate_declaration_can_add_members_via_a_second_carrier`),
    # but that fixture never exports — this pins the union's interaction
    # with the export seam (#56/#59 together).
    Given a strict project with edition "5.4"
    And a file "src/two.lua" containing:
      """
      ---@class Two
      local A = {}
      function A:a() end

      ---@class Two
      local B = {}
      function B:b() end

      return B
      """
    And a file "src/main.lua" containing:
      """
      local t = require("two")
      t:a()
      t:b()
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors, 0 warnings"

  Scenario: a very long class name crosses the export boundary
    # F80: `class_members`/`export_class` key on `&str` names with no pin
    # outside short ASCII identifiers.
    Given a strict project with edition "5.4"
    And a file "src/longname.lua" containing:
      """
      ---@class ThisClassNameIsDeliberatelyVeryLongToExerciseAnyFixedSizeAssumptionInClassNameHandlingAcrossTheRequireBoundaryXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX
      ---@field x number
      local M = {}
      return M
      """
    And a file "src/main.lua" containing:
      """
      ---@param n number
      local function want(n) end
      local m = require("longname")
      want(m.x)
      print(m.nope)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0306"
