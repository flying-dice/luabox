Feature: search luarocks.org
  luarocks.org is luabox's registry (SPEC.md §6, #2), so `luabox search` reads
  its root manifest and matches the query as a case-insensitive substring of
  rock names. It is an anonymous registry read — no GitHub, no token. Hermetic
  scenarios point LUABOX_LUAROCKS_MIRROR at a scenario-local mirror so no
  network is touched. The `--format json` envelope `{"results":[…]}` is a frozen
  contract the editor GUIs consume.

  Scenario: a query matches rock names as a case-insensitive substring
    Given a luarocks mirror providing pure-Lua rock "penlight" at "1.0"
    And a luarocks mirror providing pure-Lua rock "penlight" at "1.5.4"
    And a luarocks mirror providing pure-Lua rock "inspect" at "3.1.3"
    When I run "luabox search PEN --format json" against the luarocks mirror
    Then the command succeeds
    And stdout is valid JSON
    And stdout contains '"results"'
    And stdout contains '"name": "penlight"'
    And stdout contains '"latest": "1.5.4"'
    And stdout contains '"versions": 2'
    And stdout contains '"description": null'
    And stdout does not contain "inspect"

  Scenario: an empty query lists rocks by name
    Given a luarocks mirror providing pure-Lua rock "penlight" at "1.0"
    And a luarocks mirror providing pure-Lua rock "inspect" at "3.1.3"
    When I run "luabox search --format json" against the luarocks mirror
    Then the command succeeds
    And stdout contains '"name": "inspect"'
    And stdout contains '"name": "penlight"'

  Scenario: the text rendering is an aligned table
    Given a luarocks mirror providing pure-Lua rock "penlight" at "1.5.4"
    When I run "luabox search penlight" against the luarocks mirror
    Then the command succeeds
    And stdout contains "NAME"
    And stdout contains "LATEST"
    And stdout contains "penlight"
    And stdout contains "1.5.4"

  Scenario: a query with no matches reports none clearly
    Given a luarocks mirror providing pure-Lua rock "penlight" at "1.5.4"
    When I run "luabox search nonesuch" against the luarocks mirror
    Then the command succeeds
    And stdout contains "no rocks found for `nonesuch`"
