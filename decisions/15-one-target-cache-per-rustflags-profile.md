---
status: Accepted
date: 2026-09-06
---
# Decision 15 — One shared build-directory cache per RUSTFLAGS profile, none on one-shot jobs: the pipeline is sized to the runner, not to the ideal

## Context

`.gitlab-ci.yml` is a port of six GitHub Actions workflows (decision 14 clause
1 makes it the pipeline of record). The port carried across the source's cache
shape faithfully: every rust job got its own `target-<job>` build-directory
cache, mirroring the per-job `key:` the Actions workflows pass to
`Swatinem/rust-cache`.

On GitHub that shape is free. Each key lives in a hosted, evicted, per-repo
cache store, and the jobs run on separate ephemeral machines. Neither property
holds here:

- Of five online runners, only runner 3 has `run_untagged: true`, and nothing
  in this file is tagged (deliberately — see NARROWING in the file header). So
  every job in every pipeline lands on one host.
- This instance has no shared cache server. The runner's own trace says so:
  *"No URL provided, cache will not be downloaded from shared cache server.
  Instead a local version of cache will be extracted"*. Every cache is a local
  zip on that host's filesystem, and nothing evicts it.

Measured on 2026-09-06 (#85). Twelve multi-GB build-directory zips — nine
`target-<job>` plus three `fuzz-target-<matrix leg>` — sat on that filesystem
alongside the live workspace of every concurrent pipeline. Four MR pipelines
built concurrently. Jobs failed with the disk full, and they failed
*illegibly*: `rustc-LLVM ERROR: IO failure on output stream: No space left on
device` (job 28833), `collect2: fatal error: ld terminated with signal 7 [Bus
error]` linking under `target/llvm-cov-target` (28851, an mmap on a full
filesystem), `no profile can be merged` (28966, profraw writes), and
`fatal: sha1 file .git/index.lock write error` — none of which name the disk.
After a manual prune of the runner's cache directory, pipeline 20271 hit the
same wall within fifteen minutes (28963, 28965, 28967, plus
`Failed to extract cache`).

The owner's call was the pipeline half, not the host half: the alternative was
provisioning runner 3 with an estimated ≥150 GB free and/or setting
`concurrent = 1` so pipelines serialise. That option is not foreclosed — see
*When to revisit*.

## Decision

1. **One build-directory cache per RUSTFLAGS profile, not per job.** The nine
   `target-<job>` keys collapse into a single `target-deny`, keyed on
   `Cargo.lock` + `rust-toolchain.toml` as before. The key names the *profile*
   because `RUSTFLAGS` is what makes two build directories mutually useless:
   an artifact compiled under `-Dwarnings` and one compiled without it have
   different fingerprints, so sharing a directory across the two profiles
   would make every run a full rebuild *and* keep the disk cost. The
   `DENY-WARNINGS` reasoning that split the caches in the first place is
   correct and survives; what changes is that it is now the only thing
   allowed to split them.

   There is no `target-default` cache today. `luals-parity` is the only
   default-RUSTFLAGS rust job and it is one-shot (point 2). If a
   default-profile job ever needs a build cache, it gets `target-default` —
   it does not join `target-deny`.

2. **A single writer, everyone else `policy: pull`.** `check` writes
   `target-deny`; `ignored-deterministic-tests` and `perf-gates` read it.
   Concurrent jobs sharing a key all write at job end and the last one wins,
   so the surviving content oscillates between their disjoint artifact sets —
   the disk is paid for a cache that rarely hits. `check` is the writer
   because its tree is the broadest debug build in the pipeline
   (`clippy --workspace --all-targets`, `test --workspace`), so its artifacts
   are the most reusable. `cargo-home` is likewise `policy: pull` on the jobs
   that only consume the registry.

3. **No build-directory cache on one-shot or self-contained jobs.**
   `examples`, `differential`, `verdict-differential` and `luals-parity` each
   build a release binary once and then run scripts against it; nothing
   downstream reuses the tree. The `*-selftest` jobs and `draft-install-mock`
   run no cargo at all. The `fuzz` matrix loses `fuzz/target` — three nightly
   build directories for a 60-second smoke run. They keep the caches that are
   small and actually compound: `cargo-home` (pull), the pinned `luals` and
   `pwsh` trees, and `fuzz/corpus`.

4. **Coverage caches nothing but `cargo-home`.** `target/llvm-cov-target`
   holds LLVM-instrumented artifacts — the largest tree this repo produces —
   and `coverage-unit` and `coverage-e2e` compile *disjoint* target sets in
   the same stage. A single shared key between them would have each job
   extract a multi-GB zip that does not match what it is about to build, then
   overwrite it at job end for the other job to mis-extract next run. Peak
   disk unchanged, hit rate approaching nil. Two separate keys is the shape
   that caused the incident. Neither is worth it, so coverage rebuilds.

5. **`df -h "$CI_PROJECT_DIR"` is the first line of every rust job.**
   `.rust`'s `before_script` reports it and never fails on it. This does not
   prevent anything; it makes a full disk legible in the first ten lines
   instead of arriving as an LLVM IO error four hundred lines into a compile.
   Decision 12's principle applied to the harness itself: a failure mode
   nobody can read is one nobody fixes.

Measured effect: 19 distinct runtime cache keys become 8; the multi-GB
build-directory caches among them go from 12 to 1.

## Consequences

- **Lower hit rate, on purpose.** Three jobs now share one build directory
  written by a fourth. `ignored-deterministic-tests` and `perf-gates` build
  `--release`; `check` caches `--debug`, so their release artifacts are never
  cached by anyone and they pay a cold release build every run. That is the
  price of one writer. If it proves worse than the disk it saves, the knob is
  which job writes — not more keys.
- **Coverage rebuilds from scratch every run.** Both coverage jobs already
  carry a 2h timeout for exactly this reason (see TIMEOUTS in the file
  header), so the budget absorbs it; the wall-clock cost is real and will show
  up as the check stage's long pole.
- **`fuzz` recompiles its nightly targets every run.** The corpus — the thing
  that actually compounds across runs — is still cached, so the gate keeps its
  memory. Only the build is repaid.
- **Nothing about the gates themselves changes.** No floor, stage, rule,
  timeout or script moved. Decision 14 clause 7's quality bar is untouched:
  this is a change to how the pipeline stores intermediates, not to what it
  measures. A cache change that altered a verdict would be a different, worse
  decision.
- **This is a fit-the-host decision, not an engineering ideal.** The per-job
  caches were not wrong; they were unaffordable on one shared disk with no
  eviction.

## When to revisit

Any of these makes the trade stale, and each is a reason to reopen it rather
than to accumulate keys quietly:

- Runner 3 gets substantially more disk (est. ≥150 GB free for the original
  shape), or the runner config sets `concurrent = 1` so pipelines serialise.
- The instance gains a shared cache server (S3/GCS), which brings eviction and
  moves the storage off the executor's filesystem — at which point per-job
  keys become affordable again.
- More untagged runners appear, so that "every job lands on one host" stops
  being true. Note this cuts both ways: it spreads the disk *and* it makes a
  local cache miss on any host but the one that wrote it.

Until one of those holds, do not add a build-directory cache key to this file
without measuring the host first. The disk is the constraint; the cache is the
variable.
