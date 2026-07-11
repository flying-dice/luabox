Feature: luabox build — target lowering emit
  SPEC.md §2.1/§4 (§18 P3): `luabox build` lowers the edition (dialect you
  write) to the target (dialect you ship) and emits to the out dir. Check
  runs first — build refuses on check errors.

  Scenario: goto lowered away for a 5.1 target
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local i = 0
      ::top::
      i = i + 1
      if i < 3 then goto top end
      print(i)
      """
    When I run "luabox build"
    Then the command succeeds
    And the file "dist/src/main.lua" exists
    And the emitted output contains no "goto"
    And "dist/src/main.lua" contains "repeat"

  Scenario: bitops lowered through the tree-shaken rt polyfill
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local mask = 5
      print(mask & 3)
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" contains "__luabox_rt.band(mask, 3)"
    And the emitted output contains no "&"

  Scenario: edition equals target copies byte-identical
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      local x   =   1 -- odd spacing survives a copy
      print(x)
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/src/main.lua" equals:
      """
      local x   =   1 -- odd spacing survives a copy
      print(x)
      """

  Scenario: build refuses on check errors
    Given a strict project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      ---@param n number
      local function double(n)
        return n * 2
      end
      double("nope")
      """
    When I run "luabox build"
    Then the command fails
    And diagnostic LB0300 is reported
    And the file "dist/src/main.lua" does not exist

  Scenario: irreducible goto is a hard build error
    Given a project with edition "5.4" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      while true do
        goto out
      end
      ::out::
      print("after")
      """
    When I run "luabox build"
    Then the command fails
    And diagnostic LB0601 is reported
    And the file "dist/src/main.lua" does not exist
