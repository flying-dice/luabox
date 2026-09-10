#!/bin/bash
# Guard: a workspace crate's [dependencies] entry used by NOTHING its
# lib/bin targets compile — either dead weight, or (the shape #81/#82 were
# filed over) a dependency only exercised by that crate's own tests, which
# belongs in [dev-dependencies] instead. 4fc7ad3 fixed exactly one instance
# by hand (luabox-lint's luabox-types dep); this is the gate that would have
# caught it, so the next one is found by CI instead of by review (#82).
#
# Mechanism: rustc's own allow-by-default `unused_crate_dependencies` lint,
# denied via RUSTFLAGS (the lint has no [lints] table knob of its own worth
# trusting here — see the NOT a [workspace.lints] entry note below), run
# over `--lib --bins` ONLY. Excluding `--tests`/`--benches`/`--examples`/
# `--doc` is the whole trick: those targets compile with `dev-dependencies`
# and (`--tests`) under `cfg(test)`, so a dependency a crate's tests alone
# reach would otherwise look used. `--lib --bins` is exactly the edge that
# ships, and `unused_crate_dependencies` is evaluated per compiled crate
# unit, so the plain `lib` unit is judged on its own even when other units
# for the same package are compiled alongside it.
#
# NOT a [workspace.lints.rust] table entry: that would deny the lint for
# EVERY cargo invocation (`cargo test`, `cargo build`, a developer's `cargo
# check --all-targets`), where dev-dependencies and test-only use are
# legitimate and the lint's per-unit accounting does the right thing anyway
# only for non-test units — widening it there would either do nothing (if
# left at `warn`) or start flagging every test-only dev-dependency as a
# workspace-wide error (if set to `deny`, since `[lints] workspace = true`
# applies the same level to every target in every member crate, tests
# included). RUSTFLAGS scoped to this one job is the narrow edge; see
# scripts/tests/unused-crate-deps-guard-selftest.sh case S9 for the
# empirical reason `-D` (not `-W`) is what is wired.
#
# Failure attribution: cargo running under this RUSTFLAGS can fail for
# reasons that have nothing to do with an unused dependency — a genuine
# compile error in a crate, a Cargo.lock `--locked` refuses to update, or
# cargo itself dying (linker crash, OOM/SIGKILL). Reporting any of those as
# "you have an unused dependency" is exactly the "measures something other
# than what it claims" shape decisions/12 rules out, so this script always
# runs with `--message-format=json`, inspects the JSON stream for the
# lint's OWN diagnostic (`is unused in crate`) before ever printing the
# unused-dependency remediation text, and prints everything else — cargo's
# own stderr plus whatever compiler diagnostics DID fire — under a distinct,
# honestly-labelled header instead (see the two branches below). See
# scripts/tests/unused-crate-deps-guard-selftest.sh cases S5-S6.
#
# Positive target-visitation proof: a `--lib` (or `--bins`) silently
# dropped from the line below does not always make cargo say so (a
# `--bins`-only run over a workspace that still has a `[[bin]]` target
# elsewhere exits 0 with no warning, having quietly stopped checking every
# `lib` target). Trusting cargo's exit code alone would let that kind of
# edit to this file pass its own gate. Instead: `cargo metadata` enumerates
# every workspace member's lib/bin targets up front, and the SAME
# `--message-format=json` stream above is cross-checked for a
# `compiler-artifact` message against every one of them (cargo emits one
# per compiled unit whether or not it was rebuilt, so a warm target cache
# does not blind this) — a target on the expected list with no matching
# artifact fails the job even though cargo itself exited 0. See
# scripts/tests/unused-crate-deps-guard-selftest.sh case S4. python3 is
# already a standing CI dependency (`apt_install python3` already appears in
# a dozen other jobs in .gitlab-ci.yml — the coverage and perf-gate jobs
# among them), so parsing JSON with it, rather than trying to grep
# `--message-format=json` output, is the smallest mechanism this CI image
# reliably has, not a new one; the job that runs this script installs it the
# same way.
#
# `--locked` refuses a Cargo.lock this checkout would otherwise rewrite —
# a drifted lockfile silently checking a different dependency set than the
# one committed is exactly the "measures something other than what it
# claims" shape decisions/12 rules out. Pinned by
# scripts/tests/unused-crate-deps-guard-selftest.sh cases S6 (dropping the
# committed lock still fails, correctly attributed as unrelated to the
# lint) and S8 (a mutant with `--locked` removed lets that same broken lock
# pass silently).
#
# Empirically confirmed against this workspace (no allowlist — baseline is
# clean): every `[dependencies]` entry in every one of the 11 `crates/*`
# manifests is used by that crate's own `lib`/`bin` target; none of them
# carry a build.rs, a `[features]` table, benches or examples, so the false
# positive classes the lint is known for upstream (feature-gated deps,
# build-script-only deps, example/bench-only deps) have no live case in
# this workspace to trigger on. `tools/differ` and `fuzz/` are workspace
# `exclude`d and out of scope for the same reason `check`'s header already
# gives for `tools/differ`: this gate inherits `cargo check --workspace`'s
# own member list, nothing more.
#
# Scope this gate does NOT claim:
#   - dev-dependencies hygiene: an actually-unused dev-dep is a different,
#     lower-stakes bug this lint does not see at all on `--lib --bins`.
#   - non-Linux platforms: this gate only compiles code reachable under
#     THIS CI runner's host target (x86_64-unknown-linux-gnu, the `.rust`
#     job template's `rust:1.92-trixie` image) — it is asserted to hold
#     there and nowhere else. A dependency reachable ONLY from code behind
#     a platform `cfg` this target does not satisfy (`#[cfg(windows)]`,
#     `#[cfg(target_os = "macos")]`, ...) would look unused here even if
#     genuinely used off-Linux, because cargo strips that code before this
#     lint ever sees it. Checked against this workspace: every
#     `#[cfg(windows)]` / `#[cfg(unix)]` / `#[cfg(target_os = ...)]` site
#     that exists today (luabox-cli's atomic_write.rs, modes.rs, doc_cmd.rs;
#     luabox-lsp's server.rs; luabox-manifest's layout.rs) reaches only std
#     (`std::process::Command`, platform-specific `std::fs` calls) — none of
#     them reach an extern crate exclusively through a platform `cfg`, so
#     this scope limit has no live false-positive case today. Re-check this
#     paragraph the day a dependency is added for one platform only.
set -euo pipefail

