Feature: luabox toolchain — the runtime manager (SPEC.md §12)

  `luabox toolchain` acquires prebuilt Lua runtimes into
  `~/.luabox/toolchains` (an *acquirer* of runtimes, never a runtime), pins a
  project's choice in `luabox-toolchain.toml`, and lists what's installed. A
  pinned toolchain is resolved before any Lua on PATH.

  These scenarios are hermetic: no network and no real Lua. A scenario-local
  index (via `LUABOX_TOOLCHAIN_INDEX`) points the installer at a `.tar.gz`
  fixture whose "interpreter" is a tiny `.cmd` shim behaving like a runner, and
  toolchains land in a scenario-local directory (`LUABOX_TOOLCHAINS`).

  Scenario: install acquires a runtime and list shows it
    Given a toolchain index offering "5.4" with a working runtime
    When I run "luabox toolchain install 5.4" with the toolchain env
    Then the command succeeds
    And stdout contains "installed toolchain"
    When I run "luabox toolchain list" with the toolchain env
    Then the command succeeds
    And stdout contains "5.4"

  Scenario: a corrupt archive is rejected
    Given a corrupt toolchain index offering "5.4"
    When I run "luabox toolchain install 5.4" with the toolchain env
    Then the command fails
    And stderr contains "checksum mismatch"

  Scenario: pinning requires the toolchain to be installed
    Given a toolchain index offering "5.4" with a working runtime
    When I run "luabox toolchain pin 5.4" with the toolchain env
    Then the command fails
    And stderr contains "not installed"

  Scenario: install then pin records the pin and list marks it
    Given a toolchain index offering "5.4" with a working runtime
    When I run "luabox toolchain install 5.4" with the toolchain env
    Then the command succeeds
    When I run "luabox toolchain pin 5.4" with the toolchain env
    Then the command succeeds
    And the file "luabox-toolchain.toml" exists
    And "luabox-toolchain.toml" contains "toolchain ="
    And "luabox-toolchain.toml" contains "5.4"
    When I run "luabox toolchain list" with the toolchain env
    Then the command succeeds
    And stdout contains "(pinned)"

  Scenario: a pinned toolchain runs the test suite with no Lua on PATH
    Given a toolchain index offering "5.4" with a working runtime
    And a passing test file "ok_test.lua" with test "works"
    When I run "luabox toolchain install 5.4" with the toolchain env
    Then the command succeeds
    When I run "luabox toolchain pin 5.4" with the toolchain env
    Then the command succeeds
    When I run "luabox test" with the toolchain env
    Then the command succeeds
    And stdout contains "test result: ok"
