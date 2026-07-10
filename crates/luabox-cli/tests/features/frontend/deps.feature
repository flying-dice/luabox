Feature: Dependency management
  luabox add/remove/install/update/vendor (SPEC.md §4, §6): comment-preserving
  manifest edits, deterministic luabox.lock, store-backed installs into
  lua_modules/. Path deps are used in place; git deps are fetched with the
  git CLI and pinned to a commit; registry deps wait on #20.

  Scenario: add --path writes the dependency and preserves comments
    Given a file "luabox.toml" containing:
      """
      # project comment, must survive
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"
      """
    And a file "libs/mylib/luabox.toml" containing:
      """
      [package]
      name = "mylib"
      version = "1.0.0"
      edition = "5.4"
      """
    And a file "libs/mylib/src/init.lua" containing:
      """
      return {}
      """
    When I run "luabox add mylib --path libs/mylib"
    Then the command succeeds
    And "luabox.toml" contains "# project comment, must survive"
    And "luabox.toml" contains 'mylib = { path = "libs/mylib" }'
    And the file "luabox.lock" exists

  Scenario: install resolves a path dependency into luabox.lock
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"

      [dependencies]
      mylib = { path = "libs/mylib" }
      """
    And a file "libs/mylib/luabox.toml" containing:
      """
      [package]
      name = "mylib"
      version = "1.0.0"
      edition = "5.4"
      """
    When I run "luabox install"
    Then the command succeeds
    And the file "luabox.lock" exists
    And "luabox.lock" contains 'name = "mylib"'
    And "luabox.lock" contains "path+libs/mylib"

  Scenario: a second install with no changes does no work
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"

      [dependencies]
      mylib = { path = "libs/mylib" }
      """
    And a file "libs/mylib/luabox.toml" containing:
      """
      [package]
      name = "mylib"
      version = "1.0.0"
      edition = "5.4"
      """
    And I run "luabox install"
    When I run "luabox install"
    Then the command succeeds
    And stdout contains "up to date"

  Scenario: remove deletes the entry and updates the lockfile
    Given a file "luabox.toml" containing:
      """
      # keep me: comments survive luabox remove
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"

      [dependencies]
      mylib = { path = "libs/mylib" }
      """
    And a file "libs/mylib/luabox.toml" containing:
      """
      [package]
      name = "mylib"
      version = "1.0.0"
      edition = "5.4"
      """
    And I run "luabox install"
    When I run "luabox remove mylib"
    Then the command succeeds
    And "luabox.toml" does not contain "mylib"
    And "luabox.toml" contains "# keep me: comments survive luabox remove"
    And "luabox.lock" does not contain "mylib"

  Scenario: a registry dependency is rejected until the registry ships
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"

      [dependencies]
      penlight = "1.14"
      """
    When I run "luabox install"
    Then the command fails
    And stderr contains "#20"

  Scenario: add without --path or --git needs the registry
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"
      """
    When I run "luabox add penlight@1.14"
    Then the command fails
    And stderr contains "#20"
    And "luabox.toml" does not contain "penlight"

  @git
  Scenario: add --git installs from a repository and pins the commit
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"
      """
    And a git repository at "upstream" exporting package "gitlib" version "1.2.0"
    When I run "luabox add gitlib --git {dir}/upstream --tag v1.2.0"
    Then the command succeeds
    And "luabox.toml" contains 'gitlib = { git = "{dir}/upstream", tag = "v1.2.0" }'
    And "luabox.lock" contains 'version = "1.2.0"'
    And "luabox.lock" contains "git+{dir}/upstream#"
    And the file "lua_modules/gitlib/src/init.lua" exists
    And the file "lua_modules/gitlib/luabox.toml" exists

  @git
  Scenario: vendor copies git dependencies into vendor/
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.1.0"
      edition = "5.4"

      [dependencies]
      gitlib = { git = "{dir}/upstream", tag = "v1.2.0" }
      """
    And a git repository at "upstream" exporting package "gitlib" version "1.2.0"
    When I run "luabox vendor"
    Then the command succeeds
    And the file "vendor/gitlib/src/init.lua" exists
    And "vendor/gitlib/src/init.lua" contains "gitlib 1.2.0"
    And stdout contains "vendored 1 package(s) into vendor/"
