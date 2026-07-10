Feature: Advisory audit — luabox audit (SPEC.md §6, §14)

  `luabox audit` matches `luabox.lock` against a local, directory-of-TOML
  advisory database (RUSTSEC-analog; no hosted feed exists yet). The
  database location is `LUABOX_ADVISORY_DB`, else `~/.luabox/advisory-db`.
  When neither exists the command prints a note and exits 0 — a security
  check must never fail a build just because no database was configured.
  Findings render as `LB1100`; critical/high are errors (nonzero exit),
  medium/low are warnings; a malformed advisory file is a warning on stderr
  and does not stop the rest of the audit.

  These scenarios are hermetic: `LUABOX_ADVISORY_DB` always points at a
  scenario-local fixture directory (or a deliberately missing one), never at
  a real machine's `~/.luabox/advisory-db`.

  Scenario: auditing without a lockfile fails with a clear message
    Given a project with edition "5.4"
    When I run "luabox audit"
    Then the command fails
    And stderr contains "luabox install"

  Scenario: no advisory database configured prints a note and succeeds
    Given a project with edition "5.4"
    And a file "luabox.lock" containing:
      """
      version = 1
      """
    When I run "luabox audit" with env "LUABOX_ADVISORY_DB=missing-advisory-db"
    Then the command succeeds
    And stdout contains "no advisory database configured"

  Scenario: a vulnerable locked dependency is flagged with a nonzero exit
    Given a project with edition "5.4"
    And a file "luabox.lock" containing:
      """
      version = 1

      [[package]]
      name = "insecure-pkg"
      version = "1.0.0"
      source = "registry"
      """
    And a file "advisory-db/LBSEC-2026-0001.toml" containing:
      """
      id = "LBSEC-2026-0001"
      package = "insecure-pkg"
      severity = "high"
      title = "Remote code execution via eval"
      description = "insecure-pkg evaluates untrusted input passed to run()."
      affected = ["<2.0.0"]
      """
    When I run "luabox audit" with env "LUABOX_ADVISORY_DB=advisory-db"
    Then the command fails
    And stdout contains "LB1100"
    And stdout contains "LBSEC-2026-0001"

  Scenario: a patched version is not flagged
    Given a project with edition "5.4"
    And a file "luabox.lock" containing:
      """
      version = 1

      [[package]]
      name = "insecure-pkg"
      version = "2.5.0"
      source = "registry"
      """
    And a file "advisory-db/LBSEC-2026-0001.toml" containing:
      """
      id = "LBSEC-2026-0001"
      package = "insecure-pkg"
      severity = "high"
      title = "Remote code execution via eval"
      description = "insecure-pkg evaluates untrusted input passed to run()."
      affected = ["<3.0.0"]
      patched = [">=2.0.0"]
      """
    When I run "luabox audit" with env "LUABOX_ADVISORY_DB=advisory-db"
    Then the command succeeds
    And stdout does not contain "LB1100"

  Scenario: a malformed advisory warns but does not stop the audit
    Given a project with edition "5.4"
    And a file "luabox.lock" containing:
      """
      version = 1

      [[package]]
      name = "insecure-pkg"
      version = "1.0.0"
      source = "registry"
      """
    And a file "advisory-db/broken.toml" containing:
      """
      id = "not-a-valid-id"
      package = "insecure-pkg"
      severity = "extreme"
      """
    And a file "advisory-db/LBSEC-2026-0002.toml" containing:
      """
      id = "LBSEC-2026-0002"
      package = "insecure-pkg"
      severity = "critical"
      title = "Arbitrary file write"
      description = "insecure-pkg writes files outside its sandbox."
      affected = ["<2.0.0"]
      """
    When I run "luabox audit" with env "LUABOX_ADVISORY_DB=advisory-db"
    Then the command fails
    And stderr contains "warning:"
    And stdout contains "LBSEC-2026-0002"

  Scenario: a clean lockfile against an unrelated advisory succeeds with zero findings
    Given a project with edition "5.4"
    And a file "luabox.lock" containing:
      """
      version = 1

      [[package]]
      name = "safe-pkg"
      version = "1.0.0"
      source = "registry"
      """
    And a file "advisory-db/LBSEC-2026-0003.toml" containing:
      """
      id = "LBSEC-2026-0003"
      package = "other-pkg"
      severity = "high"
      title = "Unrelated advisory"
      description = "Does not affect safe-pkg."
      affected = ["<9.9.9"]
      """
    When I run "luabox audit" with env "LUABOX_ADVISORY_DB=advisory-db"
    Then the command succeeds
    And stdout contains "0 finding"

  Scenario: a medium-severity finding warns without failing the command
    Given a project with edition "5.4"
    And a file "luabox.lock" containing:
      """
      version = 1

      [[package]]
      name = "insecure-pkg"
      version = "1.0.0"
      source = "registry"
      """
    And a file "advisory-db/LBSEC-2026-0004.toml" containing:
      """
      id = "LBSEC-2026-0004"
      package = "insecure-pkg"
      severity = "medium"
      title = "Denial of service via crafted input"
      description = "insecure-pkg hangs on a crafted input."
      affected = ["<2.0.0"]
      """
    When I run "luabox audit" with env "LUABOX_ADVISORY_DB=advisory-db"
    Then the command succeeds
    And stdout contains "LB1100"
    And stdout contains "LBSEC-2026-0004"
