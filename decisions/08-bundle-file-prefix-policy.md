---
status: Accepted
date: 2026-07-29
---
# Decision 08 — A bundle keeps the entry's shebang and never carries a BOM

## Context

Wave 8b made `#!` shebangs and UTF-8 BOMs legal file-prefix trivia, but the
bundler spliced raw module text, so a prefix landed mid-bundle where `#` is
the length operator (round-4 finding F1: `check` passed, `build --bundle`
died with an internal error; `--minify` silently dropped the shebang).
Fixing it forced a policy choice about what a bundle's own file prefix is.

## Decision

The bundle preserves the ENTRY module's shebang at byte 0 of the output — a
bundled executable script stays executable — for plain and minified output
alike. Dependencies' shebangs are stripped (their newline kept, so sourcemap
line numbers do not move). No BOM is ever emitted: the bundle is a new file,
and a 5.1-target bundle carrying one would not load on the reference
interpreter. Implementation: crates/luabox-bundle/src/lib.rs
(`split_file_prefix`, `shift_back`, the emit path).

## Consequences

`./dist/main.lua` works when `./src/main.lua` did; 16/16 build-and-execute
matrix (entry/dep shebang x 5.4/5.1 x plain/minify) verified under real
interpreters. A BOM'd source under `[build] target = "5.1"` is still rejected
by name — pre-existing, correct for tree mode which ships sources verbatim,
and now pinned by a test so the choice is explicit.
