---
column: doing
labels: [wave, review-round]
priority: high
agent: opus-w10
live: true
status: agent brief being written
updatedAt: 2026-07-29T13:25:00.000Z
---
# Wave 10 — round-4 findings (bundler prefix trivia, --watch loop, column linearity)

F1: bundle must strip/relocate SHEBANG/BOM trivia when splicing modules (crates/luabox-bundle/src/lib.rs:270-296) and stop silently dropping the entry shebang under --minify. F2: filter notify event kinds and drain self-triggered events in the --watch loop (crates/luabox-cli/src/watch.rs:90-128). F3: O(1)-amortized column lookup + rustc-style long-line truncation in the human renderer (crates/luabox-diag/src/line_index.rs, render.rs). F4: ci.yml gate comment. F5: docs/03-reference/02-limitations.md off-by-one.

## Checklist

- [ ] On merge: record the bundler file-prefix policy (entry shebang kept at bundle top, no BOM emitted) as decisions/08
- [ ] On merge: verify LIMITATIONS/CHANGELOG edits landed in docs/03-reference/02-limitations.md, not the root stub

## Comments

- **opus-w10** (2026-07-29T13:25:00.000Z): All 5 round-4 findings fixed on sprint/w10-round4-fixes. F1: `split_file_prefix` cuts BOM+`#!` per module using the lexer's own rule and `shift_back` moves the require-rewrite ranges (crates/luabox-bundle/src/lib.rs:375, :409, :431); the entry's shebang is re-emitted at byte 0 ahead of the banner, never a BOM (crates/luabox-bundle/src/lib.rs:440). The cut keeps the shebang's trailing newline so `.lua.map` line numbers do not move. Release-binary matrix 2x2x2x2 (entry x dep shebang x 5.4/5.1 x plain/minify): 16/16 build rc 0, 16/16 run rc 0 under the matching lua5.x, shebang present at byte 0 iff the entry had one; BOM'd dependency bundles clean and the output carries no mark; 14 new e2e scenarios in crates/luabox-cli/tests/features/syntax/file-prefix.feature:138+. F2: `triggers_rerun` drops non-change event kinds at the watcher callback (crates/luabox-cli/src/watch.rs:192) and every rerun now drains its own side effects (crates/luabox-cli/src/watch.rs:160); `Access(Close(Write))` still triggers, `Modify(Metadata(AccessTime))` does not, mtime-only `touch` does. Before: 148 reruns/30s idle after one edit; after: 1 rerun then silence, and edit+touch+edit still gives 3. Bounded-loop regression test at crates/luabox-cli/tests/watch.rs:149 verified to FAIL on the pre-fix module. F3: `IndexedSource` gained a UTF-8 continuation-byte table so columns are two `partition_point`s (crates/luabox-diag/src/line_index.rs:48, :103) and `window_line` bounds the human renderer's line output (crates/luabox-diag/src/render.rs:200, :262). 377 kB single-line / 10k findings, `check`: human 71 028 ms + 3.5 GB -> 545 ms + 4.6 MB, sarif 2 489 -> 713 ms, github 2 166 -> 476 ms, gitlab 2 300 -> 582 ms, json 541 -> 586 ms; doubling 5k/10k/20k scales ~2.1x per step in all five formats. Byte identity measured, not assumed: an 851-line/440-diagnostic corpus renders byte-identical under pre- and post-change binaries in all five formats. F4: .github/workflows/ci.yml:107 now enumerates all seven perf legs and defers to the script. F5: docs/03-reference/02-limitations.md:160 corrected to 511 terms (measured: 511 parses, 512 errors), plus a new windowing caveat at :164. Gates: `cargo test --workspace` green (740 e2e + 187 lsp scenarios, all unit suites), clippy `-D warnings` clean, `cargo fmt --all --check` clean, unit coverage 96.63% lines with every crate >= 92 (bundle 96.59 / cli 92.54 / diag 97.15), e2e coverage 86.04% (floor 83), `scripts/perf-gate.sh` 7/7 PASS at LUABOX_PERF_FACTOR=2 (the clean-corpus `check` warm leg sits at ~1.5-1.8 s on this box, so 1.5 is marginal here; that leg renders no diagnostics and is untouched by these changes).
