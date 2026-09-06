---
status: Accepted
date: 2026-09-06
---
# Decision 15 — Fixed cache keys, host-side eviction: the cache leaked because nothing ever deleted a key, not because there were too many keys

## Context

`.gitlab-ci.yml` is a port of six GitHub Actions workflows (decision 14 clause
1 makes it the pipeline of record). The port carried across the source's cache
shape faithfully: every rust job got its own `target-<job>` build-directory
cache, keyed — as `Swatinem/rust-cache` keys — on `Cargo.lock` plus
`rust-toolchain.toml`.

On GitHub both halves of that shape are free. Each key lives in a hosted,
evicted, per-repo cache store, and the jobs run on separate ephemeral
machines. Neither property held here:

- Of five online runners, only runner 3 has `run_untagged: true`, and nothing
  in this file is tagged (deliberately — see NARROWING in the file header). So
  every job in every pipeline lands on one host.
- This instance has no shared cache server. The runner's own trace says so:
  *"No URL provided, cache will not be downloaded from shared cache server.
  Instead a local version of cache will be extracted"*. Every cache is a local
  zip on that host's filesystem.

**Measured (2026-09-06, #85).** Jobs failed with the disk full, and they failed
*illegibly*: `rustc-LLVM ERROR: IO failure on output stream: No space left on
device` (job 28833), `collect2: fatal error: ld terminated with signal 7 [Bus
error]` linking under `target/llvm-cov-target` (28851, an mmap on a full
filesystem), `no profile can be merged` (28966, profraw writes), and
`fatal: sha1 file .git/index.lock write error` — none of which name the disk.
After a manual prune of the runner's cache directory, pipeline 20271 hit the
same wall within fifteen minutes (28963, 28965, 28967, plus
`Failed to extract cache`). A refill that fast is the load-bearing measurement:
twelve build-directory caches cannot rewrite themselves in fifteen minutes.
Something was *adding*.

**Diagnosed on the host (2026-09-06 13:15 UTC).** Two independent faults, and
between them they explain the whole incident:

1. **The disk was the wrong disk.** `/var/lib/docker` is an 80 GB `docker.img`
   btrfs loop while the NVMe cache pool has 672 GB free. The runner's
   `[runners.docker] volumes = ["/cache"]` made `/cache` and every slot's
   `/builds` anonymous docker volumes *inside that image* (6 GB per slot,
   measured); the runner container's own binds of the appdata directories were
   dead, because job containers do not inherit them. `concurrent` was `6`, not
   the 2 the registration script sets — six Rust builds at once, load 29. The
   image also carried 65 images, 13 dangling volumes and 22 stale `runner-*`
   volumes.
2. **Nothing ever deleted a cache.** GitLab's local cache has no eviction
   whatsoever, and `key:` with `files:` produces a *content hash*: every
   change to `Cargo.lock` or `rust-toolchain.toml` writes a **new** zip under a
   **new** key and leaves the old one on disk forever. Twelve keys × every
   dependency bump on every branch, accumulating. That is the unbounded growth,
   and it is why pruning bought minutes.

The host was fixed: `concurrent = 2`, and `/cache` and `/builds` bind-mounted
from the NVMe pool. A daily host sweep now evicts `/cache` zips untouched for
>14 d (oldest first, 60 GiB cap) and `/builds` checkouts untouched for >7 d.

That leaves the pipeline half, and it changes what the pipeline half should be.
The first response to this incident (commit 38d9016, superseded by this
decision) collapsed the twelve build-directory caches into one shared
`target-deny` with a single writer. Against the real defect that is the wrong
lever, and not a cheap one: `perf-gates` and `ignored-deterministic-tests`
build `--release` against a key holding `check`'s debug tree, so both pay a
cold release build every run, and any two concurrent jobs sharing one key
overwrite each other at job end. It gave up hit-rate to bound a growth it did
not bound — a hashed key is unbounded whether there are twelve of them or one.

## Decision

1. **Every cache key in `.gitlab-ci.yml` is fixed.** A literal (`cargo-home`,
   `fuzz-cargo-home`), or a literal plus a variable naming the job, the matrix
   leg or a pinned version (`target-$CI_JOB_NAME_SLUG`,
   `fuzz-target-$FUZZ_TARGET`, `luals-$LUALS_VERSION`). No `key: files:`, no
   `prefix:`, anywhere. One archive per key, overwritten in place at the end of
   every job. Total cache size is bounded by (number of keys × one tree) and
   stays there.

   This is safe because cargo already does what the hashed key was doing.
   Cargo's fingerprinting invalidates and rebuilds the units a lockfile or
   toolchain bump affects; the hashed key merely guaranteed the tree was empty
   first. That was a storage decision dressed as a correctness one.

2. **Per-job build-directory caches stay.** `target-$CI_JOB_NAME_SLUG` for
   every rust job, `fuzz-target-$FUZZ_TARGET` per matrix leg, the coverage
   jobs' trees included. The jobs do not agree on RUSTFLAGS (see DENY-WARNINGS
   in the file header) or on profile, so sharing one directory makes them evict
   each other's artifacts every run: peak disk paid for a cache that
   structurally misses. With fixed keys the shape is self-limiting — N rust
   jobs is N trees, permanently — so the disk argument that motivated
   collapsing them no longer exists.

