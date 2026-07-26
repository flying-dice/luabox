Feature: luabox lsp — code actions
  SPEC.md §8/§9 — every machine-applicable `luabox lint` fix is offered in
  the editor as a `quickfix`, carrying both the edit that applies it and the
  diagnostic it resolves, so the editor can pair the action with the squiggle
  it fixes. A file with nothing to fix offers no quick-fix.

  Background:
    Given a project with edition "5.4"

  Scenario: a fixable lint offers a quickfix carrying its edit
    Given a file "main.lua" containing:
      """
      local unused = 1
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 0 of "main.lua"
    Then a quickfix is offered
    And the quickfix resolves LB0501
    And the quickfix writes "_unused" at 0:6 to 0:12

  Scenario: a file with nothing to fix offers no quickfix
    Given a file "main.lua" containing:
      """
      return 1
      """
    And the language server is running
    And the document "main.lua" is open
    When I request code actions on line 0 of "main.lua"
    Then no quickfix is offered
