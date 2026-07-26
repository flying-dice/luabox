Feature: luabox lsp — folding and selection ranges
  SPEC.md §8 — both are pure syntax-tree geometry. Folding marks the
  collapsible regions (block bodies, multi-line table constructors, and
  comment runs, the last tagged with the `comment` kind); selection ranges
  give the editor the expand-selection chain from the token under the cursor
  out to the whole file.

  Background:
    Given a project with edition "5.4"

  Scenario: a function body and a multi-line table both fold
    Given a file "main.lua" containing:
      """
      local function f()
        return 1
      end
      local t = {
        1,
        2,
      }
      return t
      """
    And the language server is running
    And the document "main.lua" is open
    When I request folding ranges for "main.lua"
    Then a folding range covers lines 0 to 2
    And a folding range covers lines 3 to 6

  Scenario: a multi-line comment folds under the comment kind
    Given a file "main.lua" containing:
      """
      --[[
      long comment
      ]]
      local x = 1
      return x
      """
    And the language server is running
    And the document "main.lua" is open
    When I request folding ranges for "main.lua"
    Then a comment folding range covers lines 0 to 2

  Scenario: the selection chain widens from the token out to the file
    Given a file "main.lua" containing:
      """
      local x = 1 + 2
      """
    And the language server is running
    And the document "main.lua" is open
    When I request selection ranges at 0:10 in "main.lua"
    Then the selection chain has at least 3 ranges
    And the innermost selection range spans 0:10 to 0:11
    And the outermost selection range spans 0:0 to 1:0