3. **`policy: pull` where a job only consumes.** `examples`, `differential`,
   `verdict-differential` and `luals-parity` read `cargo-home` and add nothing
   to it that `check` has not already put there. Re-uploading a
   multi-hundred-MB zip of identical content is disk churn for nothing. This
   part of 38d9016 survives on its own merits: it is not a disk-capacity
   argument, it is a write-amplification one.

4. **The host is half of this decision.** `/builds` and `/cache` on the NVMe
   bind mounts, `concurrent = 2`, and the daily sweep (14 d / 60 GiB for
   `/cache`, 7 d for `/builds`) are load-bearing, not incidental hygiene. Fixed
   keys are what make a sweep *sufficient*: a key that is still in use is
   touched on every run, so it never ages out, and the only things the sweep
   collects are keys the pipeline has genuinely stopped using — a deleted job,
   a bumped `luals` pin, an abandoned fuzz target. Against hashed keys the same
   sweep would be a race it loses every time a lockfile changes.

5. **`df -h "$CI_PROJECT_DIR"` is the first line of every rust job and of
   `fuzz`.** It reports and never fails. This prevents nothing; it makes a full
   disk legible in the first ten lines instead of arriving as an LLVM IO error
   four hundred lines into a compile. Decision 12's principle applied to the
   harness itself: a failure mode nobody can read is one nobody fixes.

## Consequences

- **A stale archive can carry artifacts nobody will use again.** With a fixed
  key, a lockfile bump leaves the previous dependency's `.rlib`s in the cached
  tree until cargo overwrites or ignores them. Cost: some dead weight inside a
  bounded archive. That is the trade, and it is the right way round — the
  alternative was a clean tree per bump and no upper bound on the total.
- **Cache size is now predictable.** Twelve build-directory keys plus
  `cargo-home`, `fuzz-cargo-home`, the corpora and the two pinned tool trees.
  One copy of each. The 60 GiB sweep cap is a ceiling with headroom, not a
  target the pipeline races towards.
- **Hit-rate is back where the port had it.** `perf-gates` and
  `ignored-deterministic-tests` keep their own release trees instead of paying
  a cold release build against `check`'s debug key; coverage keeps
  `target/llvm-cov-target`; the fuzz legs keep their nightly build directories
  alongside the corpora they were already compounding.
- **Two hosts' worth of state is now one contract.** The pipeline assumes the
  bind mounts, `concurrent = 2` and the sweep. That assumption is written into
  the CACHING header of `.gitlab-ci.yml`, not just here, because the file is
  where someone will be standing when it stops being true.
- **Nothing about the gates themselves changes.** No floor, stage, rule,
  timeout or script moved; the only script edit is the `df -h` report line.
  Decision 14 clause 7's quality bar is untouched: this is a change to how the
  pipeline stores intermediates, not to what it measures. A cache change that
  altered a verdict would be a different, worse decision.

## When to revisit

The host contract is an input to this decision. If it changes, re-derive
rather than patch:

- **The sweep stops running, or its thresholds move.** Fixed keys bound the
  steady state, but a key that falls out of use (a renamed job, a bumped pin)
  is only collected by the sweep. Without it those accumulate slowly — slowly
  is not never.
- **`concurrent` rises above 2, or more untagged runners appear.** Both raise
  peak *live workspace* usage, which is `/builds`, not `/cache`, and which no
  cache change addresses. Note the second cuts both ways: it spreads the disk
  *and* it makes a local cache miss on every host but the one that wrote it.
- **`/builds` or `/cache` stops being an NVMe bind mount** — e.g. a rebuilt
  runner container reverting to `volumes = ["/cache"]`. That puts multi-GB
  trees back inside the 80 GB `docker.img` and reproduces the original
  incident with the same illegible symptoms. Check this first when
  `No space left on device` returns.
- **The instance gains a shared cache server (S3/GCS).** That brings its own
  eviction and moves storage off the executor's filesystem, at which point the
  sweep and possibly the fixed keys stop being the mechanism that matters.

Until one of those holds, the rule is narrow and worth stating plainly: **do
not add a `key:` with `files:` to this file.** A key you cannot name is a key
nobody can delete.
