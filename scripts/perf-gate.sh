#!/usr/bin/env bash
# SPEC.md §16.1 perf gates (CI-blocking): cold start < 50 ms; `check` on a
# 100-kLOC corpus < 1 s warm (LSP keystroke-to-diagnostics gate is future
# work, not covered here).
#
# Gates: cold start, `fmt --check` throughput (kept as a wider safety
# net), the real `check` gate (live since GL#6), a diagnostics-heavy
# `lint` + `check` gate in two variants — findings suppressed, and
# findings reported — and a peak-RSS gate on the same 100-kLOC corpus.
#
# The memory gate exists because decisions/07 *accepted* a ~1.9x peak-RSS
# regression (64 -> 123 MiB on this corpus) in exchange for one read/parse
# per file, and nothing enforced the other side of that bargain: `check`
# could have grown to 500 MiB on the same input and every gate would still
# have been green. Now the accepted number has a ceiling.
#
# Why those last gates exist: the ~100-kLOC corpus the first three legs
# use is *clean* (`check: 0 errors, 0 warnings`), so none of them ever
# exercises per-diagnostic work. That blind spot hid an
# O(diagnostics x file size) line lookup: 32 k findings in one file took
# ~32 s to lint, while the same file with a single finding took 0.35 s.
# The diagnostics-heavy legs feed one big file full of findings to both
# commands. The *suppressed* pair times the per-finding bookkeeping with
# the renderer deliberately out of the way; the *rendered* pair times the
# whole path a user actually pays for, renderer included — which is where
# a second, larger O(diagnostics x file size) cost lived (one byte-0 line
# scan *and* one whole-file clone per label) until the renderers were
# given a line table of their own.
#
# Env:
#   LUABOX_PERF_FACTOR   float multiplier applied to every budget, for
#                        slow/loaded machines (antivirus scanning new
#                        binaries, a busy shared runner, an underpowered
#                        dev laptop). Default 1.0. CI is the real
#                        enforcement point and should not need to set
#                        this; if a dev's machine can't hit 1.0, that's a
#                        machine problem, not evidence the gate is wrong.
#                        Example: LUABOX_PERF_FACTOR=3 scripts/perf-gate.sh
#   LUABOX_RSS_BUDGET_MIB
#                        integer ceiling (MiB) for the peak-RSS leg.
#                        Default 300. Deliberately NOT scaled by
#                        LUABOX_PERF_FACTOR: a slow or loaded machine runs
#                        the same allocations, it just takes longer over
#                        them, so a CPU multiplier has no business
#                        loosening a memory ceiling. Override it only when
#                        the *budget* is being renegotiated.
#
# Usage: scripts/perf-gate.sh
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

factor="${LUABOX_PERF_FACTOR:-1.0}"

cold_start_budget_base_ms=50
fmt_budget_base_ms=2000
check_budget_base_ms=1000
# Diagnostics-heavy legs, on one file holding `diag_corpus_findings`
# findings. Calibrated on the dev baseline (see CHANGELOG "Fixed", wave 8):
#   lint   357 ms fixed / 14 163 ms with the quadratic lookup restored
#   check  663 ms fixed /  4 183 ms ditto
# The budgets sit ~3x above the fixed numbers (headroom for a noisy box,
# on top of LUABOX_PERF_FACTOR) and ~3-12x below the broken ones.
diag_lint_budget_base_ms=1200
diag_check_budget_base_ms=1500
# The same two corpora with nothing suppressed, so every finding is also
# *rendered*. Calibrated the same way (dev baseline, 20 k findings):
#   lint   162 ms fixed /  9 295 ms with the quadratic renderer restored
#   check  788 ms fixed / 15 462 ms ditto
# Budgets ~3x above the fixed numbers and >=6x below the broken ones.
# Rendered `lint` is *faster* than the suppressed leg above because
# resolving 20 k `---@luabox-ignore` directives costs more than printing
# 20 k frames — the suppressed leg is not a subset of this one, which is
# why both stay.
diag_lint_rendered_budget_base_ms=600
diag_check_rendered_budget_base_ms=2400
diag_corpus_findings=20000
# Peak-RSS ceiling for `check` on the 100-kLOC corpus, in MiB. decisions/07
# accepted 64 -> 123 MiB; measured 123 MiB (three runs, identical) on the dev
# baseline, so the budget sits at ~2.4x that. Wide on purpose: this is a
# ceiling that catches a *regime* change (a whole-project structure retained,
# a per-file clone that used to be a borrow), not a 10% drift — allocator
# behaviour and page-cache pressure vary too much between machines for a tight
# memory gate to mean anything. NOT scaled by LUABOX_PERF_FACTOR; see the Env
# block above.
rss_budget_mib=300

