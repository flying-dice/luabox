Feature: luabox lsp — semantic tokens
  SPEC.md §8 — the whole-document token stream is delta-encoded against the
  legend advertised at initialize. It is semantic, not lexical: a name is
  classified by what it resolves to (parameter, local, stdlib global), and a
  LuaCATS block is distinguished from prose by the `documentation` modifier.

  Background:
    Given a project with edition "5.4"

  Scenario: names are classified by what they resolve to
    Given a file "main.lua" containing:
      """
      ---@param n number
      local function double(n)
          return n * 2
      end
      -- plain comment
      local answer = double(2)
      print(answer)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request semantic tokens for "main.lua"
    Then the token stream is well formed
    And the token at 1:15 is a function
    And the token at 1:15 is marked "declaration"
    And the token at 1:22 is a parameter
    And the token at 2:11 is a parameter
    And the token at 6:6 is a variable

  Scenario: a stdlib global is tagged as the default library
    Given a file "main.lua" containing:
      """
      ---@param n number
      local function double(n)
          return n * 2
      end
      -- plain comment
      local answer = double(2)
      print(answer)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request semantic tokens for "main.lua"
    Then the token at 6:0 is a variable
    And the token at 6:0 is marked "defaultLibrary"

  Scenario: an annotation block is documentation, a plain comment is not
    Given a file "main.lua" containing:
      """
      ---@param n number
      local function double(n)
          return n * 2
      end
      -- plain comment
      local answer = double(2)
      print(answer)
      """
    And the language server is running
    And the document "main.lua" is open
    When I request semantic tokens for "main.lua"
    Then the token at 0:0 is a comment
    And the token at 0:0 is marked "documentation"
    And the token at 4:0 is a comment
    And the token at 4:0 is not marked "documentation"
