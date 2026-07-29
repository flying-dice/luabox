---
column: done
labels: [review-round]
priority: high
updatedAt: 2026-07-29T15:29:00.000Z
---
# Round-6 independent adversarial review

Fresh reviewer after wave 11 merges. Trajectory: 8 -> 5 -> 9 -> 5 -> 2; exit requires zero.

## Comments

- **claude** (2026-07-29T15:29:00.000Z): Round-6 verdict: FAIL, exactly 1 finding (MAJOR): two bare eprintln! sites in the LSP crate - crates/luabox-lsp/src/server.rs:269 and :543 - SIGABRT the language server when stderr's reader is gone (reviewer's repro: invalid luabox.toml + dead stderr = signal 6; controls isolated it to the log lines). Wave 11's EPIPE sweep stopped at the luabox-cli crate boundary. EVERYTHING else held under attack: the drain deletion proven sound (gap sweep 10/10, atomic/vim saves, delete+recreate, mid-rerun edits, kernel inotify mask read from /proc), fmt --watch termination explained from code (write-only-on-change fixed point) and guarded by the proptest'd formatter idempotency, EPIPE exit-code policy checked against the repo's written contract and accepted, bundle builds byte-deterministic, SIGTERM/SIGKILL mid-fmt tears zero files, 233 markdown files 0 broken links. Reviewer's exit statement: fix this one finding and no remaining improvement path blocks production readiness.
- **claude** (2026-07-29T15:29:00.000Z): Fixed directly on develop (two-line fix with the reviewer's specified shape): new log_to_stderr helper in crates/luabox-lsp/src/server.rs drops the message on a dead pipe instead of panicking - survival policy, not the CLI's exit-0, per the in-code rationale. Verified with the reviewer's exact repro: invalid manifest + stderr closed at spawn = ALIVE after 6s, clean shutdown/exit 0 (was signal -6). CHANGELOG's stderr claim amended to state the LSP's survival policy. Gates: luabox-lsp 289+132 tests, 187 LSP scenarios, clippy/fmt clean.