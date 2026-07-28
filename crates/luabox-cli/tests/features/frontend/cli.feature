Feature: The luabox command-line surface
  SPEC.md §14: `luabox` is a single binary whose subcommands share one
  contract. A malformed invocation is clap's problem and exits 2 with a
  usage hint; a command that ran and found problems exits 1. Nothing on
  this surface reaches the network or runs Lua.

  Scenario: --help lists every subcommand
    Given an empty directory
    When I run "luabox --help"
    Then the command succeeds
    And stdout contains "Usage: luabox <COMMAND>"
    And stdout contains "check"
    And stdout contains "lint"
    And stdout contains "fmt"
    And stdout contains "build"
    And stdout contains "doc"
    And stdout contains "explain"

  Scenario: --version reports the binary version
    Given an empty directory
    When I run "luabox --version"
    Then the command succeeds
    And stdout contains "luabox"

  Scenario: invoking with no subcommand is a usage error
    Given an empty directory
    When I run "luabox"
    Then the command exits with code 2
    And stderr contains "Usage: luabox <COMMAND>"

  Scenario: an unknown subcommand is a usage error
    Given an empty directory
    When I run "luabox frobnicate"
    Then the command exits with code 2
    And stderr contains "unrecognized subcommand 'frobnicate'"
    And stderr contains "For more information, try '--help'."

  Scenario: an unknown flag on a known subcommand is a usage error
    Given an empty directory
    When I run "luabox lint --nope"
    Then the command exits with code 2
    And stderr contains "unexpected argument '--nope' found"
    And stderr contains "Usage: luabox lint"

  Scenario: a command that ran and found errors exits 1, not 2
    Given a strict project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      ---@param n number
      local function double(n)
        return n * 2
      end

      double("nope")
      """
    When I run "luabox check"
    Then the command exits with code 1

  Scenario: a clean run exits 0
    Given a project with edition "5.4"
    And a file "src/main.lua" containing:
      """
      return 1
      """
    When I run "luabox check"
    Then the command exits with code 0

  Scenario: a failure reports its cause, not a stack backtrace
    # RUST_BACKTRACE is a debugging knob for the *toolchain's* developers, and
    # is commonly exported for something else entirely. luabox renders its own
    # error chain, so a user with it set sees the same diagnostic as everyone
    # else — never frames from a stripped release binary.
    Given a file "luabox.toml" containing:
      """
      [package]
      edition = "5.9"
      """
    When I run "luabox check" with RUST_BACKTRACE set
    Then the command exits with code 1
    And stderr contains "invalid package.edition `5.9`"
    And stderr does not contain "Stack backtrace"

  Scenario Outline: a command cut from the v1 surface stays gone
    Given an empty directory
    When I run "luabox <command>"
    Then the command exits with code 2
    And stderr contains "unrecognized subcommand '<command>'"

    # The v1 scope cut (DIRECTION.md, 2026-07-26) made luabox a pure static
    # toolchain: no package manager, no registry client, no interpreter. Each
    # of these once existed. Re-adding one — even hidden — fails here.
    Examples:
      | command   |
      | add       |
      | remove    |
      | install   |
      | update    |
      | vendor    |
      | search    |
      | outdated  |
      | publish   |
      | login     |
      | logout    |
      | whoami    |
      | run       |
      | toolchain |

  Scenario Outline: every subcommand documents its own flags
    Given an empty directory
    When I run "luabox <command> --help"
    Then the command succeeds
    And stdout contains "Usage: luabox <command>"

    Examples:
      | command |
      | init    |
      | new     |
      | check   |
      | lint    |
      | fmt     |
      | build   |
      | doc     |
      | explain |
