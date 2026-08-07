Feature: luabox lsp — hover and completion on a `require` binding
  Issue #54 — `local m = require("mod")` hovered `unknown` while the type
  pass (CLI *and* LSP diagnostics) already resolved the module's export type
  for the same binding: two resolvers, one of which the editor never asked.
  There is now one — `crates/luabox-lsp/src/requires.rs` — and diagnostics,
  hover and completion all read it, so what the problems pane believes and
  what the cursor shows cannot drift apart again.

  README claims the editor and CI see the same surfaces ("hover and
  completion agree with CI"); these scenarios are that claim, executable.

  Background:
    Given a project with edition "5.4"

  # --- project modules, resolved through the database (#85) ---------------

  Scenario: a require binding hovers as the required module's export type
    Given a file "other.lua" containing:
      """
      local M = {}

      ---Helps.
      ---@param n number
      ---@return string
      function M.helper(n) return tostring(n) end

      return M
      """
    And a file "main.lua" containing:
      """
      local m = require("other")
      print(m)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 1:6 in "main.lua"
    Then the hover text contains "local m:"
    And the hover text contains "helper"
    And the hover text does not contain "local m: unknown"

  Scenario: the require binding's declaration hovers the same as its use
    Given a file "other.lua" containing:
      """
      local M = {}

      ---@return string
      function M.helper() return "s" end

      return M
      """
    And a file "main.lua" containing:
      """
      local m = require("other")
      print(m)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 0:6 in "main.lua"
    Then the hover text contains "helper"
    And the hover text does not contain "unknown"

  Scenario: a field of a require binding hovers as the module's field
    Given a file "other.lua" containing:
      """
      local M = {}

      ---@param n number
      ---@return string
      function M.helper(n) return tostring(n) end

      return M
      """
    And a file "main.lua" containing:
      """
      local m = require("other")
      print(m.helper(1))
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 1:10 in "main.lua"
    Then the hover text contains "(field) other.helper: fun("
    And the hover text contains "string"

  Scenario: completing a require binding lists the module's exported members
    Given a file "other.lua" containing:
      """
      local M = {}

      M.count = 1

      ---@param n number
      ---@return string
      function M.helper(n) return tostring(n) end

      return M
      """
    And a file "main.lua" containing:
      """
      local m = require("other")
      print(m.
      """
    And the language server is running
    And the document "main.lua" is open
    When I request completion at 1:8 in "main.lua"
    Then the completion list contains "helper"
    And the completion list contains "count"
    # The *inferred* type, literal and all — the one the checker will hold a
    # use site to. Widening it for display would be the editor disagreeing
    # with CI, which is the defect this file exists for.
    And completion item "count" has detail "other.count: 1"

  Scenario: a colon on a require binding offers only its function members
    Given a file "other.lua" containing:
      """
      local M = {}

      M.count = 1

      ---@return string
      function M.helper() return "s" end

      return M
      """
    And a file "main.lua" containing:
      """
      local m = require("other")
      print(m:
      """
    And the language server is running
    And the document "main.lua" is open
    When I request completion at 1:8 in "main.lua"
    Then the completion list contains "helper"
    And the completion list does not contain "count"
    And every completion item is a method

  # --- rock modules, resolved through the vendored-tree harvest (#30) ------

  Scenario: a rock require binding hovers as the harvested export type
    Given a file "lua_modules/share/lua/5.4/mylib/init.lua" containing:
      """
      local M = {}

      ---@param who string
      ---@return string
      function M.greet(who) return "hi " .. who end

      return M
      """
    And a file "main.lua" containing:
      """
      local mylib = require("mylib")
      print(mylib)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 1:6 in "main.lua"
    Then the hover text contains "greet"
    And the hover text does not contain "local mylib: unknown"

  Scenario: completing a rock require binding lists the harvested members
    Given a file "lua_modules/share/lua/5.4/mylib/init.lua" containing:
      """
      local M = {}

      ---@param who string
      ---@return string
      function M.greet(who) return "hi " .. who end

      return M
      """
    And a file "main.lua" containing:
      """
      local mylib = require("mylib")
      print(mylib.
      """
    And the language server is running
    And the document "main.lua" is open
    When I request completion at 1:12 in "main.lua"
    Then the completion list contains "greet"

  # --- explicit still beats implicit --------------------------------------

  Scenario: an annotated require binding keeps its annotation
    Given a file "other.lua" containing:
      """
      local M = {}
      ---@return string
      function M.helper() return "s" end
      return M
      """
    And a file "main.lua" containing:
      """
      ---@type string
      local m = require("other")
      print(m)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 2:6 in "main.lua"
    Then the hover text contains "local m: string"

  # --- the neighbourhood: what stays `unknown`, and why --------------------

  Scenario: requiring a module that does not exist hovers gracefully
    Given a file "main.lua" containing:
      """
      local m = require("absent")
      print(m)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 1:6 in "main.lua"
    Then the hover text contains "local m: unknown"

  # By design, not a gap: a dynamic require has no statically known module,
  # and the type pass does not resolve one either. Showing a type here would
  # mean inventing one.
  Scenario: a dynamic require hovers as unknown by design
    Given a file "other.lua" containing:
      """
      local M = {}
      ---@return string
      function M.helper() return "s" end
      return M
      """
    And a file "main.lua" containing:
      """
      local name = "other"
      local m = require(name)
      print(m)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 2:6 in "main.lua"
    Then the hover text contains "local m: unknown"

  # Also by design: which branch of `a or b` runs is a runtime fact, so
  # naming either module's type would be a coin flip presented as a type.
  Scenario: an or-chained require hovers as unknown by design
    Given a file "other.lua" containing:
      """
      local M = {}
      ---@return string
      function M.helper() return "s" end
      return M
      """
    And a file "spare.lua" containing:
      """
      return 1
      """
    And a file "main.lua" containing:
      """
      local m = require("other") or require("spare")
      print(m)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 1:6 in "main.lua"
    Then the hover text contains "local m: unknown"

  Scenario: a shadowing require binding hovers as its own module
    Given a file "other.lua" containing:
      """
      local M = {}
      ---@return string
      function M.helper() return "s" end
      return M
      """
    And a file "spare.lua" containing:
      """
      local S = {}
      ---@return number
      function S.count() return 1 end
      return S
      """
    And a file "main.lua" containing:
      """
      local m = require("other")
      print(m)
      local m = require("spare")
      print(m)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 3:6 in "main.lua"
    Then the hover text contains "count"
    And the hover text does not contain "helper"

  # --- navigation on a require binding (audited alongside, #54) -----------

  Scenario: the definition of a require binding is its local declaration
    Given a file "other.lua" containing:
      """
      return 1
      """
    And a file "main.lua" containing:
      """
      local m = require("other")
      print(m)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the definition at 1:6 in "main.lua"
    Then the location is in "main.lua"
    And the location starts at 0:6

  Scenario: the definition of the require string is the module file
    Given a file "other.lua" containing:
      """
      return 1
      """
    And a file "main.lua" containing:
      """
      local m = require("other")
      print(m)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the definition at 0:20 in "main.lua"
    Then the location is in "other.lua"

  # --- a `---@class` module export closes, in both spellings (#56) ---------
  #
  # This block used to pin the one shape the editor could not read (#54): a
  # module whose export is a `---@class`. The class's `---@field`s live in
  # the workspace ambient environment, and the per-file view hover and
  # completion were built on could not reach it. Two things closed it: the
  # export now crosses the `require` boundary as the class it carries (the
  # workspace-global identity, which is what luals resolves a require to),
  # and the editor surfaces resolve class members through the same ambient
  # environment the checker uses — so what the editor offers is what
  # `luabox check` enforces.

  Scenario: a class-carrier module's binding hovers as the class name
    Given a file "point.lua" containing:
      """
      ---@class Point
      ---@field x number
      local P = {}
      return P
      """
    And a file "main.lua" containing:
      """
      local p = require("point")
      print(p)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 1:6 in "main.lua"
    Then the hover text contains "local p: Point"

  Scenario: a class-carrier module's member hovers with its declared type
    Given a file "point.lua" containing:
      """
      ---@class Point
      ---@field x number
      local P = {}
      return P
      """
    And a file "main.lua" containing:
      """
      local p = require("point")
      print(p.x)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 1:8 in "main.lua"
    Then the hover text contains "Point.x"
    And the hover text contains "number"

  Scenario: a class-carrier module's members are offered by completion
    Given a file "point.lua" containing:
      """
      ---@class Point
      ---@field x number
      local P = {}
      return P
      """
    And a file "main.lua" containing:
      """
      local p = require("point")
      print(p.
      """
    And the language server is running
    And the document "main.lua" is open
    When I request completion at 1:8 in "main.lua"
    Then the completion list contains "x"
    And completion item "x" has detail "Point.x: number"

  Scenario: a member the class does not declare still has no hover
    Given a file "point.lua" containing:
      """
      ---@class Point
      ---@field x number
      local P = {}
      return P
      """
    And a file "main.lua" containing:
      """
      local p = require("point")
      print(p.nope)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 1:8 in "main.lua"
    Then the reply is null

  Scenario: a class *instance* export hovers as the class name
    Given a file "point.lua" containing:
      """
      ---@class Point
      ---@field x number

      ---@type Point
      local P = nil
      return P
      """
    And a file "main.lua" containing:
      """
      local p = require("point")
      print(p)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 1:6 in "main.lua"
    Then the hover text contains "local p: Point"

  Scenario: a class instance module's member hovers with its declared type
    Given a file "point.lua" containing:
      """
      ---@class Point
      ---@field x number

      ---@type Point
      local P = nil
      return P
      """
    And a file "main.lua" containing:
      """
      local p = require("point")
      print(p.x)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 1:8 in "main.lua"
    Then the hover text contains "Point.x"
    And the hover text contains "number"

  Scenario: a class instance module's members are offered by completion
    Given a file "point.lua" containing:
      """
      ---@class Point
      ---@field x number

      ---@type Point
      local P = nil
      return P
      """
    And a file "main.lua" containing:
      """
      local p = require("point")
      print(p.
      """
    And the language server is running
    And the document "main.lua" is open
    When I request completion at 1:8 in "main.lua"
    Then the completion list contains "x"
    And completion item "x" has detail "Point.x: number"

  Scenario: hover tracks an edit to the declaring file
    # Pins the merged-ambient cache's invalidation: the first hover populates
    # the revision-keyed cache, the didChange to the OTHER file must
    # invalidate it, and the second hover must see the member that edit
    # introduced. A cache keyed on anything that misses cross-file edits
    # serves the first (null) answer forever.
    Given a file "point.lua" containing:
      """
      ---@class Point
      ---@field x number
      local P = {}
      return P
      """
    And a file "main.lua" containing:
      """
      local p = require("point")
      print(p.y)
      """
    And the language server is running
    And the document "point.lua" is open
    And the document "main.lua" is open
    When I hover at 1:8 in "main.lua"
    Then the reply is null
    When I change "point.lua" to:
      """
      ---@class Point
      ---@field x number
      ---@field y number
      local P = {}
      return P
      """
    And I hover at 1:8 in "main.lua"
    Then the hover text contains "Point.y"
    And the hover text contains "number"

  Scenario: a class declared in another file resolves members with no require
    Given a file "shapes.lua" containing:
      """
      ---@class Circle
      ---@field radius number
      local C = {}
      return C
      """
    And a file "main.lua" containing:
      """
      ---@type Circle
      local c = nil
      print(c.radius)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 2:8 in "main.lua"
    Then the hover text contains "Circle.radius"
    And the hover text contains "number"

  # --- a receiver's own class is declared here, but inherits from another
  # file (#46) --------------------------------------------------------------
  #
  # #56 closed cross-file *require*; this closes cross-file *inheritance*.
  # `s`'s class (`Sub`) is declared in the same file as the receiver, so the
  # old file-local lookup succeeded and never fell through to the ambient —
  # an inherited member whose parent lives elsewhere hovered null though
  # `luabox check` resolved it through the merged project classes.

  Scenario: a member inherited from a parent declared in another file hovers
    Given a file "base.lua" containing:
      """
      ---@class Base
      ---@field id number
      """
    And a file "main.lua" containing:
      """
      ---@class Sub : Base
      ---@field name string

      ---@type Sub
      local s = nil
      print(s.id)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 5:8 in "main.lua"
    Then the hover text contains "Sub.id"
    And the hover text contains "number"

  Scenario: a member neither the class nor its cross-file parent declares still has no hover
    Given a file "base.lua" containing:
      """
      ---@class Base
      ---@field id number
      """
    And a file "main.lua" containing:
      """
      ---@class Sub : Base
      ---@field name string

      ---@type Sub
      local s = nil
      print(s.nope)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 5:8 in "main.lua"
    Then the reply is null

  # --- a bound generic reference resolves its type argument (#48) ----------
  #
  # `---@type Box<number>` monomorphises at the reference site — the checker
  # binds `Box`'s `T` to `number` when it resolves `b.item`. The ambient arm
  # used to extract just the bare name `Box` and ask for its unbound shape,
  # so hover answered `T` where the checker answered `number`.

  Scenario: a bound generic class declared in another file resolves its type argument
    Given a file "box.lua" containing:
      """
      ---@class Box<T>
      ---@field item T
      """
    And a file "main.lua" containing:
      """
      ---@type Box<number>
      local b = nil
      print(b.item)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 2:8 in "main.lua"
    Then the hover text contains "Box.item: number"

  # Round 4 review R32: this is a one-variable *control*, not #48 coverage —
  # the pre-fix bare-name lookup produces the identical `Box.item: T` answer
  # for this fixture (measured on both binaries), so it cannot distinguish
  # the fix from its absence by itself. It stands beside the falsifiable
  # bound-case scenario above (which the pre-fix lookup gets wrong, answering
  # `T` where the fix answers `number`) to show the fix does not change the
  # *unbound* answer — not to stand in for coverage of the bound one.
  Scenario: control — an unbound generic class still hovers its free type parameter
    Given a file "box.lua" containing:
      """
      ---@class Box<T>
      ---@field item T
      """
    And a file "main.lua" containing:
      """
      ---@type Box
      local b = nil
      print(b.item)
      """
    And the language server is running
    And the document "main.lua" is open
    When I hover at 2:8 in "main.lua"
    Then the hover text contains "Box.item: T"
