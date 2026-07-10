Feature: First-party registry: publish, install, yank
  SPEC.md §6 (ticket #20): a static sparse index on the crates.io model.
  `luabox publish` gates on check + tests, packs the package tree, records
  its content-addressed tree hash as the index checksum, and appends one
  immutable JSON line per version; `luabox install` resolves from the index,
  fetches the artifact tar, and verifies the tree hash before materializing.
  Yank hides a version from new resolutions without ever deleting it.

  These scenarios are hermetic: LUABOX_REGISTRY points at a scenario-local
  directory registry (".registry"), LUABOX_STORE at a scenario-local store.
  Publisher and consumer are sibling project directories in one scenario.

  Scenario: publish a package, then install it from another project
    Given a file "pkg/luabox.toml" containing:
      """
      [package]
      name = "mathlib"
      version = "1.2.0"
      edition = "5.4"
      """
    And a file "pkg/src/init.lua" containing:
      """
      local M = {}

      ---Adds two numbers.
      ---@param a number
      ---@param b number
      ---@return number
      function M.add(a, b)
        return a + b
      end

      return M
      """
    When I run "luabox publish" in "pkg" against the registry
    Then the command succeeds
    And stdout contains "published `mathlib@1.2.0`"
    And the file ".registry/index/ma/th/mathlib" exists
    And the file ".registry/artifacts/mathlib/1.2.0.tar" exists
    Given a file "app/luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.0.1"
      edition = "5.4"

      [dependencies]
      mathlib = "^1.2"
      """
    When I run "luabox install" in "app" against the registry
    Then the command succeeds
    And the file "app/luabox.lock" exists
    And "app/luabox.lock" contains 'name = "mathlib"'
    And "app/luabox.lock" contains 'source = "registry"'
    And "app/luabox.lock" contains 'checksum = "sha256:'
    And the file "app/lua_modules/mathlib/src/init.lua" exists
    And the file "app/lua_modules/mathlib/luabox.toml" exists
    And "app/lua_modules/mathlib/src/init.lua" contains "M.add"

  Scenario: publishing the same version twice is refused
    Given a file "pkg/luabox.toml" containing:
      """
      [package]
      name = "mathlib"
      version = "1.0.0"
      edition = "5.4"
      """
    And a file "pkg/src/init.lua" containing:
      """
      return {}
      """
    And I run "luabox publish" in "pkg" against the registry
    When I run "luabox publish" in "pkg" against the registry
    Then the command fails
    And stderr contains "already published"
    And stderr contains "yank"

  Scenario: publish is blocked by check errors
    Given a file "pkg/luabox.toml" containing:
      """
      [package]
      name = "mathlib"
      version = "1.0.0"
      edition = "5.4"
      """
    And a file "pkg/src/init.lua" containing:
      """
      local = broken syntax here
      """
    When I run "luabox publish" in "pkg" against the registry
    Then the command fails
    And stderr contains "publish blocked"
    And the file ".registry/index/ma/th/mathlib" does not exist

  Scenario: publish warns about public functions without annotations
    Given a file "pkg/luabox.toml" containing:
      """
      [package]
      name = "mathlib"
      version = "1.0.0"
      edition = "5.4"
      """
    And a file "pkg/src/init.lua" containing:
      """
      local M = {}

      function M.add(a, b)
        return a + b
      end

      return M
      """
    When I run "luabox publish" in "pkg" against the registry
    Then the command succeeds
    And stderr contains "lack ---@param/---@return annotations"
    And stderr contains "M.add"

  Scenario: yanked versions are skipped by new resolutions but never deleted
    Given a file "pkg/luabox.toml" containing:
      """
      [package]
      name = "mathlib"
      version = "1.0.0"
      edition = "5.4"
      """
    And a file "pkg/src/init.lua" containing:
      """
      return { version = 1 }
      """
    And I run "luabox publish" in "pkg" against the registry
    And a file "pkg/luabox.toml" containing:
      """
      [package]
      name = "mathlib"
      version = "1.1.0"
      edition = "5.4"
      """
    And I run "luabox publish" in "pkg" against the registry
    When I run "luabox publish --yank 1.1.0" in "pkg" against the registry
    Then the command succeeds
    And stdout contains "yanked `mathlib@1.1.0`"
    And ".registry/index/ma/th/mathlib" contains "1.1.0"
    Given a file "app/luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.0.1"
      edition = "5.4"

      [dependencies]
      mathlib = "^1"
      """
    When I run "luabox install" in "app" against the registry
    Then the command succeeds
    And "app/luabox.lock" contains 'version = "1.0.0"'
    And "app/luabox.lock" does not contain "1.1.0"

  Scenario: scoped packages publish and install under their org directory
    Given a file "pkg/luabox.toml" containing:
      """
      [package]
      name = "@acme/util"
      version = "0.2.0"
      edition = "5.4"
      """
    And a file "pkg/src/init.lua" containing:
      """
      return {}
      """
    When I run "luabox publish" in "pkg" against the registry
    Then the command succeeds
    And the file ".registry/index/@acme/util" exists
    And the file ".registry/artifacts/@acme/util/0.2.0.tar" exists
    Given a file "app/luabox.toml" containing:
      """
      [package]
      name = "app"
      version = "0.0.1"
      edition = "5.4"

      [dependencies]
      "@acme/util" = "^0.2"
      """
    When I run "luabox install" in "app" against the registry
    Then the command succeeds
    And the file "app/lua_modules/@acme/util/src/init.lua" exists
    And "app/luabox.lock" contains 'name = "@acme/util"'

  Scenario: publish refuses a path dependency in the manifest
    Given a file "pkg/luabox.toml" containing:
      """
      [package]
      name = "mathlib"
      version = "1.0.0"
      edition = "5.4"

      [dependencies]
      helper = { path = "../helper" }
      """
    And a file "pkg/src/init.lua" containing:
      """
      return {}
      """
    When I run "luabox publish" in "pkg" against the registry
    Then the command fails
    And stderr contains "registry consumers cannot resolve"

  Scenario: publish without a configured registry gives setup guidance
    Given a file "pkg/luabox.toml" containing:
      """
      [package]
      name = "mathlib"
      version = "1.0.0"
      edition = "5.4"
      """
    When I run "luabox publish" in "pkg" without a registry
    Then the command fails
    And stderr contains "LUABOX_REGISTRY"
