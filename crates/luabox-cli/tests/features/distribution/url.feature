Feature: http(s) tarball dependencies
  luabox supports bun-style url sources in luabox.toml
  (`pkg = { url = "…", sha256 = "…" }`, SPEC.md §6): the tarball is fetched,
  its SHA-256 verified before extraction, and the tree installed into
  lua_modules/. The digest is captured at `luabox add --url` time and enforced
  on every install afterwards. Scenarios point at a local `.tar.gz` fixture, so
  they are hermetic and touch no network.

  Scenario: add --url captures the sha256 and installs the tarball
    Given a file "luabox.toml" containing:
      """
      # project comment, must survive
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"
      """
    And a tarball fixture "mylib.tar.gz" exporting file "init.lua"
    When I run "luabox add mylib --url file://{dir}/mylib.tar.gz"
    Then the command succeeds
    And "luabox.toml" contains "# project comment, must survive"
    And "luabox.toml" contains 'url = "file://{dir}/mylib.tar.gz"'
    And "luabox.toml" contains "sha256 ="
    And the file "luabox.lock" exists
    And "luabox.lock" contains "url+file://{dir}/mylib.tar.gz"
    And the file "lua_modules/mylib/init.lua" exists

  Scenario: a tampered tarball fails the install and installs nothing
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"

      [dependencies]
      mylib = { url = "file://{dir}/mylib.tar.gz", sha256 = "0000000000000000000000000000000000000000000000000000000000000000" }
      """
    And a tarball fixture "mylib.tar.gz" exporting file "init.lua"
    When I run "luabox install"
    Then the command fails
    And stderr contains "integrity check failed"
    And the file "lua_modules/mylib/init.lua" does not exist

  Scenario: a second install reuses the cache offline
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"
      """
    And a tarball fixture "mylib.tar.gz" exporting file "init.lua"
    And I run "luabox add mylib --url file://{dir}/mylib.tar.gz"
    And I delete the file "mylib.tar.gz"
    When I run "luabox install"
    Then the command succeeds
    And stdout contains "up to date"
    And the file "lua_modules/mylib/init.lua" exists
