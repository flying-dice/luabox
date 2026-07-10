Feature: LuaRocks bridge
  Transparent LuaRocks bridge via rockspec translation (SPEC.md §6, #19).
  A dependency written `luarocks/<rock> = "<req>"` resolves against
  luarocks.org (or a local mirror) instead of the first-party registry: the
  `luarocks/` name prefix routes it to the bridge. Rockspecs — which are Lua
  files — are read statically, their sources fetched, and pure-Lua modules
  laid out under lua_modules/. C-module rocks are out of scope and rejected
  with a clear error. Hermetic scenarios use LUABOX_LUAROCKS_MIRROR so no
  network is touched.

  Scenario: a pure-Lua rock resolves and installs from a mirror
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"

      [dependencies]
      "luarocks/greet" = "1.0"
      """
    And a luarocks mirror providing pure-Lua rock "greet" at "1.0"
    When I run "luabox install" against the luarocks mirror
    Then the command succeeds
    And the file "luabox.lock" exists
    And "luabox.lock" contains 'name = "luarocks/greet"'
    And "luabox.lock" contains 'version = "1.0.0"'
    And the file "lua_modules/luarocks/greet/greet.lua" exists

  Scenario: a transitive rock dependency is bridged recursively
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"

      [dependencies]
      "luarocks/greet" = "1.0"
      """
    And a luarocks mirror providing pure-Lua rock "greet" at "1.0"
    And a luarocks mirror providing pure-Lua rock "colour" at "2.3"
    When I run "luabox install" against the luarocks mirror
    Then the command succeeds
    And the file "lua_modules/luarocks/greet/greet.lua" exists

  Scenario: a C-module rock is rejected with a clear message
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"

      [dependencies]
      "luarocks/luasocket" = "3.0"
      """
    And a luarocks mirror providing C rock "luasocket" at "3.0"
    When I run "luabox install" against the luarocks mirror
    Then the command fails
    And stderr contains "luasocket"
    And stderr contains "C/native module"

  Scenario: a luarocks dependency needs no first-party registry configured
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"

      [dependencies]
      "luarocks/greet" = "1.0"
      """
    And a luarocks mirror providing pure-Lua rock "greet" at "1.0"
    When I run "luabox install" against the luarocks mirror
    Then the command succeeds
    And stdout does not contain "LUABOX_REGISTRY"

  @network
  Scenario: a real pure-Lua rock resolves and installs from luarocks.org
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"

      [dependencies]
      "luarocks/inspect" = "3.1"
      """
    When I install "luarocks/inspect@3.1" from luarocks.org
    Then the command succeeds
