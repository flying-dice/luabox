Feature: luabox lsp — document formatting
  SPEC.md §8 — formatting hands back the canonical `luabox fmt` rendering as
  a single whole-document edit. An already-canonical document yields no
  edits, and a document that does not parse is returned untouched: the
  formatter never destroys broken code, and never answers with an error.
  Range formatting shares the whole-file semantics (the formatter is
  whole-file by construction).

  Background:
    Given a project with edition "5.4"

  Scenario: a messy document is rewritten by one whole-document edit
    Given a file "main.lua" containing:
      """
      local x=1
      if x>0 then
      print( x )
      end
      """
    And the language server is running
    And the document "main.lua" is open
    When I format "main.lua"
    Then the reply is a single text edit
    And the text edit produces:
      """
      local x = 1
      if x > 0 then
          print(x)
      end
      """

  Scenario: an already canonical document needs no edits
    Given a file "main.lua" containing:
      """
      local x = 1
      if x > 0 then
          print(x)
      end
      """
    And the language server is running
    And the document "main.lua" is open
    When I format "main.lua"
    Then the reply is an empty list

  Scenario: a document with a parse error is left alone
    Given a file "broken.lua" containing:
      """
      local = 1
      """
    And the language server is running
    And the document "broken.lua" is open
    When I format "broken.lua"
    Then the reply is an empty list

  Scenario: range formatting returns the same whole-document edit
    Given a file "main.lua" containing:
      """
      local x=1
      if x>0 then
      print( x )
      end
      """
    And the language server is running
    And the document "main.lua" is open
    When I format lines 2 to 3 of "main.lua"
    Then the reply is a single text edit
    And the text edit produces:
      """
      local x = 1
      if x > 0 then
          print(x)
      end
      """