repo_dir="${UNUSED_CRATE_DEPS_REPO_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
cd "$repo_dir"

if ! command -v python3 >/dev/null 2>&1; then
    echo "error: python3 is required to validate Cargo's structured output" >&2
    exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

check_log="$work/cargo-check.json"
check_status=0
RUSTFLAGS="-D unused_crate_dependencies" cargo check --workspace --lib --bins --locked \
    --message-format=json >"$check_log" 2>"$work/cargo-check.stderr" || check_status=$?

# dump_diagnostics — cargo's own stderr (top-level errors: manifest parse
# failures, --locked lockfile refusals, cargo dying outright) plus every
# rendered compiler diagnostic pulled back out of the JSON stream. Used by
# both failure branches below so neither one has to re-derive it.
dump_diagnostics() {
    if [ -s "$work/cargo-check.stderr" ]; then
        cat "$work/cargo-check.stderr"
    fi
    python3 - "$check_log" <<'PY'
import json
import sys

with open(sys.argv[1]) as handle:
    for line in handle:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        if msg.get("reason") == "compiler-message":
            rendered = msg.get("message", {}).get("rendered")
            if rendered:
                sys.stdout.write(rendered)
PY
}

if [ "$check_status" -ne 0 ]; then
    if python3 - "$check_log" <<'PY'
import json
import sys

with open(sys.argv[1]) as handle:
    for line in handle:
        try:
            message = json.loads(line)
        except ValueError:
            continue
        diagnostic = message.get("message", {})
        code = diagnostic.get("code") or {}
        if message.get("reason") == "compiler-message" and code.get("code") == "unused_crate_dependencies":
            raise SystemExit(0)
raise SystemExit(1)
PY
    then
        echo "error: a [dependencies] entry above is unused by its crate's lib/bin" >&2
        echo "       target(s). If it is only reachable from that crate's own tests," >&2
        echo "       move it to [dev-dependencies] (see 4fc7ad3, #81, #82); if nothing" >&2
        echo "       reaches it at all, remove it." >&2
        dump_diagnostics >&2
        exit 1
    fi
    echo "error: cargo check failed for a reason unrelated to the" >&2
    echo "       unused-crate-dependencies lint this guard checks for (a compile" >&2
    echo "       error, a Cargo.lock --locked refused to rewrite, cargo itself" >&2
    echo "       dying, ...). This is NOT an unused-dependency finding — see the" >&2
    echo "       cargo output below for the actual cause." >&2
    dump_diagnostics >&2
    exit 1
fi

# cargo exited 0 — prove it actually visited every intended lib/bin target
# rather than trusting that alone (see the "Positive target-visitation
# proof" header note above).
metadata_log="$work/cargo-metadata.json"
cargo metadata --no-deps --format-version 1 --locked >"$metadata_log"

visitation_log="$work/visitation.log"
if ! python3 - "$metadata_log" "$check_log" >"$visitation_log" 2>&1 <<'PY'
import json
import sys

LIB_KINDS = {"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"}


def categorize(kinds):
    kinds = set(kinds)
    cats = set()
    if kinds & LIB_KINDS:
        cats.add("lib")
    if "bin" in kinds:
        cats.add("bin")
    return cats


metadata_path, artifacts_path = sys.argv[1], sys.argv[2]

with open(metadata_path) as handle:
    metadata = json.load(handle)

expected = set()
members = set(metadata["workspace_members"])
for pkg in metadata["packages"]:
    if pkg["id"] not in members:
        continue
    for tgt in pkg.get("targets", []):
        for cat in categorize(tgt.get("kind", [])):
            expected.add((pkg["id"], tgt["name"], cat))

if not expected:
    print(
        "no workspace lib/bin targets found in `cargo metadata` output — "
        "this proves nothing, which decisions/12 treats as a failure, not a pass",
        file=sys.stderr,
    )
    sys.exit(1)

visited = set()
with open(artifacts_path) as handle:
    for line in handle:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        if msg.get("reason") != "compiler-artifact":
            continue
        pkg_id = msg.get("package_id")
        target = msg.get("target", {})
        for cat in categorize(target.get("kind", [])):
            visited.add((pkg_id, target.get("name"), cat))

missing = sorted(expected - visited)
if missing:
    for pkg_id, name, cat in missing:
        print(f"  never visited: {cat} target '{name}' in {pkg_id}", file=sys.stderr)
    sys.exit(1)
PY
then
    echo "error: cargo check exited 0 but did not visit every intended lib/bin" >&2
    echo "       target above — a flag was silently dropped from this script's" >&2
    echo "       cargo invocation without cargo itself saying so:" >&2
    cat "$visitation_log" >&2
    exit 1
fi

exit 0
