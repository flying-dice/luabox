---
column: doing
labels: [wave, review-round]
priority: high
agent: opus-w11
live: true
status: agent launching
updatedAt: 2026-07-29T14:44:30.000Z
---
# Wave 11 — round-5 findings (watch drain swallow, EPIPE abort)

F1: stop the post-rerun drain from discarding real edits (crates/luabox-cli/src/watch.rs:160,:243) - the triggers_rerun kind filter already prevents the loop; drained events that pass the filter must be acted on, not thrown away. Regression test: an edit inside the drain window must be reported. F2: BrokenPipe on stdout must exit quietly instead of SIGABRT/134 (crates/luabox-cli/src/project.rs:39 and every report print path).

## Comments

- **opus-w11** (2026-07-29T14:44:30.000Z): Both round-5 findings fixed on `sprint/w11-round5-fixes`. **F1** — took option (a), deleted the discarding drain outright, after auditing notify 8.2 per backend: inotify maps a read to `Access(_)` (filtered), FSEvents and `ReadDirectoryChangesW` do not report reads at all (windows never subscribes `FILE_NOTIFY_CHANGE_LAST_ACCESS`), so `triggers_rerun` alone kills the loop; the audit table and the kqueue caveat are documented where the drain was, crates/luabox-cli/src/watch.rs:70-109, and the two call sites are gone at crates/luabox-cli/src/watch.rs:173-188. Unit test swapped for crates/luabox-cli/src/watch.rs:654 (an edit queued during a run survives to be the next batch); new integration regression at crates/luabox-cli/tests/watch.rs:246 — verified FAILED 5/5 against the pre-fix watch.rs (git stash) and passes after. Reviewer gap sweep (edit A, breaking edit B at 0.05/0.15/0.25/0.35/0.5 s): pre-fix reported yes/yes/**no**/**no**/yes (0.25 and 0.35 swallowed, identical across 2 runs); post-fix yes at every gap, 2/2 runs. Loop still dead: one edit -> 1 rerun -> 30 s silence; `fmt --watch` now formats the file saved 300 ms later (pre-fix it was left unformatted) and converges in 3 reruns. **F2** — new seam crates/luabox-cli/src/emit.rs: `classify` maps `ErrorKind::BrokenPipe` to `Flow::ReaderGone` (pure, unit tested), `to_stdout`/`to_stderr` are the only two `process::exit` edges (0 on a departed reader, 1 with one stderr line on any other write failure); `outln!`/`out!`/`errln!`/`err!` replace all 29 std print call sites across the CLI, including crates/luabox-cli/src/project.rs:41. Before (release, `| head -2`): status **134**, SIGABRT + panic text, all five formats. After: status 0, empty stderr, all five formats; a fully-read failing run still exits 1. New integration test crates/luabox-cli/tests/broken_pipe.rs (4 cases), 3/4 verified failing pre-fix; added to the unit-coverage leg in .github/workflows/ci.yml:66. CHANGELOG.md `[Unreleased]`/`Fixed`: wave-10 watch entry rewritten (the "drains whatever that run stirred up" claim is gone), plus one new entry per finding. docs/03-reference/02-limitations.md needed no change — it makes no `--watch` claim. Verification: `cargo test --workspace` green (2215 tests + 740 acceptance + 187 LSP scenarios), clippy `-D warnings` clean, `cargo fmt --all --check` clean, `scripts/perf-gate.sh` at LUABOX_PERF_FACTOR=2 all 7 legs PASS, luabox-cli unit coverage 92.59% (floor 92).