# Plain `awk` (POSIX, present on every CI/dev box we target) does the
# float multiply; everything else is integer ms from here on.
cold_start_budget=$(awk -v b="$cold_start_budget_base_ms" -v f="$factor" 'BEGIN { printf "%d", b * f }')
fmt_budget=$(awk -v b="$fmt_budget_base_ms" -v f="$factor" 'BEGIN { printf "%d", b * f }')

echo "perf-gate: LUABOX_PERF_FACTOR=${factor} (cold-start budget ${cold_start_budget} ms, fmt budget ${fmt_budget} ms)"

echo "perf-gate: building release binaries..."
cargo build --release -p luabox-cli
cargo build --release --manifest-path tools/gen-corpus/Cargo.toml --target-dir target/gen-corpus

luabox_bin="$repo_root/target/release/luabox"
gen_corpus_bin="$repo_root/target/gen-corpus/release/gen-corpus"
if [[ -f "${luabox_bin}.exe" ]]; then
  luabox_bin="${luabox_bin}.exe"
  gen_corpus_bin="${gen_corpus_bin}.exe"
fi

corpus_dir="$(mktemp -d)"
cleanup() { rm -rf "$corpus_dir"; }
trap cleanup EXIT

echo "perf-gate: generating ~100 kLOC corpus into ${corpus_dir} ..."
"$gen_corpus_bin" --out "$corpus_dir/src" --seed 42 --files 50 --lines-per-file 2000

cat > "$corpus_dir/luabox.toml" <<'EOF'
[package]
name = "perf-gate-corpus"
version = "0.0.0"
edition = "5.4"

[build]
target = "5.4"
out = "dist"

[types]
strict = true

[dependencies]
EOF

fail=0

# --- Cold start: MIN of N runs -------------------------------------------
# Min (not mean/median) is the right statistic for a cold-start *ceiling*:
# it's the best this machine can do free of scheduler/IO noise from other
# processes. Percentiles would blend in unrelated noise; min isolates the
# binary's own startup cost, which is what the 50 ms budget is about.
echo
echo "perf-gate: cold start (luabox --version), 10 runs, taking MIN..."
min_ms=""
for i in $(seq 1 10); do
  start=$(date +%s%N)
  "$luabox_bin" --version >/dev/null
  end=$(date +%s%N)
  ms=$(( (end - start) / 1000000 ))
  echo "  run ${i}: ${ms} ms"
  if [[ -z "$min_ms" || "$ms" -lt "$min_ms" ]]; then
    min_ms=$ms
  fi
done

if [[ "$min_ms" -lt "$cold_start_budget" ]]; then
  echo "PASS cold start: ${min_ms} ms < ${cold_start_budget} ms"
else
  echo "FAIL cold start: ${min_ms} ms >= ${cold_start_budget} ms"
  fail=1
fi

# --- fmt --check throughput gate (warm) -----------------------------------
# The corpus is synthetic and not guaranteed to already be in canonical
# form, so `fmt --check` may legitimately exit nonzero here; that's not a
# gate failure — only the elapsed time is. The gate would only fail if
# wall time exceeds the budget.
echo
echo "perf-gate: fmt --check throughput on corpus (warm)..."
( cd "$corpus_dir" && "$luabox_bin" fmt --check >/dev/null 2>&1 ) || true
start=$(date +%s%N)
( cd "$corpus_dir" && "$luabox_bin" fmt --check >/dev/null 2>&1 ) || true
end=$(date +%s%N)
fmt_ms=$(( (end - start) / 1000000 ))

