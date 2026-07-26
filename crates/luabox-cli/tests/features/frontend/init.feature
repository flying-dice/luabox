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

  Scenario: new refuses an existing destination
    Given an empty directory
    And I run "luabox new my-tool"
    When I run "luabox new my-tool"
    Then the command fails
    And stderr contains "already exists"

  Scenario: new validates the edition before creating anything
    Given an empty directory
    When I run "luabox new my-tool --edition 6.0"
    Then the command fails
    And stderr contains "unknown edition"
    And the file "my-tool/luabox.toml" does not exist

  Scenario: a package name is derived from the directory, lowercased and dashed
    Given an empty directory
    When I run "luabox new My_Cool.Lib"
    Then the command succeeds
    And the file "My_Cool.Lib/my-cool-lib-0.1.0-1.rockspec" exists
    And "My_Cool.Lib/my-cool-lib-0.1.0-1.rockspec" contains 'package = "my-cool-lib"'

  Scenario: a name with no alphanumerics yields no package name
    Given an empty directory
    When I run "luabox new ..."
    Then the command fails
    And stderr contains "cannot derive a package name from directory `...`"

  Scenario: a library scaffold turns dashes into a Lua identifier
    Given an empty directory
    When I run "luabox new my-tool --lib"
    Then the command succeeds
    And the file "my-tool/src/lib.lua" exists
    And "my-tool/src/lib.lua" contains "local my_tool = {}"
    And "my-tool/src/lib.lua" contains "function my_tool.hello()"
    And the file "my-tool/src/main.lua" does not exist

  Scenario: a luajit project pins its rockspec to Lua 5.1
    Given an empty directory
    When I run "luabox new jitpkg --edition luajit"
    Then the command succeeds
    And "jitpkg/luabox.toml" contains 'edition = "luajit"'
    And "jitpkg/jitpkg-0.1.0-1.rockspec" contains '"lua >= 5.1"'

  Scenario: --lib and --bin contradict each other
    Given an empty directory
    When I run "luabox init --lib --bin"
    Then the command exits with code 2

  Scenario: init leaves an existing .gitignore alone
    Given an empty directory
    And a file ".gitignore" containing:
      """
      *.log
      """
    When I run "luabox init"
    Then the command succeeds
    And ".gitignore" equals:
      """
      *.log
      """

  Scenario: a scaffolded project formats and lints clean
    Given an empty directory
    And I run "luabox init --edition 5.4"
    When I run "luabox fmt --check"
    Then the command succeeds
    When I run "luabox lint"
    Then the command succeeds
    And stderr contains "0 errors, 0 warnings"
