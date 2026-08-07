Feature: luabox lsp — inlay hints
  SPEC.md §8 — inlay hints render the display-mode inference into the
  source, so unannotated Lua reads like a statically typed language:
  inferred binding types after each name, and the inferred (or annotated)
  return type after a parameter list. Function names and implicit `self`
  are deliberately skipped — those are hover's territory.

  Background:
    Given a project with edition "5.4"

  Scenario: unannotated locals and for-variables carry their inferred type
    Given a file "main.lua" containing:
      """
      local count = 42
      local greeting = "hi"
      local flag = true
      for i = 1, 10 do
        print(i)
      end
      """
    And the language server is running
    And the document "main.lua" is open
    When I request inlay hints for the first 6 lines of "main.lua"
    Then an inlay hint at 0:11 reads ": integer"
    And an inlay hint at 1:14 reads ": string"
    And an inlay hint at 2:10 reads ": boolean"
    And an inlay hint at 3:5 reads ": integer"

  Scenario: a table literal hints its inferred shape
    Given a file "main.lua" containing:
      """
      local point = { x = 1, y = 2 }
      return point
      """
    And the language server is running
    And the document "main.lua" is open
    When I request inlay hints for the first 2 lines of "main.lua"
    Then an inlay hint at 0:11 reads ": { x: integer, y: integer }"

  Scenario: an annotated return renders verbatim, even for a cross-file class
    Given a file "main.lua" containing:
      """
      local Rect = {}
      Rect.__index = Rect

      ---@param width number
      ---@param height number
      ---@return Rect
      function Rect.new(width, height)
        return setmetatable({ width = width, height = height }, Rect)
      end
      """
    And the language server is running
    And the document "main.lua" is open
    When I request inlay hints for the first 9 lines of "main.lua"
    Then an inlay hint at 6:32 reads ": Rect"
    And an inlay hint at 6:23 reads ": number"

  # Round 3 review F44, round 4 review R31: the display-mode ambient
  # (`binding_types`, the query this surface reads) needs the project-wide
  # class merge, not just the file's own annotations, or a required module's
  # `---@class` carrier export stays opaque instead of resolving to the
  # class it carries. F44's own test pinned the regression one layer down,
  # at `binding_types` itself (`luabox-db/tests/analysis.rs`); this is the
  # inlay-hint-level scenario nothing had reached.
  Scenario: a required module's class-carrier binding hints with the class name
    Given a file "widget.lua" containing:
      """
      ---@class Widget
      ---@field id number
      local W = {}
      return W
      """
    And a file "main.lua" containing:
      """
      local w = require("widget")
      print(w)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request inlay hints for the first 2 lines of "main.lua"
    Then an inlay hint at 0:7 reads ": Widget"

  # Round 5 review N43: the scenario above does not actually reach F44's fix.
  # `w`'s type is a bare `require` export (`Ty::Named("Widget")`, handed over
  # by `require_exports` unaided since #56), so it needs no cross-file merge
  # to render — deleting the merge (`query.rs`'s `with_project_types`) left
  # that scenario green. F44's real shape is a **constructor call**:
  # `reify_shape`'s instance-identity rule hands the display inference a bare
  # `Widget` for `m.make()`'s return, and only the cross-file merge lets it
  # resolve back to the class's own declaration — `luabox-db/tests/analysis.rs`'s
  # `display_mode_resolves_a_required_carriers_class_across_files` is the one
  # that actually fails when the merge is deleted; this is that fixture at
  # the surface a user sees.
  Scenario: a required module's carrier constructor call hints with the class name
    Given a file "widget.lua" containing:
      """
      ---@class Widget
      ---@field id number
      local W = {}
      W.__index = W

      ---@return Widget
      function W.make() return setmetatable({}, W) end
      return W
      """
    And a file "main.lua" containing:
      """
      local m = require("widget")
      local v = m.make()
      print(v)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request inlay hints for the first 3 lines of "main.lua"
    Then an inlay hint at 1:7 reads ": Widget"

  Scenario: a local function name gets no binding hint, only a return hint
    Given a file "main.lua" containing:
      """
      local function helper()
        return 1
      end
      local result = helper()
      return result
      """
    And the language server is running
    And the document "main.lua" is open
    When I request inlay hints for the first 5 lines of "main.lua"
    Then no inlay hint sits at 0:21
    And an inlay hint at 0:23 reads ": integer"
    And an inlay hint at 3:12 reads ": integer"
