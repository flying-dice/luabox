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

  Scenario: a colon method is named for its receiver and matched by method name
    Given a file "main.lua" containing:
      """
      local C = {}
      function C:greet() return 1 end
      local function run(c) return c:greet() end
      return run
      """
    And the language server is running
    And the document "main.lua" is open
    When I prepare the call hierarchy at 1:11 in "main.lua"
    Then the prepared item is named "C:greet"
    And the prepared item selects 1:11 to 1:16
    And the prepared item is in "main.lua"

  Scenario: a method call is an incoming call to the method it names
    Given a file "main.lua" containing:
      """
      local C = {}
      function C:greet() return 1 end
      local function run(c) return c:greet() end
      return run
      """
    And the language server is running
    And the document "main.lua" is open
    And I prepare the call hierarchy at 1:11 in "main.lua"
    When I request incoming calls for the prepared item
    Then the incoming calls are "run"
    And the incoming call from "run" lists 1 call sites

  Scenario: a method call is an outgoing call of the function that makes it
    Given a file "main.lua" containing:
      """
      local C = {}
      function C:greet() return 1 end
      local function run(c) return c:greet() end
      return run
      """
    And the language server is running
    And the document "main.lua" is open
    And I prepare the call hierarchy at 2:15 in "main.lua"
    When I request outgoing calls for the prepared item
    Then the outgoing calls are "C:greet"
    And the outgoing call to "C:greet" lists 1 call sites

  Scenario: a dotted function is matched by its full dotted spelling
    Given a file "main.lua" containing:
      """
      local M = {}
      function M.helper() return 1 end
      function M.main() return M.helper() + M.helper() end
      return M
      """
    And the language server is running
    And the document "main.lua" is open
    When I prepare the call hierarchy at 1:11 in "main.lua"
    Then the prepared item is named "M.helper"
    And the prepared item selects 1:11 to 1:17

  Scenario: incoming calls group both dotted call sites under one caller
    Given a file "main.lua" containing:
      """
      local M = {}
      function M.helper() return 1 end
      function M.main() return M.helper() + M.helper() end
      return M
      """
    And the language server is running
    And the document "main.lua" is open
    And I prepare the call hierarchy at 1:11 in "main.lua"
    When I request incoming calls for the prepared item
    Then the incoming calls are "M.main"
    And the incoming call from "M.main" lists 2 call sites

  Scenario: a recursive function calls and is called by itself
    Given a file "main.lua" containing:
      """
      local function fact(n)
        if n <= 1 then return 1 end
        return n * fact(n - 1)
      end
      return fact
      """
    And the language server is running
    And the document "main.lua" is open
    And I prepare the call hierarchy at 0:15 in "main.lua"
    When I request outgoing calls for the prepared item
    Then the outgoing calls are "fact"
    And the outgoing call to "fact" lists 1 call sites

  Scenario: the recursive call is also an incoming call from the function itself
    Given a file "main.lua" containing:
      """
      local function fact(n)
        if n <= 1 then return 1 end
        return n * fact(n - 1)
      end
      return fact
      """
    And the language server is running
    And the document "main.lua" is open
    And I prepare the call hierarchy at 0:15 in "main.lua"
    When I request incoming calls for the prepared item
    Then the incoming calls are "fact"
    And the incoming call from "fact" lists 1 call sites

  Scenario: a call at file top level groups under a module item for the file
    Given a file "main.lua" containing:
      """
      local function greet() return 1 end
      greet()
      return greet
      """
    And the language server is running
    And the document "main.lua" is open
    And I prepare the call hierarchy at 0:15 in "main.lua"
    When I request incoming calls for the prepared item
    Then the incoming calls are "main.lua"
    And the incoming call from "main.lua" is a module
    And the incoming call from "main.lua" lists 1 call sites

  Scenario: a local bound to a function expression is a declaration too
    Given a file "main.lua" containing:
      """
      local helper = function() return 1 end
      local function run() return helper() end
      return run
      """
    And the language server is running
    And the document "main.lua" is open
    When I prepare the call hierarchy at 0:6 in "main.lua"
    Then the prepared item is named "helper"
    And the prepared item selects 0:6 to 0:12

  Scenario: outgoing calls cross files to the declaring module
    Given a file "shared.lua" containing:
      """
      function shared() return 1 end
      """
    And a file "main.lua" containing:
      """
      local function run() return shared() end
      return run
      """
    And the language server is running
    And the document "main.lua" is open
    And I prepare the call hierarchy at 0:15 in "main.lua"
    When I request outgoing calls for the prepared item
    Then the outgoing calls are "shared"
    And the outgoing call to "shared" is in "shared.lua"

  Scenario: a leaf function calls nothing
    Given a file "main.lua" containing:
      """
      local function leaf() return 1 end
      return leaf
      """
    And the language server is running
    And the document "main.lua" is open
    And I prepare the call hierarchy at 0:15 in "main.lua"
    When I request outgoing calls for the prepared item
    Then the reply is an empty list

  Scenario: a function nobody calls has no incoming calls
    Given a file "main.lua" containing:
      """
      local function orphan() return 1 end
      return orphan
      """
    And the language server is running
    And the document "main.lua" is open
    And I prepare the call hierarchy at 0:15 in "main.lua"
    When I request incoming calls for the prepared item
    Then the reply is an empty list

  Scenario: a call through an alias local resolves to no declaration
    Given a file "main.lua" containing:
      """
      local function greet() return 1 end
      local alias = greet
      local function run() return alias() end
      return run
      """
    And the language server is running
    And the document "main.lua" is open
    When I prepare the call hierarchy at 2:28 in "main.lua"
    Then no call-hierarchy item is prepared

  Scenario: a cursor on a plain local names no function
    Given a file "main.lua" containing:
      """
      local n = 1
      return n
      """
    And the language server is running
    And the document "main.lua" is open
    When I prepare the call hierarchy at 0:6 in "main.lua"
    Then no call-hierarchy item is prepared

  Scenario: calls made by a nested function belong to that function, not its parent
    Given a file "main.lua" containing:
      """
      local function target() return 1 end
      local function outer()
        local function inner() return target() end
        return inner
      end
      return outer
      """
    And the language server is running
    And the document "main.lua" is open
    And I prepare the call hierarchy at 1:15 in "main.lua"
    When I request outgoing calls for the prepared item
    Then the reply is an empty list
