Feature: stdlib definition packages — `---@meta` `.d.lua` ambient types
  SPEC.md §3 — luabox ships per-edition definition packages describing the
  real stdlib (basic globals, string/table/math/io/os/coroutine/debug plus
  version-specific bit32/bit/jit/utf8). They are selected by `edition` and
  merged beneath a file's own annotations, so calls to `print`, `string.*`,
  `math.*`, ... are arity- and type-checked. Project-local `[types] defs`
  layer additional packages resolved from the `defs/` directory.

  Scenario: a stdlib misuse is caught in a strict project
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      string.rep("x", "y")
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: passing anything to print is fine
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      print(1, "two", true, nil, {})
      """
    When I run "luabox check"
    Then the command succeeds
    And stderr contains "check: 0 errors"

  Scenario: a version-gated stdlib signature is enforced in its edition
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      string.pack(123, 1)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: a global absent from the edition is not itself an error
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      local x = bit32.band(1, 2)
      print(x)
      """
    When I run "luabox check"
    Then the command succeeds

  Scenario: the same module resolves where the edition provides it
    Given a strict project with edition "5.2"
    And a file "src/main.lua" containing:
      """
      local x = bit32.band(1, 2)
      print(x)
      """
    When I run "luabox check"
    Then the command succeeds

  Scenario: a project-local defs package is loaded and enforced
    Given a file "defs/game.d.lua" containing:
      """
      ---@meta
      ---@param name string
      ---@return boolean
      function register(name) end
      """
    And a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["game"]
      """
    And a file "src/main.lua" containing:
      """
      register(123)
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "LB0300"

  Scenario: an unresolvable defs entry is a clear diagnostic
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [types]
      strict = true
      defs = ["nonexistent"]
      """
    And a file "src/main.lua" containing:
      """
      print("hi")
      """
    When I run "luabox check"
    Then the command fails
    And stdout contains "cannot resolve definition package `nonexistent`"
