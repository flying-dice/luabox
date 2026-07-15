Feature: Project scaffolding — luabox init / luabox new
  The entry point to the toolchain (SPEC.md §4, §5): scaffold a manifest and
  source layout that every later command operates on.

  Scenario: init a binary project
    Given an empty directory
    When I run "luabox init --edition 5.4"
    Then the command succeeds
    And the file "luabox.toml" exists
    And "luabox.toml" contains 'edition = "5.4"'
    And "luabox.toml" does not contain "[dependencies]"
    And a rockspec file exists
    And the file "src/main.lua" exists

  Scenario: a scaffolded project passes check
    Given an empty directory
    And I run "luabox init --edition 5.4"
    When I run "luabox check"
    Then the command succeeds

  Scenario: init a library project
    Given an empty directory
    When I run "luabox init --lib --edition luajit"
    Then the command succeeds
    And "luabox.toml" contains 'edition = "luajit"'
    And the file "src/lib.lua" exists

  Scenario: init refuses to overwrite an existing project
    Given an empty directory
    And I run "luabox init"
    When I run "luabox init"
    Then the command fails
    And stderr contains "already exists"

  Scenario: init rejects an unknown edition
    Given an empty directory
    When I run "luabox init --edition 6.0"
    Then the command fails
    And stderr contains "unknown edition"

  Scenario: luau is not a supported edition
    Given an empty directory
    When I run "luabox init --edition luau"
    Then the command fails
    And stderr contains "unknown edition"

  Scenario: new scaffolds a slimmed manifest and a rockspec
    Given an empty directory
    When I run "luabox new my-tool"
    Then the command succeeds
    And the file "my-tool/luabox.toml" exists
    And "my-tool/luabox.toml" contains 'edition = "5.4"'
    And the file "my-tool/my-tool-0.1.0-1.rockspec" exists
    And "my-tool/my-tool-0.1.0-1.rockspec" contains 'package = "my-tool"'
    And "my-tool/my-tool-0.1.0-1.rockspec" contains 'url = "git+https://github.com/OWNER/my-tool.git"'