if [[ "$fmt_ms" -lt "$fmt_budget" ]]; then
  echo "PASS fmt --check (warm): ${fmt_ms} ms < ${fmt_budget} ms"
else
  echo "FAIL fmt --check (warm): ${fmt_ms} ms >= ${fmt_budget} ms"
  fail=1
fi

# --- CHECK GATE ------------------------------------------------------------
# SPEC.md §16.1: `check` on the 100-kLOC corpus < 1 s warm. Live since
# GL#6; the fmt --check gate above stays as the wider safety net.
check_budget=$(awk -v b="$check_budget_base_ms" -v f="$factor" 'BEGIN { printf "%d", b * f }')
echo
echo "perf-gate: check throughput on corpus (warm)..."
( cd "$corpus_dir" && "$luabox_bin" check >/dev/null 2>&1 ) || true
start=$(date +%s%N)
( cd "$corpus_dir" && "$luabox_bin" check >/dev/null 2>&1 ) || true
end=$(date +%s%N)
check_ms=$(( (end - start) / 1000000 ))
if [[ "$check_ms" -lt "$check_budget" ]]; then
  echo "PASS check (warm): ${check_ms} ms < ${check_budget} ms"
else
  echo "FAIL check (warm): ${check_ms} ms >= ${check_budget} ms"
  fail=1
fi

# --- PEAK-RSS GATE ---------------------------------------------------------
# The other half of decisions/07's bargain. `check` is the command that
# retains per-file artifacts across both of its passes, and the 100-kLOC
# corpus above is the input that number was measured on, so this reuses both
# rather than inventing a third fixture. Warm, like every leg here.
#
# The measurement is `wait4(2)`'s rusage — what `/usr/bin/time -v` reports —
# read through python3's stdlib (scripts/peak-rss.py). One mechanism for Linux
# and macOS, no packages, and python3 is already a CI dependency (the coverage
# job runs scripts/per-crate-coverage.py). Where it is missing, the leg SKIPs
# loudly rather than failing, the way scripts/examples.sh skips its lua5.1
# step — a missing measuring tool is not evidence of a regression.
rss_budget="${LUABOX_RSS_BUDGET_MIB:-$rss_budget_mib}"
echo
if command -v python3 >/dev/null 2>&1; then
  echo "perf-gate: peak RSS of check on corpus (warm)..."
  ( cd "$corpus_dir" && "$luabox_bin" check >/dev/null 2>&1 ) || true
  if rss_mib="$( cd "$corpus_dir" && python3 "$repo_root/scripts/peak-rss.py" "$luabox_bin" check )"; then
    if [[ "$rss_mib" -lt "$rss_budget" ]]; then
      echo "PASS check peak RSS: ${rss_mib} MiB < ${rss_budget} MiB"
    else
      echo "FAIL check peak RSS: ${rss_mib} MiB >= ${rss_budget} MiB"
      echo "     decisions/07 accepted 123 MiB on this corpus; see scripts/peak-rss.py"
      fail=1
    fi
  else
    echo "FAIL check peak RSS: could not measure"
    fail=1
  fi
else
  echo "perf-gate: SKIP peak RSS — no python3 on PATH (it reads the child's rusage)"
fi

