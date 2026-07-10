Feature: luabox bundle --mode — embedding modes (LÖVE, Neovim plugin)
  SPEC.md §7 (ticket #32): `luabox bundle` supports three embedding modes,
  chosen via `--mode` (overriding `[build] mode`, default `plain`):
  `plain` (today's single-file `.lua`, unchanged), `love` (a LÖVE-loadable
  `.love` zip archive with `main.lua` at its root), and `nvim-plugin` (a
  Neovim runtimepath plugin layout: `lua/<name>/init.lua` +
  `plugin/<name>.lua` + `doc/<name>.txt`). An unknown mode — from either
  source — is a hard, cargo-style error listing the valid set.

  Scenario: plain is the default mode
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      print("hi")
      """
    When I run "luabox bundle"
    Then the command succeeds
    And the file "dist/fixture.lua" exists

  Scenario: love mode packages the bundle as a .love archive containing main.lua
    Given a project with edition "5.1" targeting "5.1" using mode "love"
    And a file "src/main.lua" containing:
      """
      print("hello-from-love")
      """
    When I run "luabox bundle"
    Then the command succeeds
    And the file "dist/fixture.love" exists
    And the archive "dist/fixture.love" contains "main.lua"
    And stdout contains "packaged as a LÖVE .love archive"

  Scenario: love mode bundles conf.lua separately from main.lua
    Given a project with edition "5.1" targeting "5.1" using mode "love"
    And a file "src/main.lua" containing:
      """
      print("hello-from-love")
      """
    And a file "src/conf.lua" containing:
      """
      print("conf")
      """
    When I run "luabox bundle"
    Then the command succeeds
    And the archive "dist/fixture.love" contains "main.lua"
    And the archive "dist/fixture.love" contains "conf.lua"

  Scenario: love mode copies an assets directory into the archive verbatim
    Given a project with edition "5.1" targeting "5.1" using mode "love"
    And a file "src/main.lua" containing:
      """
      print("hello-from-love")
      """
    And a file "assets/sprite.txt" containing:
      """
      not-really-an-image
      """
    And a file "assets/sub/nested.txt" containing:
      """
      nested-and-not-really-an-image-either
      """
    When I run "luabox bundle"
    Then the command succeeds
    And the archive "dist/fixture.love" contains "assets/sprite.txt"
    And the archive "dist/fixture.love" contains "assets/sub/nested.txt"

  Scenario: love mode without conf.lua or assets packages just main.lua
    Given a project with edition "5.1" targeting "5.1" using mode "love"
    And a file "src/main.lua" containing:
      """
      print("hello-from-love")
      """
    When I run "luabox bundle"
    Then the command succeeds
    And the archive "dist/fixture.love" contains "main.lua"

  Scenario: nvim-plugin mode writes the runtimepath layout
    Given a project with edition "5.1" targeting "5.1" using mode "nvim-plugin"
    And a file "src/main.lua" containing:
      """
      local M = {}
      function M.hello()
        return "hi"
      end
      return M
      """
    When I run "luabox bundle"
    Then the command succeeds
    And the file "dist/fixture/lua/fixture/init.lua" exists
    And "dist/fixture/lua/fixture/init.lua" contains "return M"
    And the file "dist/fixture/plugin/fixture.lua" exists
    And "dist/fixture/plugin/fixture.lua" contains "lazy"
    And the file "dist/fixture/doc/fixture.txt" exists
    And "dist/fixture/doc/fixture.txt" contains "fixture"
    And stdout contains "written as a Neovim plugin layout"

  Scenario: nvim-plugin doc stub carries the package description
    Given a project with edition "5.1" targeting "5.1" using mode "nvim-plugin" and description "a test fixture plugin"
    And a file "src/main.lua" containing:
      """
      return {}
      """
    When I run "luabox bundle"
    Then the command succeeds
    And "dist/fixture/doc/fixture.txt" contains "a test fixture plugin"

  Scenario: invalid --mode lists the valid modes
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      print("hi")
      """
    When I run "luabox bundle --mode roblox"
    Then the command fails
    And stderr contains "roblox"
    And stderr contains "plain"
    And stderr contains "love"
    And stderr contains "nvim-plugin"

  Scenario: an invalid manifest [build] mode is rejected too
    Given a project with edition "5.1" targeting "5.1" using mode "roblox"
    And a file "src/main.lua" containing:
      """
      print("hi")
      """
    When I run "luabox bundle"
    Then the command fails
    And stderr contains "build.mode"

  Scenario: --mode overrides the manifest's [build] mode
    Given a project with edition "5.1" targeting "5.1" using mode "love"
    And a file "src/main.lua" containing:
      """
      print("hi")
      """
    When I run "luabox bundle --mode plain"
    Then the command succeeds
    And the file "dist/fixture.lua" exists
    And the file "dist/fixture.love" does not exist
