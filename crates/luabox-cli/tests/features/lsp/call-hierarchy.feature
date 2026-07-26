Feature: luabox lsp — call hierarchy
  SPEC.md §8 — `prepareCallHierarchy` resolves the function under the cursor
  (from its declaration or from any call site) to a hierarchy item; incoming
  calls then list its callers across the workspace, grouped by the function
  they sit in, and outgoing calls list what it calls.

  Background:
    Given a project with edition "5.4"

  Scenario: preparing on a declaration selects the function name
    Given a file "main.lua" containing:
      """
      local function greet() end
      greet()
      """
    And the language server is running
    And the document "main.lua" is open
    When I prepare the call hierarchy at 0:16 in "main.lua"
    Then the prepared item is named "greet"
    And the prepared item selects 0:15 to 0:20

  Scenario: preparing on a call site resolves to the same declaration
    Given a file "main.lua" containing:
      """
      local function greet() end
      greet()
      """
    And the language server is running
    And the document "main.lua" is open
    When I prepare the call hierarchy at 1:0 in "main.lua"
    Then the prepared item is named "greet"
    And the prepared item selects 0:15 to 0:20

  Scenario: outgoing calls list the callees in the function body
    Given a file "main.lua" containing:
      """
      local function a() end
      local function b() end
      local function caller()
        a()
        b()
        a()
      end
      return caller
      """
    And the language server is running
    And the document "main.lua" is open
    And I prepare the call hierarchy at 2:16 in "main.lua"
    When I request outgoing calls for the prepared item
    Then the outgoing calls are "a, b"
    And the outgoing call to "a" lists 2 call sites
    And the outgoing call to "b" lists 1 call sites

  Scenario: incoming calls find callers in another file
    Given a file "a.lua" containing:
      """
      function greet() return 1 end
      """
    And a file "b.lua" containing:
      """
      local function useGreet()
        greet()
        greet()
      end
      return useGreet
      """
    And the language server is running
    And the document "a.lua" is open
    And I prepare the call hierarchy at 0:10 in "a.lua"
    When I request incoming calls for the prepared item
    Then an incoming call comes from "useGreet" in "b.lua"
    And the incoming call from "useGreet" lists 2 call sites