# --- RETAINED-TYPEENV REGRESSION GATE --------------------------------------
# `luabox check`'s cross-file pass once shared one `TypeEnv` per file across
# the export pass and the check pass instead of building and dropping one
# transiently in each (round 4 review R14, reverted by round 4 review
# finding 6 — see check_cmd.rs's `run_passes`/`check_one` doc comments and
# luabox-types/src/lib.rs's `build_file_env` doc comment). It bought ~1.76x
# CPU (one ambient clone per file instead of two) for a peak-memory
# regression that grows with (file count x ambient size): every file's env
# had to stay alive until the whole export pass finished, so peak RSS scaled
# O(files^2) on a project where file count and workspace-global class count
# scale together — not O(files), the way the CPU-time saving alone would
# suggest. The PEAK-RSS GATE above would not catch it coming back: its
# 50-file/100-kLOC corpus only widens the retained-vs-transient gap to
# ~3-5x, nowhere near a ceiling sized for a ~1.9x accepted trade.
#
# Reproduced here as its own leg on a corpus shaped to actually grow the
# ambient with N: an N-file project, two `---@class` declarations per file,
# and a linear `require` chain (module i requires module i-1). Measured on
# the CURRENT (correct, transient) implementation and, separately, on a
# deliberately reintroduced retained-`Vec<TypeEnv>` implementation
# (check_cmd.rs's `file_exports`/`check_one` patched back to R14's shape),
# three runs each, release build, peak RSS via scripts/peak-rss.py:
#
#   N      transient (correct)   retained (R14, reintroduced)   ratio
#   100     11 MiB                 51 MiB                        4.6x
#   200     15 MiB                145 MiB                        9.3x
#   300     20 MiB                290 MiB                       14.5x
#   500     29 MiB                734 MiB                       25.3x
#
# N=500 is the size check_cmd.rs's own doc comment already cites (~29x
# peak RSS on the project that measurement was taken on), the gap here is
# already >25x at that size, and generating + checking 500 tiny files costs
# under 2s warm — cheap enough for every CI run, unlike a corpus anywhere
# near N=500 built from the ~100-kLOC generator's 2000-line files (that
# would be 1M lines and minutes, not seconds). The budget sits at 100 MiB:
# just over 3x above the correct implementation's ~29 MiB (headroom for
# machine noise, same rationale as the PEAK-RSS GATE above), while the
# reintroduced retained implementation misses it by ~7x (734 MiB) — not a
# close call in either direction. Deliberately NOT scaled by
# LUABOX_PERF_FACTOR, for the same reason as $rss_budget_mib above: a slow
# machine runs the same allocations, it just takes longer over them.
retained_env_corpus_files=500
retained_env_rss_budget_mib="${LUABOX_RETAINED_ENV_RSS_BUDGET_MIB:-100}"
echo
if command -v python3 >/dev/null 2>&1; then
  echo "perf-gate: generating ${retained_env_corpus_files}-file retained-TypeEnv regression corpus..."
  retained_env_dir="$corpus_dir/retained-env/src"
  mkdir -p "$retained_env_dir"
  cat > "$corpus_dir/retained-env/luabox.toml" <<'EOF'
[package]
name = "perf-gate-retained-env"
version = "0.0.0"
edition = "5.4"

[build]
target = "5.4"
out = "dist"

[types]
strict = true

[dependencies]
EOF
  for ((i = 0; i < retained_env_corpus_files; i++)); do
    f="$retained_env_dir/mod_${i}.lua"
    {
      if ((i > 0)); then
        printf 'local prev = require("mod_%d")\n\n' "$((i - 1))"
      fi
      printf -- '---@class Widget%dA\n---@field n number\nlocal A = { n = %d }\n\n' "$i" "$i"
      printf -- '---@class Widget%dB\n---@field n number\nlocal B = { n = %d }\n\n' "$i" "$i"
      if ((i > 0)); then
        printf 'return { a = A, b = B, prev = prev }\n'
      else
        printf 'return { a = A, b = B }\n'
      fi
    } > "$f"
  done

  echo "perf-gate: peak RSS of check on the retained-TypeEnv regression corpus (warm)..."
  ( cd "$corpus_dir/retained-env" && "$luabox_bin" check >/dev/null 2>&1 ) || true
  if retained_env_rss_mib="$( cd "$corpus_dir/retained-env" && python3 "$repo_root/scripts/peak-rss.py" "$luabox_bin" check )"; then
    if [[ "$retained_env_rss_mib" -lt "$retained_env_rss_budget_mib" ]]; then
      echo "PASS retained-TypeEnv regression: ${retained_env_rss_mib} MiB < ${retained_env_rss_budget_mib} MiB"
    else
      echo "FAIL retained-TypeEnv regression: ${retained_env_rss_mib} MiB >= ${retained_env_rss_budget_mib} MiB"
      echo "     round 4 review R14 (reverted) measured ~734 MiB on this corpus; see check_cmd.rs's run_passes doc comment"
      fail=1
    fi
  else
    echo "FAIL retained-TypeEnv regression: could not measure"
    fail=1
  fi
