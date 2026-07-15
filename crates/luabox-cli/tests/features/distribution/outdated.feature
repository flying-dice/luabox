Feature: outdated dependencies
  `luabox outdated` reports every resolved dependency against its latest
  available version (SPEC.md §6, #2): registry rocks (rockspec-declared) against
  the highest version on luarocks.org, and git deps against their GitHub repo's
  latest release. It always exits 0 — a report, not a gate. Hermetic scenarios
  use LUABOX_LUAROCKS_MIRROR (registry) and local git repositories (git); a
  local git URL is not a GitHub repo, so no network is touched. The
  `--format json` envelope `{"dependencies":[…]}` is a frozen GUI contract.

  Scenario: a registry dependency behind the registry's latest is outdated
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
        "greet >= 1.0, < 2.0",
      }
      """
    And a luarocks mirror providing pure-Lua rock "greet" at "1.0"
    And a luarocks mirror providing pure-Lua rock "greet" at "2.0"
    When I run "luabox install" against the luarocks mirror
    And I run "luabox outdated --format json" against the luarocks mirror
    Then the command succeeds
    And stdout is valid JSON
    And stdout contains '"dependencies"'
    And stdout contains '"name": "greet"'
    And stdout contains '"kind": "registry"'
    And stdout contains '"current": "1.0.0"'
    And stdout contains '"latest": "2.0.0"'
    And stdout contains '"outdated": true'

  Scenario: a registry dependency at the registry's latest is not outdated
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
    And I run "luabox outdated --format json" against the luarocks mirror
    Then the command succeeds
    And stdout contains '"name": "greet"'
    And stdout contains '"current": "1.0.0"'
    And stdout contains '"latest": "1.0.0"'
    And stdout contains '"outdated": false'

  @git
  Scenario: a git dependency is reported without hitting the network
    Given a git repository at "cool-lib" exporting package "cool-lib" version "1.0.0"
    And a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"

      [dependencies]
      cool-lib = { git = "{dir}/cool-lib", tag = "v1.0.0" }
      """
    When I run "luabox outdated --format json"
    Then the command succeeds
    And stdout is valid JSON
    And stdout contains '"dependencies"'
    And stdout contains '"name": "cool-lib"'
    And stdout contains '"kind": "git"'
    And stdout contains '"current": "v1.0.0"'
    And stdout contains '"outdated": false'
