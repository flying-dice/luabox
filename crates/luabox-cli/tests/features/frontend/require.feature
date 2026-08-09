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

  Scenario: a require cycle between two class carriers terminates, and names the cycle
    # F80: the existing cycle pin (above, "a require cycle is tolerated")
    # uses plain-table modules, so the class-graph walk `collect_class`
    # added for parent resolution is never entered cyclically. Here each
    # carrier names the OTHER's class as its parent, and each module
    # requires the other — measured to terminate rather than hang or crash,
    # via the same `seen` recursion guard `collect_class` already carries.
    #
    # Round 6 review M67: terminating is what this scenario exists to pin,
    # and it still does. What it USED to pin alongside that — `0 errors, 0
    # warnings` — was not a property worth keeping. `---@class A : B` with
    # `---@class B : A` is a class that is its own ancestor: meaningless as
    # written, and previously accepted in complete silence, so a user who
    # closed the loop by a rename or a copy-paste got no signal from any
    # surface. That is now `LB0318`. The escape hatches are pinned in the
    # two scenarios below.
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
    Then the command fails
    And stdout contains "LB0318"
    And stdout contains "ancestry is cyclic"
    # Production readiness review G2: each project file gets its own
    # `TypeEnv`, so BOTH `a.lua`'s and `b.lua`'s check pass independently
    # rediscover the WHOLE cycle (`env::TypeEnv::note_cyclic` records every
    # name a cycle reaches, not just the one queried) and each produces its
    # own candidate diagnostic for BOTH `A` and `B` — 4 candidates for this
    # 2-class cycle where the identical shape written in one file produces
    # 2. `luabox_cli::check_cmd::dedupe_ancestry_diagnostics` collapses the
    # duplicates by `(code, primary span, message)` after every file's
    # diagnostics converge, so exactly one `LB0318` survives per class: 2,
    # not 4. The message is in the key because the ambient tier has no
    # distinguishing span at all — see that function's own doc comment.
    And stdout contains exactly 2 occurrence of "LB0318"

  Scenario: a require cycle across three class carriers dedupes to one diagnostic per class
    # R5 (production readiness issue): the scenario above proves the dedup
    # for a TWO-file mutual cycle only — every project file gets its own
    # `TypeEnv` (production readiness review G2), so a regression that
    # dedups by `code` alone (collapsing every `LB0318` in the project down
    # to just one, regardless of which class it names), or that fails to
    # collapse a three-way rediscovery at all, would still pass a two-file
    # fixture: with only two classes total, "one diagnostic per class" and
    # "one diagnostic total" cannot be told apart there. A three-file mutual
    # cycle (`A : C`, `C : B`, `B : A`) can tell them apart: three DISTINCT
    # classes, each independently rediscovering the whole cycle from its own
    # file's check pass (3 files × 3 names = 9 candidates before dedup), so
    # "collapsed to one span-keyed diagnostic per class" and "collapsed to
    # one diagnostic, period" predict different counts — 3 vs 1 — and only
    # the former is correct.
    #
    # The expected count (3) is measured against the built binary, not
    # guessed: `luabox_cli::check_cmd::dedupe_ancestry_diagnostics` collapses
    # by `(code, primary span, message)`, and each class's declaration span
    # is distinct, so each of the three survives exactly once.
    Given a strict project with edition "5.4"
    And a file "src/a.lua" containing:
      """
      local C = require("c")
      ---@class A : C
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
    And a file "src/c.lua" containing:
      """
      local B = require("b")
      ---@class C : B
      local M = {}
      return M
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0318"
    And stdout contains exactly 3 occurrence of "LB0318"
    And stdout contains "`A`'s `---@class` ancestry is cyclic"
    And stdout contains "`B`'s `---@class` ancestry is cyclic"
    And stdout contains "`C`'s `---@class` ancestry is cyclic"

  Scenario: the cyclic-class diagnostic downgrades with the strictness ladder
    # M67's escape hatch, half one: `LB0318` is a new strictness INCREASE
    # over code that checked clean before, so it must honour the same ladder
    # every other LB03xx does rather than being a hard rejection with no way
    # out (round 6 review M4(a) made exactly that complaint about LB0317).
    Given a project with edition "5.4"
    And a file "src/a.lua" containing:
      """
      ---@class CycA : CycB
      local M = {}
      return M
      """
    And a file "src/b.lua" containing:
      """
      ---@class CycB : CycA
      local M = {}
      return M
      """
    And a file "src/use.lua" containing:
      """
      ---@type CycA
      local a
      return a
      """
    When I run "luabox check"
    Then the command succeeds
    And stdout contains "LB0318"
    # The tight form, measured against the built binary: bare `0 errors` is a
    # substring of `10 errors` too, so it cannot tell a downgrade from a
    # flood. Every sibling scenario in this file already spells the whole
    # summary line out.
    And stderr contains "check: 0 errors, 2 warnings in 3 files"

  Scenario: the cyclic-class diagnostic is suppressible from its own declaring file alone
    # M67's escape hatch, half two, and production readiness review G1. The
    # rule name is luabox's own — lua-language-server 3.13.5 has no
    # counterpart to borrow one from (measured against the pinned binary: it
    # reports nothing for this shape) — and it has exactly one owner,
    # `luabox_types::RULE_CYCLIC_CLASS_ANCESTRY`.
    #
    # A prior version of this scenario put the SAME `---@diagnostic disable`
    # comment in BOTH files, which cannot fail even when the escape hatch is
    # broken cross-file (issue #58's pattern): a bare, file-wide `disable`
    # needs no line number at all to apply, so each file's own comment
    # coincidentally suppressed that file's misattributed copy of the OTHER
    # class's finding too — regardless of whether the checker was even
    # consulting the right file's directives. This version puts the comment
    # ONLY in `SupA`'s own declaring file, `a.lua`; `b.lua` has none. `SupA`'s
    # finding can disappear only if whichever file's check pass ends up
    # emitting it — not necessarily `a.lua`'s own; see
    # `check_file_with_artifacts_and_sources`'s doc comment — honors
    # `a.lua`'s directive against `a.lua`'s own source, never its own.
    # `SupB` carries no such comment anywhere and must still be reported.
    Given a strict project with edition "5.4"
    And a file "src/a.lua" containing:
      """
      ---@diagnostic disable: cyclic-class-ancestry
      ---@class SupA : SupB
      local M = {}
      return M
      """
    And a file "src/b.lua" containing:
      """
      ---@class SupB : SupA
      local M = {}
      return M
      """
    And a file "src/use.lua" containing:
      """
      ---@type SupA
      local a
      return a
      """
    When I run "luabox check"
    Then the command fails
    And stdout does not contain "`SupA`'s `---@class` ancestry is cyclic"
    And stdout contains "`SupB`'s `---@class` ancestry is cyclic"

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

  Scenario: a cross-file class cycle between two non-ASCII class names dedupes to one diagnostic each
    # Production readiness review, finding 7. Two of this file's mechanisms
    # are keyed on the class NAME as it reaches a `Diagnostic` — the
    # cross-file dedup (`(code, primary span, message)`, and the message is
    # the only place the name survives) and the `exactly N occurrence`
    # counting the cycle scenarios above assert with. Both had zero non-ASCII
    # coverage: every cycle fixture in this file uses `A`/`B`/`C`, so a
    # regression that truncated, normalised or byte-sliced a name would still
    # dedup and count correctly on ASCII and silently collapse two Greek
    # classes into one.
    #
    # Greek rather than CJK on purpose: the sibling scenario below already
    # covers a CJK name on the export boundary, and Greek is the case where a
    # careless `char_indices`/byte-offset slice produces a DIFFERENT wrong
    # answer (2-byte code points) than CJK does (3-byte).
    #
    # Measured against the built binary: 2 errors, one per class, each naming
    # its own class and attributed to its own declaring file.
    Given a strict project with edition "5.4"
    And a file "src/a.lua" containing:
      """
      local B = require("b")
      ---@class Ωμέγα : Δέλτα
      local M = {}
      return M
      """
    And a file "src/b.lua" containing:
      """
      local A = require("a")
      ---@class Δέλτα : Ωμέγα
      local M = {}
      return M
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains exactly 2 occurrence of "LB0318"
    And stdout contains "`Ωμέγα`'s `---@class` ancestry is cyclic"
    And stdout contains "`Δέλτα`'s `---@class` ancestry is cyclic"
    And stderr contains "check: 2 errors, 0 warnings in 2 files"

  Scenario: a unicode class name crosses the export boundary
    # Round 4 review R28: chaos gap (c) — the "very long class name" scenario
    # above is ~270 ASCII characters, a size axis, not a character-set one.
    # `take_name` (luacats/mod.rs) explicitly accepts any non-ASCII byte in a
    # name, so a CJK class name is legal LuaCATS input; nothing pinned it
    # surviving `class_members`/`export_class`'s `&str` keying across the
    # `require` boundary.
    #
    # Round 5 review N46: the original version of this scenario asserted only
    # field EXISTENCE (`m.nope` is `LB0306`), never that the declared field's
    # TYPE survives the boundary too — the half most likely to regress, since
    # a lenient `unknown` erasure of `x` would still leave `want(m.x)` clean
    # and would still leave `m.nope` undeclared, so the scenario would keep
    # passing. `wants` below pins the type: it demands `string`, `m.x` is
    # declared `number`, so only a genuine `number` crossing (not `unknown`)
    # makes `wants(m.x)` fail.
    #
    # Round 6 review M38: pinning `LB0300` alone still isn't the type-crossing
    # assertion N46 asked for — in strict mode an `unknown` erasure of `x`
    # emits the identical code, `LB0300`, just with a different message
    # (`found `unknown`` instead of `found `number``), so the round-4
    # regression this scenario exists to catch would still pass the
    # code-only check. Measured against this head: `wants(m.x)` produces
    # exactly `type mismatch: expected `string`, found `number``. Asserting
    # that message text — not just the code — is the discriminating check:
    # it is satisfied by the correct `number` crossing and not by an
    # `unknown` erasure, which would read `found `unknown`` instead.
    Given a strict project with edition "5.4"
    And a file "src/config.lua" containing:
      """
      ---@class 配置
      ---@field x number
      local M = {}
      return M
      """
    And a file "src/main.lua" containing:
      """
      ---@param n number
      local function want(n) end
      ---@param s string
      local function wants(s) end
      local m = require("config")
      want(m.x)
      wants(m.x)
      print(m.nope)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0306"
    And stdout contains "type mismatch: expected `string`, found `number`"

  Scenario: a class carried by a file that opens with a UTF-8 BOM crosses require with its field types intact
    # Round 4 review R28's third chaos gap, on the BOM half. This scenario
    # replaces a comment that declined coverage on the claim that asserting
    # the correct (clean) result "fails today" — round 5 review N44 measured
    # that, at this head, it does not: the `is_file_prefix` fold at
    # `crates/luabox-syntax/src/luacats/mod.rs:1105` (mutation-verified — two
    # tests die when it is removed) makes `resolve_target`'s backward scan
    # skip the leading BOM, so a doc block that opens a BOM'd file still
    # links to the statement that follows it rather than misreading it as a
    # trailing comment on nothing. Before that fold landed, field EXISTENCE
    # crossed `require` correctly from a BOM'd carrier but every declared
    # field's TYPE read back as `unknown` at the consumer. `wants` below
    # pins the type half the same way the unicode scenario above does: it
    # demands `string`, `p.x` is declared `number`, so only a genuine
    # `number` crossing makes `wants(p.x)` fail.
    #
    # Round 6 review M38: same gap as the unicode scenario above — `LB0300`
    # alone does not discriminate a genuine `number` crossing from an
    # `unknown` erasure, which reports the same code with a different
    # message. Measured against this head: `wants(p.x)` produces exactly
    # `type mismatch: expected `string`, found `number``; an erased `x`
    # would instead read `found `unknown``. Asserting the message closes the
    # gap the code-only assertion left open.
    Given a strict project with edition "5.4"
    And a file "src/point.lua" with a UTF-8 BOM containing:
      """
      ---@class Point
      ---@field x number
      local M = {}
      return M
      """
    And a file "src/main.lua" containing:
      """
      ---@param n number
      local function want(n) end
      ---@param s string
      local function wants(s) end
      local p = require("point")
      want(p.x)
      wants(p.x)
      print(p.nope)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0306"
    And stdout contains "type mismatch: expected `string`, found `number`"

  # One round 4 review R28 chaos gap still does not get a scenario here:
  #
  # * A required module deleted mid-session, then hovered. Hover
  #   (`textDocument/hover`) is exclusively an LSP-protocol surface —
  #   `features/lsp/`, driven by the separate `lsp_acceptance` harness. The
  #   CLI's `check` never hovers anything, so faking this against `luabox
  #   check` would not pin what the finding actually names. It belongs in
  #   `features/lsp/hover-require.feature`, outside this file's scope — and
  #   outside a documentation-only pass's file ownership besides. Round 5
  #   review N45: this deferral, like the CRLF one below, is not tracked in
  #   an issue; N24 names the live consequence (hover keeps answering from a
  #   deleted file after a `didChangeWatchedFiles` delete event).
  #
  # * CRLF through the export seam — genuinely blocked, not merely skipped.
  #   Every content-bearing `Given` step in this suite writes its fixture
  #   from a Gherkin docstring, and the `gherkin` crate's `docstring()` rule
  #   dedents the captured text via `textwrap::dedent`, which walks it with
  #   `str::lines()` (splits on `\r\n` and `\n` alike) and rejoins with `\n`
  #   alone — a literal CRLF authored inside a `"""..."""` block is silently
  #   normalized to LF before any step function ever runs (checked against
  #   `gherkin` 0.14.0's `docstring()` rule and `textwrap` 0.16.2's
  #   `dedent`). The BOM scenario above dodges the identical problem by
  #   never putting the mark INSIDE the docstring — `a file {string} with a
  #   UTF-8 BOM containing:` (`acceptance.rs`) prepends it outside the
  #   dedented text. CRLF needs the same treatment: a `write_file`-driven
  #   step that appends `\r\n` line endings itself. That step does not
  #   exist, and adding one means editing
  #   `crates/luabox-cli/tests/acceptance.rs`/`support/mod.rs` — outside
  #   this pass's file ownership. (BOM+CRLF combined and CRLF-only were both
  #   measured correct through this same seam at this head; only the harness
  #   gap keeps them uncovered.)
