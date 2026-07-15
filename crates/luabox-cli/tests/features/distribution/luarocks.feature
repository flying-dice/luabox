Feature: luarocks.org registry
  luabox follows the pnpm/bun model: luarocks.org IS the registry (SPEC.md §6,
  #19). Registry dependencies are declared in the project's rockspec — its
  `dependencies` are bare rock names in LuaRocks constraint syntax — and
  resolved against luarocks.org (or a local mirror). Rockspecs are Lua files,
  read statically; sources are fetched and pure-Lua modules laid out under
  lua_modules/. C-module rocks are out of scope and rejected with a clear
  error. Hermetic scenarios use LUABOX_LUAROCKS_MIRROR so no network is touched.

  Scenario: a rockspec-declared registry dependency resolves and installs from a mirror
    Given a file "luabox.toml" containing:
      """
      [package]
      edition = "5.4"
      """
    And a file "app-0.1.0-1.rockspec" containing:
      """
      package = "app"
      version = "0.1.0-1"
      dependencies = {
        "lua >= 5.1",
        "greet >= 1.0",
      }
      """
    And a luarocks mirror providing pure-Lua rock "greet" at "1.0"
    When I run "luabox install" against the luarocks mirror
    Then the command succeeds
    And the file "luabox.lock" exists
    And "luabox.lock" contains 'name = "greet"'
    And "luabox.lock" contains 'version = "1.0.0"'
    And the file "lua_modules/greet/greet.lua" exists

  Scenario: registry dependencies need no LUABOX_REGISTRY and no first-party registry
    Given a file "luabox.toml" containing:
      """
      [package]
      edition = "5.4"
      """
    And a file "app-0.1.0-1.rockspec" containing:
      """
      package = "app"
      version = "0.1.0-1"
      dependencies = { "greet >= 1.0" }
      """
    And a luarocks mirror providing pure-Lua rock "greet" at "1.0"
    When I run "luabox install" against the luarocks mirror
    Then the command succeeds
    And stdout does not contain "LUABOX_REGISTRY"

  Scenario: a C-module rock is rejected with a clear message
    Given a file "luabox.toml" containing:
      """
      [package]
      edition = "5.4"
      """
    And a file "app-0.1.0-1.rockspec" containing:
      """
      package = "app"
      version = "0.1.0-1"
      dependencies = { "luasocket >= 3.0" }
      """
    And a luarocks mirror providing C rock "luasocket" at "3.0"
    When I run "luabox install" against the luarocks mirror
    Then the command fails
    And stderr contains "luasocket"
    And stderr contains "C/native module"

  Scenario: add writes the rockspec, installs, and locks the rock
    Given a file "luabox.toml" containing:
      """
      [package]
      edition = "5.4"
      """
    And a file "app-0.1.0-1.rockspec" containing:
      """
      package = "app"
      version = "0.1.0-1"
      dependencies = {
         "lua >= 5.1",
      }
      """
    And a luarocks mirror providing pure-Lua rock "greet" at "1.0"
    When I run "luabox add greet" against the luarocks mirror
    Then the command succeeds
    And "app-0.1.0-1.rockspec" contains "greet >= 1.0.0"
    And "app-0.1.0-1.rockspec" contains "lua >= 5.1"
    And "luabox.lock" contains 'name = "greet"'
    And the file "lua_modules/greet/greet.lua" exists

  Scenario: add with an explicit version writes a lower-bound constraint
    Given a file "luabox.toml" containing:
      """
      [package]
      edition = "5.4"
      """
    And a file "app-0.1.0-1.rockspec" containing:
      """
      package = "app"
      version = "0.1.0-1"
      dependencies = {
         "lua >= 5.1",
      }
      """
    And a luarocks mirror providing pure-Lua rock "greet" at "1.0"
    When I run "luabox add greet@1.0" against the luarocks mirror
    Then the command succeeds
    And "app-0.1.0-1.rockspec" contains "greet >= 1.0"

  Scenario: add --dev creates and targets test_dependencies
    Given a file "luabox.toml" containing:
      """
      [package]
      edition = "5.4"
      """
    And a file "app-0.1.0-1.rockspec" containing:
      """
      package = "app"
      version = "0.1.0-1"
      dependencies = {
         "lua >= 5.1",
      }
      """
    And a luarocks mirror providing pure-Lua rock "greet" at "1.0"
    When I run "luabox add greet --dev" against the luarocks mirror
    Then the command succeeds
    And "app-0.1.0-1.rockspec" contains "test_dependencies = {"
    And "app-0.1.0-1.rockspec" contains "greet >= 1.0.0"

  Scenario: add of an unknown rock errors helpfully without touching the rockspec
    Given a file "luabox.toml" containing:
      """
      [package]
      edition = "5.4"
      """
    And a file "app-0.1.0-1.rockspec" containing:
      """
      package = "app"
      version = "0.1.0-1"
      dependencies = {
         "lua >= 5.1",
      }
      """
    And a luarocks mirror providing pure-Lua rock "greet" at "1.0"
    When I run "luabox add nosuchrock" against the luarocks mirror
    Then the command fails
    And stderr contains "luabox search nosuchrock"
    And "app-0.1.0-1.rockspec" does not contain "nosuchrock"

  Scenario: remove deletes the entry and drops the module after sync
    Given a file "luabox.toml" containing:
      """
      [package]
      edition = "5.4"
      """
    And a file "app-0.1.0-1.rockspec" containing:
      """
      package = "app"
      version = "0.1.0-1"
      dependencies = {
         "lua >= 5.1",
         "greet >= 1.0",
      }
      """
    And a luarocks mirror providing pure-Lua rock "greet" at "1.0"
    And I run "luabox install" against the luarocks mirror
    When I run "luabox remove greet" against the luarocks mirror
    Then the command succeeds
    And "app-0.1.0-1.rockspec" does not contain "greet"
    And "app-0.1.0-1.rockspec" contains "lua >= 5.1"
    And the file "lua_modules/greet/greet.lua" does not exist

  Scenario: a rockspec with comments survives an add/remove round-trip byte-identical
    Given a file "luabox.toml" containing:
      """
      [package]
      edition = "5.4"
      """
    And a file "app-0.1.0-1.rockspec" containing:
      """
      -- app package manifest
      package = "app"
      version = "0.1.0-1"

      dependencies = {
         "lua >= 5.1",     -- interpreter pin
         "greet >= 1.0",
      }
      """
    And a luarocks mirror providing pure-Lua rock "greet" at "1.0"
    And a luarocks mirror providing pure-Lua rock "penlight" at "1.14.0"
    And I run "luabox add penlight" against the luarocks mirror
    When I run "luabox remove penlight" against the luarocks mirror
    Then the command succeeds
    And "app-0.1.0-1.rockspec" equals:
      """
      -- app package manifest
      package = "app"
      version = "0.1.0-1"

      dependencies = {
         "lua >= 5.1",     -- interpreter pin
         "greet >= 1.0",
      }
      """

  @network
  Scenario: a real pure-Lua rock resolves and installs from luarocks.org
    Given a file "luabox.toml" containing:
      """
      [package]
      edition = "5.4"
      """
    And a file "app-0.1.0-1.rockspec" containing:
      """
      package = "app"
      version = "0.1.0-1"
      dependencies = { "inspect >= 3.1" }
      """
    When I install "inspect@3.1" from luarocks.org
    Then the command succeeds
