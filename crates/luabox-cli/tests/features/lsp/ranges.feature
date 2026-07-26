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

  Scenario: nested functions each fold over their own span
    Given a file "main.lua" containing:
      """
      local function outer()
        local function inner()
          return 1
        end
        return inner
      end
      return outer
      """
    And the language server is running
    And the document "main.lua" is open
    When I request folding ranges for "main.lua"
    Then a folding range covers lines 0 to 5
    And a folding range covers lines 1 to 3
    And the folding range covering lines 0 to 5 has no kind

  Scenario: every block-bearing statement folds
    Given a file "main.lua" containing:
      """
      do
        local x = 1
        while x < 3 do
          x = x + 1
        end
        for i = 1, 2 do
          print(i)
        end
        repeat
          x = x + 1
        until x > 5
        return x
      end
      """
    And the language server is running
    And the document "main.lua" is open
    When I request folding ranges for "main.lua"
    Then a folding range covers lines 0 to 12
    And a folding range covers lines 2 to 4
    And a folding range covers lines 5 to 7
    And a folding range covers lines 8 to 10

  Scenario: an if chain folds as a whole and per clause
    Given a file "main.lua" containing:
      """
      if a then
        f()
      elseif b then
        g()
      else
        h()
      end
      """
    And the language server is running
    And the document "main.lua" is open
    When I request folding ranges for "main.lua"
    Then a folding range covers lines 0 to 6
    And a folding range covers lines 2 to 3
    And a folding range covers lines 4 to 5

  Scenario: a nested table constructor folds inside its parent
    Given a file "main.lua" containing:
      """
      local t = {
        inner = {
          1,
        },
      }
      return t
      """
    And the language server is running
    And the document "main.lua" is open
    When I request folding ranges for "main.lua"
    Then a folding range covers lines 0 to 4
    And a folding range covers lines 1 to 3

  Scenario: a run of line comments folds as one region and a lone one does not
    Given a file "main.lua" containing:
      """
      -- one
      -- two
      -- three
      local x = 1
      -- lonely
      return x
      """
    And the language server is running
    And the document "main.lua" is open
    When I request folding ranges for "main.lua"
    Then the reply lists 1 folding ranges
    And a comment folding range covers lines 0 to 2
    And no folding range covers lines 4 to 4

  Scenario: comment runs separated by a blank line stay separate regions
    Given a file "main.lua" containing:
      """
      -- a
      -- b

      -- c
      -- d
      return 1
      """
    And the language server is running
    And the document "main.lua" is open
    When I request folding ranges for "main.lua"
    Then the reply lists 2 folding ranges
    And a comment folding range covers lines 0 to 1
    And a comment folding range covers lines 3 to 4

  Scenario: nothing spanning more than one line means nothing to fold
    Given a file "main.lua" containing:
      """
      local t = { 1, 2 }
      local function f() return 1 end
      return t, f
      """
    And the language server is running
    And the document "main.lua" is open
    When I request folding ranges for "main.lua"
    Then the reply is an empty list
