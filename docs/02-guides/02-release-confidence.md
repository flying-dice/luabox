# Release-confidence handoff

A passing suite is evidence for its fixtures, not proof that a release has no
defects. Record the candidate commit, binary checksum, operating system, command,
exit status, log location and all skips for every check below. A blocked row is
not a pass. This runbook tracks the remaining work in GitLab issue #92.

## Linux candidate checks

Use a disposable checkout; example scripts generate artifacts. Install reference
Lua 5.1–5.4 interpreters/compilers and `unzip` on PATH. Build the exact candidate:

```sh
cargo build --release --locked -p luabox-cli
export LUABOX_E2E_BIN="$PWD/target/release/luabox"
export LUABOX="$LUABOX_E2E_BIN"
sha256sum "$LUABOX"
cargo test --locked -p luabox-cli --test acceptance --test lsp_acceptance
cargo test --locked -p luabox-cli --test watch --test atomic_rewrite --test broken_pipe
bash scripts/examples.sh
bash scripts/tests/control-flow-differential.sh
bash scripts/tests/verdict-differential.sh
bash scripts/perf-gate.sh
```

The last three Rust integration targets may use Cargo's debug executable rather
than the override: inspect their implementation and report that distinction.
Do not loosen performance budgets simply to obtain a pass. The control-flow
matrix contains documented exceptions; report them alongside its result.

Install the LuaLS version and checksum pinned in `.gitlab-ci.yml`, then run:

```sh
LUALS_REQUIRED=1 LUALS=/absolute/path/to/lua-language-server \
  bash scripts/tests/luals-differential.sh
```

Keep `LUALS_REQUIRED=1`: an absent reference must not become a green skipped run.
Run the bounded fuzz commands and durations in the current GitLab CI definition,
preserving seeds and crash artifacts. Fuzzing is bounded evidence, not exhaustive.

## Real editor-host checks

On a graphical dev box or a CI extension-host harness, build/package the matching
extension revision following its own README. Record its revision and supported
editor version. Install the package into a clean profile without competing Lua
extensions, and point its binary setting at the candidate above.

Create a two-file project: `greetings.lua` exports an annotated `greet(string)`
function through a module table; `main.lua` imports and calls it. Verify:

- A numeric argument reports an error, corrected text clears it without restart.
- Hover, completion, signature help, definition, references and rename agree on
  the imported symbol; definition selects the actual declaration range.
- Formatting/code actions preserve code; save/reopen and unsaved edits work.
- Rename/delete/recreate files, switch workspaces and restart the server: no
  stale diagnostics, duplicate processes or lost edits.
- Every contributed command works with the candidate or is explicitly disabled
  with an actionable compatibility explanation.
- Missing/invalid executable configuration produces an actionable error.

For VS Code, automate with an extension-host test runner and its provider-command
APIs; assert returned locations/edits and diagnostics, not merely activation.
Test the packaged VSIX as well as development sources. For JetBrains use that
repository's supported IDE test harness and repeat the same observable contract.
Attach host logs and screenshots for checks that cannot be asserted via APIs.

## Platform and artifact matrix

Repeat supported checks on macOS Apple Silicon and Windows x86_64. Include paths
with spaces/Unicode, write failures, watch events and archive-tool fallbacks.
Test generated output on LuaJIT and actual LÖVE/Neovim hosts where claimed.
Test installation and upgrade of the candidate artifact in an isolated install
directory, including checksum rejection and preservation of the old executable
on failure. Do not publish a release merely to make this test possible; use the
documented draft-artifact workflow. Installing an older public release is not
evidence for the candidate.

## Independent workflow checks

Use a real annotated dependency and representative application beyond the
repository fixtures. Compare original versus formatted, lint-fixed, bundled and
lowered runtime output, including side effects/error paths; verify second-run
formatter/fix idempotence. Exercise watch create/rename/delete and manifest edits.
Validate valid and invalid manifests with an independent schema validator.

For every new defect record a minimal reproducer, why prior tests missed it,
and a regression test that fails before the fix. Dependency advisories need
build-only/runtime reachability triage and an explicit disposition.

## Sign-off

Publish a PASS/FAIL/BLOCKED matrix with evidence links and tracked exceptions.
Resolve defects or obtain explicit owner acceptance; do not close an unexecuted
test handoff because its instructions are complete. Release publication remains
a separate owner-authorized action.
