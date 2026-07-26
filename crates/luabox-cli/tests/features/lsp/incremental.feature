Feature: luabox lsp — incremental analysis across the workspace
  SPEC.md §8 — the server answers from a salsa analysis host layered over the
  virtual file system: an editor overlay shadows the file on disk, a
  `didChange` invalidates every query that read the changed file (including
  the ones answering requests about *other* files), a `didClose` drops the
  overlay back to disk, and a `workspace/didChangeWatchedFiles` folds an
  out-of-editor edit into the same picture. These scenarios drive the cache
  through those transitions and check the answers move with it.

  Background:
    Given a project with edition "5.4"

  Scenario: a subclass added in one file appears in another file's implementations
    Given a file "base.lua" containing:
      """
      ---@class Base
      ---@field id number
      """
    And a file "derived.lua" containing:
      """
      ---@class Derived : Base
      """
    And the language server is running
    And the document "base.lua" is open
    And the document "derived.lua" is open
    When I request implementations at 0:10 in "base.lua"
    Then the reply lists 1 locations
    And 1 of the locations are in "derived.lua"
    When I change "derived.lua" to:
      """
      ---@class Derived : Base

      ---@class Second : Base
      """
    And I request implementations at 0:10 in "base.lua"
    Then the reply lists 2 locations
    And 2 of the locations are in "derived.lua"

  Scenario: editing a file moves the cross-file type definition it declares
    Given a file "point.lua" containing:
      """
      ---@class Point
      ---@field x number
      """
    And a file "main.lua" containing:
      """
      ---@type Point
      local p = nil
      print(p)
      """
    And the language server is running
    And the document "point.lua" is open
    And the document "main.lua" is open
    When I request the type definition at 2:6 in "main.lua"
    Then the location is in "point.lua"
    And the location starts on line 0
    When I change "point.lua" to:
      """
      -- a leading comment

      ---@class Point
      ---@field x number
      """
    And I request the type definition at 2:6 in "main.lua"
    Then the location is in "point.lua"
    And the location starts on line 2

  Scenario: workspace symbols follow a rename made in the editor
    Given a file "main.lua" containing:
      """
      local function oldName() return 1 end
      return oldName
      """
    And the language server is running
    And the document "main.lua" is open
    When I search the workspace for symbols matching "oldName"
    Then the symbols include "oldName"
    When I change "main.lua" to:
      """
      local function newName() return 1 end
      return newName
      """
    And I search the workspace for symbols matching "newName"
    Then the symbols include "newName"
    When I search the workspace for symbols matching "oldName"
    Then the symbols do not include "oldName"

  Scenario: closing a document drops the overlay back to the file on disk
    Given a file "main.lua" containing:
      """
      local unused = 1
      """
    And the language server is running
    And the document "main.lua" is open
    Then the diagnostics for "main.lua" include LB0501
    When I change "main.lua" to:
      """
      return 1
      """
    Then the diagnostics for "main.lua" are empty
    When I close "main.lua"
    Then the diagnostics for "main.lua" include LB0501
    When I open "main.lua"
    Then the diagnostics for "main.lua" include LB0501

  Scenario: an out-of-editor edit to a closed file republishes its diagnostics
    Given a file "helper.lua" containing:
      """
      return 1
      """
    And a file "main.lua" containing:
      """
      return 2
      """
    And the language server is running
    And the document "main.lua" is open
    When the file "helper.lua" changes on disk to:
      """
      local unused = 1
      return 1
      """
    Then the diagnostics for "helper.lua" include LB0501

  Scenario: an out-of-editor edit to an open file leaves the overlay in charge
    Given a file "main.lua" containing:
      """
      local unused = 1
      """
    And the language server is running
    And the document "main.lua" is open
    When the file "main.lua" changes on disk to:
      """
      local shadowed = 1
      """
    And I request document symbols for "main.lua"
    Then the symbols include "unused"
    And the symbols do not include "shadowed"
    And the diagnostics for "main.lua" include LB0501

  Scenario: an out-of-editor edit moves the type definition an open file resolves
    Given a file "point.lua" containing:
      """
      ---@class Point
      ---@field x number
      """
    And a file "main.lua" containing:
      """
      ---@type Point
      local p = nil
      print(p)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request the type definition at 2:6 in "main.lua"
    Then the location is in "point.lua"
    And the location starts on line 0
    When the file "point.lua" changes on disk to:
      """
      -- moved down by an external edit

      ---@class Point
      ---@field x number
      """
    And I request the type definition at 2:6 in "main.lua"
    Then the location is in "point.lua"
    And the location starts on line 2

  Scenario: a manifest change reloads the lint configuration for open documents
    Given a file "main.lua" containing:
      """
      local unused = 1
      """
    And the language server is running
    And the document "main.lua" is open
    Then the diagnostics for "main.lua" include LB0501
    When the manifest changes on disk to:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [lint]
      unused-local = "allow"
      """
    Then the diagnostics for "main.lua" are empty

  Scenario: a workspace query reaches every indexed file
    Given a file "one.lua" containing:
      """
      local function widgetOne() return 1 end
      return widgetOne
      """
    And a file "two.lua" containing:
      """
      local function widgetTwo() return 2 end
      return widgetTwo
      """
    And a file "nested/three.lua" containing:
      """
      local function widgetThree() return 3 end
      return widgetThree
      """
    And a file "nested/deep/four.lua" containing:
      """
      local function widgetFour() return 4 end
      return widgetFour
      """
    And the language server is running
    When I search the workspace for symbols matching "widget"
    Then the symbols include "widgetOne"
    And the symbols include "widgetTwo"
    And the symbols include "widgetThree"
    And the symbols include "widgetFour"
    And symbol "widgetThree" is declared in "nested/three.lua"
    And symbol "widgetFour" is declared in "nested/deep/four.lua"

  Scenario: a required module's export type reaches the requiring file's hints
    Given a file "util.lua" containing:
      """
      local M = {}
      ---@return number
      function M.count() return 1 end
      return M
      """
    And a file "main.lua" containing:
      """
      local util = require("util")
      local total = util.count()
      return total
      """
    And the language server is running
    And the document "main.lua" is open
    When I request inlay hints for the first 3 lines of "main.lua"
    Then an inlay hint at 1:11 reads ": number"

  Scenario: an exported function's parameters are seeded from its callers
    Given a file "util.lua" containing:
      """
      local M = {}
      function M.scale(n) return n * 2 end
      return M
      """
    And a file "main.lua" containing:
      """
      local util = require("util")
      return util.scale(3)
      """
    And the language server is running
    And the document "util.lua" is open
    When I request inlay hints for the first 3 lines of "util.lua"
    Then an inlay hint at 1:18 reads ": integer"
