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