else
  echo "perf-gate: SKIP retained-TypeEnv regression — no python3 on PATH (it reads the child's rusage)"
fi

# --- DIAGNOSTICS-HEAVY GATE ------------------------------------------------
# The corpus above is clean, so nothing so far times per-diagnostic work.
# These four legs do: one file, `diag_corpus_findings` findings, run twice
# over — once with every finding *suppressed*, once with every finding
# *reported*.
#
# The suppressed pair keeps the renderer out of the measurement, so what
# is left is exactly the per-finding bookkeeping; both commands still
# compute every finding and resolve every suppression directive. The
# rendered pair adds the renderer back and measures what a user with a
# genuinely broken file pays: one source snippet, line and column per
# label. Neither subsumes the other — a regression in either half moves
# only its own pair — and stdout goes to /dev/null in both, so the gate
# times the toolchain, not the terminal.
diag_lint_budget=$(awk -v b="$diag_lint_budget_base_ms" -v f="$factor" 'BEGIN { printf "%d", b * f }')
diag_check_budget=$(awk -v b="$diag_check_budget_base_ms" -v f="$factor" 'BEGIN { printf "%d", b * f }')
diag_lint_rendered_budget=$(awk -v b="$diag_lint_rendered_budget_base_ms" -v f="$factor" 'BEGIN { printf "%d", b * f }')
diag_check_rendered_budget=$(awk -v b="$diag_check_rendered_budget_base_ms" -v f="$factor" 'BEGIN { printf "%d", b * f }')

diag_dir="$corpus_dir/diagnostics-heavy"
mkdir -p "$diag_dir/lint/src" "$diag_dir/check/src"
mkdir -p "$diag_dir/lint-rendered/src" "$diag_dir/check-rendered/src"
for project in lint check lint-rendered check-rendered; do
  cat > "$diag_dir/$project/luabox.toml" <<'EOF'
[package]
name = "perf-gate-diagnostics"
version = "0.0.0"
edition = "5.4"

[build]
target = "5.4"
out = "dist"

[dependencies]
EOF
done

echo
echo "perf-gate: generating diagnostics-heavy corpus (${diag_corpus_findings} findings each) ..."

# `unused-local` (LB0501) x N, each silenced by its own `---@luabox-ignore`.
awk -v n="$diag_corpus_findings" 'BEGIN {
  print "local function main()"
  for (i = 0; i < n; i++) {
    print "    ---@luabox-ignore unused-local perf-gate corpus"
    printf "    local unused_%d = %d\n", i, i
  }
  print "end"
  print "return main"
}' > "$diag_dir/lint/src/main.lua"

# `undefined-field` (LB0306) x N, silenced file-wide by luals' own
# `---@diagnostic disable`, which is the checker's line-keyed path.
awk -v n="$diag_corpus_findings" 'BEGIN {
  print "---@diagnostic disable: undefined-field"
  print "---@class Point"
  print "---@field x number"
  print "local Point = { x = 1 }"
  print ""
  print "---@type Point"
  print "local p = Point"
  print ""
  for (i = 0; i < n; i++) {
    printf "local v%d = p.nope%d\n", i, i
    printf "print(v%d)\n", i
  }
  print "return p"
}' > "$diag_dir/check/src/main.lua"

# The same two files with the suppressions removed, so every finding is
# reported and rendered.
awk -v n="$diag_corpus_findings" 'BEGIN {
  print "local function main()"
  for (i = 0; i < n; i++) {
    printf "    local unused_%d = %d\n", i, i
  }
  print "end"
  print "return main"
}' > "$diag_dir/lint-rendered/src/main.lua"

