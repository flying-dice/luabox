Feature: luabox run — scripts and tasks (SPEC.md §4, §5)
  Resolves the argument as a `[tasks]` entry first, then a `.lua` script run
  via the configured runtime, then a bare `$PATH` executable as the last
  resort. Task commands run through the platform shell; array tasks stop at
  the first failing command; the exit code of whatever actually ran is
  propagated faithfully.

  These scenarios are hermetic: task scenarios use shell builtins available
  on both `cmd` and `sh` (`echo`, `exit`), and the script scenario uses the
  same "fake Lua runtime" trick as execution/test.feature (a tiny `.bat`
  shim pointed at via `LUABOX_LUA`), so no real Lua interpreter is required.

  Scenario: a single-command task runs through the platform shell
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [tasks]
      hello = "echo task-output"
      """
    When I run "luabox run hello"
    Then the command succeeds
    And stdout contains "task-output"

  Scenario: a .lua script runs via the configured runtime, with args passed through
    Given a fake Lua runtime that echoes its arguments
    And a file "src/main.lua" containing:
      """
      -- placeholder script body; the fake runtime never reads it
      """
    When I run "luabox run src/main.lua --flag value" with the echo runtime
    Then the command succeeds
    And stdout contains "--flag"
    And stdout contains "value"

  Scenario: an array task stops at the first failing command
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [tasks]
      ci = ["echo one", "exit 1", "echo two"]
      """
    When I run "luabox run ci"
    Then the command fails
    And stdout contains "one"
    And stdout does not contain "two"

  Scenario: an unknown name lists the project's available tasks
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      version = "0.1.0"
      edition = "5.4"

      [tasks]
      build = "echo building"
      ci = "echo testing"
      """
    When I run "luabox run luabox-no-such-task-xyz"
    Then the command fails
    And stderr contains "luabox-no-such-task-xyz"
    And stderr contains "build"
    And stderr contains "ci"

  Scenario: the resolved script's exit code propagates to luabox run
    Given a fake Lua runtime that always fails
    And a file "src/main.lua" containing:
      """
      -- placeholder script body; the fake runtime never reads it
      """
    When I run "luabox run src/main.lua" with the failing runtime
    Then the command fails
