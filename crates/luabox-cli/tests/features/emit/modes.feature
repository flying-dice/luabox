Feature: luabox build --mode — embedding modes (LÖVE, Neovim plugin)
  SPEC.md §7 (ticket #32, flying-dice/luabox#4): `luabox build` packages a
  bundle in one of three embedding modes, chosen via `[build] mode` or
  `--mode` (which overrides it): `plain` (a single-file `.lua`), `love` (a
  LÖVE-loadable `.love` zip archive with `main.lua` at its root), and
  `nvim-plugin` (a Neovim runtimepath plugin layout: `lua/<name>/init.lua` +
  `plugin/<name>.lua` + `doc/<name>.txt`). A non-`plain` mode implies
  bundling, so it needs no `--bundle`. An unknown mode — from either source —
  is a hard, cargo-style error listing the valid set. `outfile` conflicts
  with a non-`plain` mode.

  Scenario: plain mode emits a single-file bundle
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      print("hi")
      """
    When I run "luabox build --bundle"
    Then the command succeeds
    And the file "dist/main.lua" exists

  Scenario: love mode packages the bundle as a .love archive containing main.lua
    Given a project with edition "5.1" targeting "5.1" using mode "love"
    And a file "src/main.lua" containing:
      """
      print("hello-from-love")
      """
    When I run "luabox build"
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
    When I run "luabox build"
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
    When I run "luabox build"
    Then the command succeeds
    And the archive "dist/fixture.love" contains "assets/sprite.txt"
    And the archive "dist/fixture.love" contains "assets/sub/nested.txt"

  Scenario: love mode without conf.lua or assets packages just main.lua
    Given a project with edition "5.1" targeting "5.1" using mode "love"
    And a file "src/main.lua" containing:
      """
      print("hello-from-love")
      """
    When I run "luabox build"
    Then the command succeeds
    And the archive "dist/fixture.love" contains "main.lua"

  Scenario: love mode conflicts with outfile
    Given a project with edition "5.1" targeting "5.1" using mode "love"
    And a file "src/main.lua" containing:
      """
      print("hello-from-love")
      """
    When I run "luabox build --outfile dist/game.lua"
    Then the command fails
    And stderr contains "outfile"
    And stderr contains "love"

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
    When I run "luabox build"
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
    When I run "luabox build"
    Then the command succeeds
    And "dist/fixture/doc/fixture.txt" contains "a test fixture plugin"

  Scenario: invalid --mode lists the valid modes
    Given a project with edition "5.1" targeting "5.1"
    And a file "src/main.lua" containing:
      """
      print("hi")
      """
    When I run "luabox build --mode roblox"
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
    When I run "luabox build"
    Then the command fails
    And stderr contains "build.mode"

  Scenario: love mode has no place for a sourcemap, so --sourcemap is dropped
    Given a project with edition "5.1" targeting "5.1" using mode "love"
    And a file "src/main.lua" containing:
      """
      print("hello-from-love")
      """
    When I run "luabox build --sourcemap"
    Then the command succeeds
    And the file "dist/fixture.love" exists
    And the file "dist/fixture.love.map" does not exist
    And stdout does not contain "with sourcemap"

  Scenario: nvim-plugin mode writes the sourcemap beside the bundled module
    Given a project with edition "5.1" targeting "5.1" using mode "nvim-plugin"
    And a file "src/main.lua" containing:
      """
      return {}
      """
    When I run "luabox build --sourcemap"
    Then the command succeeds
    And the file "dist/fixture/lua/fixture/init.lua.map" exists
    And stdout contains "with sourcemap"

  Scenario: the nvim doc stub falls back to a placeholder description
    Given a project with edition "5.1" targeting "5.1" using mode "nvim-plugin"
    And a file "src/main.lua" containing:
      """
      return {}
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/fixture/doc/fixture.txt" contains "(no description)"

  Scenario: the nvim bootstrap stub says the plugin is meant to be lazy-required
    Given a project with edition "5.1" targeting "5.1" using mode "nvim-plugin"
    And a file "src/main.lua" containing:
      """
      return {}
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/fixture/plugin/fixture.lua" contains "bootstrap stub, auto-sourced by Neovim on startup."
    And "dist/fixture/plugin/fixture.lua" contains 'require("fixture")'

  Scenario: nvim-plugin mode conflicts with outfile too
    Given a project with edition "5.1" targeting "5.1" using mode "nvim-plugin"
    And a file "src/main.lua" containing:
      """
      return {}
      """
    When I run "luabox build --outfile dist/plugin.lua"
    Then the command fails
    And stderr contains "outfile"
    And stderr contains "nvim-plugin"

  Scenario: an embedding mode packages exactly one entry point
    Given a project with edition "5.1" targeting "5.1" using mode "nvim-plugin"
    And a file "src/main.lua" containing:
      """
      return {}
      """
    And a file "src/other.lua" containing:
      """
      return {}
      """
    When I run "luabox build --entry src/main.lua --entry src/other.lua"
    Then the command fails
    And stderr contains "packages a single entry point, but 2 are configured"

  Scenario: a non-plain mode bundles without being asked to
    Given a project with edition "5.1" targeting "5.1" using mode "nvim-plugin"
    And a file "src/main.lua" containing:
      """
      local helper = require("helper")
      return helper
      """
    And a file "src/helper.lua" containing:
      """
      return { name = "helper" }
      """
    When I run "luabox build"
    Then the command succeeds
    And "dist/fixture/lua/fixture/init.lua" contains "-- bundled by luabox"
    And "dist/fixture/lua/fixture/init.lua" contains "__luabox_modules"

  Scenario: --mode overrides the manifest's [build] mode
    Given a project with edition "5.1" targeting "5.1" using mode "love"
    And a file "src/main.lua" containing:
      """
      print("hi")
      """
    When I run "luabox build --bundle --mode plain"
    Then the command succeeds
    And the file "dist/main.lua" exists
    And the file "dist/fixture.love" does not exist