awk -v n="$diag_corpus_findings" 'BEGIN {
  print "---@class Point"
  print "---@field x number"
  print "local Point = { x = 1 }"
  print ""
  print "---@type Point"
  print "local p = Point"
  print ""
  for (i = 0; i < n; i++) {
    printf "local v%d = p.nope%d\n", i, i
    printf "print(v%d)\n", i
  }
  print "return p"
}' > "$diag_dir/check-rendered/src/main.lua"

echo
echo "perf-gate: lint on a diagnostics-heavy file (warm)..."
( cd "$diag_dir/lint" && "$luabox_bin" lint >/dev/null 2>&1 ) || true
start=$(date +%s%N)
( cd "$diag_dir/lint" && "$luabox_bin" lint >/dev/null 2>&1 ) || true
end=$(date +%s%N)
diag_lint_ms=$(( (end - start) / 1000000 ))
if [[ "$diag_lint_ms" -lt "$diag_lint_budget" ]]; then
  echo "PASS lint (diagnostics-heavy): ${diag_lint_ms} ms < ${diag_lint_budget} ms"
else
  echo "FAIL lint (diagnostics-heavy): ${diag_lint_ms} ms >= ${diag_lint_budget} ms"
  fail=1
fi

echo
echo "perf-gate: check on a diagnostics-heavy file (warm)..."
( cd "$diag_dir/check" && "$luabox_bin" check >/dev/null 2>&1 ) || true
start=$(date +%s%N)
( cd "$diag_dir/check" && "$luabox_bin" check >/dev/null 2>&1 ) || true
end=$(date +%s%N)
diag_check_ms=$(( (end - start) / 1000000 ))
if [[ "$diag_check_ms" -lt "$diag_check_budget" ]]; then
  echo "PASS check (diagnostics-heavy): ${diag_check_ms} ms < ${diag_check_budget} ms"
else
  echo "FAIL check (diagnostics-heavy): ${diag_check_ms} ms >= ${diag_check_budget} ms"
  fail=1
fi

echo
echo "perf-gate: lint on a diagnostics-heavy file, findings rendered (warm)..."
( cd "$diag_dir/lint-rendered" && "$luabox_bin" lint >/dev/null 2>&1 ) || true
start=$(date +%s%N)
( cd "$diag_dir/lint-rendered" && "$luabox_bin" lint >/dev/null 2>&1 ) || true
end=$(date +%s%N)
diag_lint_rendered_ms=$(( (end - start) / 1000000 ))
if [[ "$diag_lint_rendered_ms" -lt "$diag_lint_rendered_budget" ]]; then
  echo "PASS lint (diagnostics-heavy, rendered): ${diag_lint_rendered_ms} ms < ${diag_lint_rendered_budget} ms"
else
  echo "FAIL lint (diagnostics-heavy, rendered): ${diag_lint_rendered_ms} ms >= ${diag_lint_rendered_budget} ms"
  fail=1
fi

echo
echo "perf-gate: check on a diagnostics-heavy file, findings rendered (warm)..."
( cd "$diag_dir/check-rendered" && "$luabox_bin" check >/dev/null 2>&1 ) || true
start=$(date +%s%N)
( cd "$diag_dir/check-rendered" && "$luabox_bin" check >/dev/null 2>&1 ) || true
end=$(date +%s%N)
diag_check_rendered_ms=$(( (end - start) / 1000000 ))
if [[ "$diag_check_rendered_ms" -lt "$diag_check_rendered_budget" ]]; then
  echo "PASS check (diagnostics-heavy, rendered): ${diag_check_rendered_ms} ms < ${diag_check_rendered_budget} ms"
else
  echo "FAIL check (diagnostics-heavy, rendered): ${diag_check_rendered_ms} ms >= ${diag_check_rendered_budget} ms"
  fail=1
fi

echo
if [[ "$fail" -eq 0 ]]; then
  echo "perf-gate: ALL GATES PASSED"
else
  echo "perf-gate: GATES FAILED"
fi
exit "$fail"
