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

  # --- the disclosed edge: a `---@class` carrier module (#54) --------------
  #
  # These scenarios pin what was *measured*, not what would be convenient.
  # A module whose export is a `---@class` carrier (`---@class Point` over
  # `local P = {}`) is a structural table as far as the per-file view the
  # editor surfaces are built on can tell: the class's `---@field`s live in
  # the declaring file's ambient environment, which only the type pass holds.
  # So the binding hovers as that table, its members have no hover, and
  # completion does not offer them. Recorded in
  # docs/03-reference/02-limitations.md.

  Scenario: a class-carrier module's binding hovers as a structural table
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
    Then the hover text contains "local p: {"
    And the hover text does not contain "Point"

  Scenario: a class-carrier module's member has no hover
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
    Then the reply is null

  Scenario: a class-carrier module's members are not offered by completion
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
    Then the completion list does not contain "x"

  Scenario: a class *instance* export does hover as the class name
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
