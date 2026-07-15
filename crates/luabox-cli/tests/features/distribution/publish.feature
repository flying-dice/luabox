Feature: publish to luarocks.org
  luabox follows the pnpm/bun model: luarocks.org IS the registry and the
  project's `*.rockspec` IS the authored package manifest (SPEC.md §6, #2).
  `luabox publish` is a thin proxy that uploads the rockspec you wrote,
  verbatim — it generates nothing. `--dry-run` validates and previews the
  upload without touching the network; a real publish needs a luarocks.org API
  key (`luabox login --luarocks`). These scenarios are hermetic: no network is
  reached (the dry-run never uploads, and the no-key/mismatch cases fail before
  any upload).

  Scenario: --dry-run validates and previews the rockspec without uploading
    Given a file "luabox.toml" containing:
      """
      [package]
      edition = "5.4"
      """
    And a file "app-0.1.0-1.rockspec" containing:
      """
      rockspec_format = "3.0"
      package = "app"
      version = "0.1.0-1"
      source = { url = "git+https://github.com/me/app.git" }
      dependencies = { "lua >= 5.1" }
      build = { type = "builtin", modules = {} }
      """
    When I run "luabox publish --dry-run"
    Then the command succeeds
    And stdout contains "rockspec_format"
    And stdout contains "git+https://github.com/me/app.git"
    And stdout contains "upload target"
    And stdout contains "dry run"

  Scenario: publish with no API key configured errors with onboarding guidance
    Given a file "luabox.toml" containing:
      """
      [package]
      edition = "5.4"
      """
    And a file "app-0.1.0-1.rockspec" containing:
      """
      rockspec_format = "3.0"
      package = "app"
      version = "0.1.0-1"
      source = { url = "git+https://github.com/me/app.git" }
      dependencies = { "lua >= 5.1" }
      build = { type = "builtin", modules = {} }
      """
    When I run "luabox publish"
    Then the command fails
    And stderr contains "luabox login --luarocks"
    And stderr contains "luarocks.org/settings/api-keys"

  Scenario: a rockspec whose filename disagrees with its contents is refused
    Given a file "luabox.toml" containing:
      """
      [package]
      edition = "5.4"
      """
    And a file "app-0.2.0-1.rockspec" containing:
      """
      package = "app"
      version = "0.1.0-1"
      source = { url = "git+https://github.com/me/app.git" }
      build = { type = "builtin", modules = {} }
      """
    When I run "luabox publish --dry-run"
    Then the command fails
    And stderr contains "app-0.1.0-1.rockspec"

  Scenario: a project with no rockspec has nothing to publish
    Given a file "luabox.toml" containing:
      """
      [package]
      edition = "5.4"
      """
    When I run "luabox publish --dry-run"
    Then the command fails
    And stderr contains "no `*.rockspec`"

  Scenario: a C/native rock is refused — luabox publishes pure-Lua rocks only
    Given a file "luabox.toml" containing:
      """
      [package]
      edition = "5.4"
      """
    And a file "app-0.1.0-1.rockspec" containing:
      """
      package = "app"
      version = "0.1.0-1"
      source = { url = "git+https://github.com/me/app.git" }
      build = { type = "make", modules = {} }
      """
    When I run "luabox publish --dry-run"
    Then the command fails
    And stderr contains "pure-Lua rocks only"
