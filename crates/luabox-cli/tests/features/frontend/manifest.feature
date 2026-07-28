Feature: luabox.toml validation — every problem, reported at once
  SPEC.md §5 + §14: the manifest is validated before any command does work,
  so every command that reads it (here: `luabox check`) refuses to run on an
  invalid one and exits 1. Validation is *batch* — a single parse reports
  every problem in the file rather than stopping at the first — and unknown
  keys carry a cargo-style did-you-mean nudge alongside the valid set.

  Scenario Outline: a mistyped key is rejected with a did-you-mean nudge
    Given a manifest whose [<section>] table contains '<line>'
    When I run "luabox check"
    Then the command exits with code 1
    And stderr contains "<message>"

    Examples: tables and keys whose valid set is closed
      | section      | line                                       | message                                                            |
      | typo         | strict = true                              | unknown top-level table `typo`, did you mean `types`?              |
      | package      | verson = "0.1.0"                           | unknown [package] key `verson`, did you mean `version`?            |
      | build        | targt = "5.4"                              | unknown [build] key `targt`, did you mean `target`?                |
      | types        | strct = true                               | unknown [types] key `strct`, did you mean `strict`?                |
      | dependencies | foo = { git = "https://x", revision = "a" } | unknown `dependencies.foo` key `revision`                          |
      | lint         | style = "den"                              | unknown lint level for `style` `den`, did you mean `deny`?         |

  Scenario Outline: a table 0.2.0 removed names the removal, not a typo
    # A 0.1.4 manifest carries these across the upgrade untouched (#18). The
    # generic unknown-table error would send the reader hunting for a
    # misspelling; these two never were one.
    Given a manifest whose [<section>] table contains '<line>'
    When I run "luabox check"
    Then the command exits with code 1
    And stderr contains "unknown top-level table `<section>`"
    And stderr contains "removed in 0.2.0, see CHANGELOG.md"

    Examples: the two tables the v1 scope cut dropped
      | section   | line             |
      | tasks     | test = "busted"  |
      | workspace | members = ["a"]  |

  Scenario Outline: a key holding the wrong TOML type names the type it wants
    Given a manifest whose [<section>] table contains '<line>'
    When I run "luabox check"
    Then the command exits with code 1
    And stderr contains "<message>"

    Examples: scalars, arrays, and array entries
      | section | line                    | message                                          |
      | package | name = 42               | `package.name` must be a string                  |
      | package | description = 7         | `package.description` must be a string           |
      | types   | strict = "yes"          | `types.strict` must be a boolean                 |
      | types   | defs = "stdlib.lua"     | `types.defs` must be an array of strings         |
      | types   | defs = [1]              | `types.defs` entries must be strings             |
      | build   | bundle = "yes"          | `build.bundle` must be a boolean                 |
      | build   | minify = 1              | `build.minify` must be a boolean                 |
      | build   | entry = "src/main.lua"  | `build.entry` must be an array of strings        |
      | build   | entry = [true]          | `build.entry` entries must be strings            |
      | lint    | style = 1               | `lint.style` must be a level string (allow, warn, deny) |
      | lint    | globals = "love"        | `lint.globals` must be an array of strings       |

  Scenario Outline: a value outside the allowed set is listed against it
    Given a manifest whose [<section>] table contains '<line>'
    When I run "luabox check"
    Then the command exits with code 1
    And stderr contains "<message>"

    Examples: dialects and bundle modes
      | section | line                          | message                                                                 |
      | build   | target = "5.9"                | invalid build.target `5.9` (valid: 5.1, 5.2, 5.3, 5.4, luajit)          |
      | build   | mode = "nope"                 | invalid build.mode `nope` (valid: plain, love, nvim-plugin)             |
      | package | lua-versions = ["5.4", "6.0"] | invalid package.lua-versions entry `6.0` (valid: 5.1, 5.2, 5.3, 5.4, luajit) |
      | package | lua-versions = [54]           | `package.lua-versions` entries must be strings                          |

  Scenario Outline: a malformed package name explains which rule it broke
    Given a manifest whose [package] table contains 'name = <name>'
    When I run "luabox check"
    Then the command exits with code 1
    And stderr contains "`package.name`"
    And stderr contains "<reason>"

    Examples: plain and scoped `@scope/name` forms
      | name          | reason                                            |
      | "Fixture_Bad" | must be lowercase ASCII alphanumeric or `-`       |
      | "9lives"      | must not start with a digit                       |
      | "@scope"      | is scoped but not of the form `@scope/name`       |
      | "@a/"         | must not have an empty scope or name segment      |

  Scenario Outline: a version that is not semver-shaped is rejected
    Given a manifest whose [package] table contains '<key> = <version>'
    When I run "luabox check"
    Then the command exits with code 1
    And stderr contains "`package.<key>`"
    And stderr contains "doesn't look like semver"

    Examples: `X.Y.Z` with optional pre-release and build metadata
      | key                | version    |
      | version            | "1.0"      |
      | version            | "1.2.3.4"  |
      | version            | "vv.b.c"   |
      | version            | "1..3"     |
      | min-luabox-version | "1"        |

  Scenario: the semver hint spells out the shape it expected
    Given a manifest whose [package] table contains 'version = "1.0"'
    When I run "luabox check"
    Then the command exits with code 1
    And stderr contains "(expected X.Y.Z, optional -pre-release/+build)"

  Scenario Outline: a semver-shaped version is accepted
    Given a manifest whose [package] table contains 'version = <version>'
    When I run "luabox check"
    Then the command succeeds

    Examples: the shapes the check deliberately allows
      | version         |
      | "0.1.0"         |
      | "1.2.3-alpha.1" |
      | "1.2.3+build7"  |
      | "1.2.3-rc1+ci"  |

  Scenario Outline: a dependency entry must name exactly one source kind
    Given a manifest whose [<section>] table contains '<line>'
    When I run "luabox check"
    Then the command exits with code 1
    And stderr contains "<message>"

    Examples: git / path / url shapes and their integrity rules
      | section          | line                                                | message                                                                 |
      | dependencies     | foo = 42                                            | `dependencies.foo` must be a version-requirement string or an inline table |
      | dependencies     | foo = {}                                            | `dependencies.foo` must specify one of `git`, `path`, `url`, or `version` |
      | dependencies     | foo = { tag = "v1" }                                | `dependencies.foo` has a git reference key but no `git` source          |
      | dependencies     | foo = { git = "https://x", path = "../foo" }        | `dependencies.foo` must specify only one of `git`, `path`, or `url`     |
      | dependencies     | foo = { git = "https://x", rev = "a", tag = "b" }   | `dependencies.foo` must specify at most one of `rev`, `tag`, `branch`   |
      | dependencies     | foo = { path = "../foo", sha256 = "abc" }           | `dependencies.foo.sha256` is only valid alongside a `url` source        |
      | dependencies     | foo = { git = 5 }                                   | `dependencies.foo.git` must be a string                                 |
      | dev-dependencies | bar = 7                                             | `dev-dependencies.bar` must be a version-requirement string or an inline table |

  Scenario: an http(s) tarball dependency must pin its digest
    Given a manifest whose [dependencies] table contains 'foo = { url = "https://x/foo.tar.gz" }'
    When I run "luabox check"
    Then the command exits with code 1
    And stderr contains "has a `url` source but no `sha256`"
    And stderr contains "must pin its SHA-256 digest for integrity"

  Scenario Outline: a well-formed dependency entry is accepted
    Given a manifest whose [dependencies] table contains '<line>'
    When I run "luabox check"
    Then the command succeeds

    Examples: one source kind, at most one git reference
      | line                                                          |
      | foo = "1.0"                                                   |
      | foo = { path = "../foo" }                                     |
      | foo = { git = "https://x", tag = "v1" }                       |
      | foo = { url = "https://x/f.tar.gz", sha256 = "abc" }          |

  Scenario Outline: a section that is not a table says so
    Given a file "luabox.toml" containing:
      """
      <section> = 5

      [package]
      edition = "5.4"
      """
    When I run "luabox check"
    Then the command exits with code 1
    And stderr contains "`[<section>]` must be a table"

    Examples: every table-shaped section
      | section      |
      | build        |
      | types        |
      | lint         |
      | dependencies |

  Scenario: a missing [package] table is a hard error
    Given a file "luabox.toml" containing:
      """
      [build]
      target = "5.4"
      """
    When I run "luabox check"
    Then the command exits with code 1
    And stderr contains "missing required table `[package]`"

  Scenario: [package] is required to declare an edition
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "fixture"
      """
    When I run "luabox check"
    Then the command exits with code 1
    And stderr contains "missing required key `package.edition`"

  Scenario: an unknown edition lists the dialects luabox knows
    Given a file "luabox.toml" containing:
      """
      [package]
      edition = "5.9"
      """
    When I run "luabox check"
    Then the command exits with code 1
    And stderr contains "invalid package.edition `5.9` (valid: 5.1, 5.2, 5.3, 5.4, luajit)"

  Scenario: every problem in the file is reported, not just the first
    Given a file "luabox.toml" containing:
      """
      [package]
      name = "Bad_Name"
      version = "1.0"
      edition = "5.9"
      """
    When I run "luabox check"
    Then the command exits with code 1
    And stderr contains "must be lowercase ASCII alphanumeric"
    And stderr contains "doesn't look like semver"
    And stderr contains "invalid package.edition `5.9`"

  Scenario: malformed TOML is reported with the line and column it broke at
    Given a file "luabox.toml" containing:
      """
      [package
      edition = "5.4"
      """
    When I run "luabox check"
    Then the command exits with code 1
    And stderr contains "TOML parse error at line 1, column 9"
    And stderr contains "invalid table header"

  Scenario: the manifest path is named in the failure so the file is findable
    Given a manifest whose [types] table contains 'strict = 1'
    When I run "luabox check"
    Then the command exits with code 1
    And stderr contains "luabox.toml"

  # --- deliberately permissive corners ------------------------------------

  Scenario: lint rule ids stay open — only the level value is validated
    Given a manifest whose [lint] table contains 'some-future-rule = "deny"'
    When I run "luabox check"
    Then the command succeeds

  Scenario: an empty package name is left to the rockspec to supply
    Given a manifest whose [package] table contains 'name = ""'
    When I run "luabox check"
    Then the command succeeds

  Scenario: an explicitly empty entry list is a library with nothing to bundle
    Given a manifest whose [build] table contains 'entry = []'
    When I run "luabox check"
    Then the command succeeds

  Scenario: a version-only dependency table is the bare-string form spelled longhand
    Given a manifest whose [dependencies] table contains 'foo = { version = "1.0" }'
    When I run "luabox check"
    Then the command succeeds
