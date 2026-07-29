#!/usr/bin/env bash
# SPEC.md §16.1 perf gates (CI-blocking): cold start < 50 ms; `check` on a
# 100-kLOC corpus < 1 s warm (LSP keystroke-to-diagnostics gate is future
# work, not covered here).
#
# Gates: cold start, `fmt --check` throughput (kept as a wider safety
# net), the real `check` gate (live since GL#6), and a diagnostics-heavy
# `lint` + `check` gate.
#
# Why that fourth gate exists: the ~100-kLOC corpus the first three legs
# use is *clean* (`check: 0 errors, 0 warnings`), so none of them ever
# exercises per-diagnostic work. That blind spot hid an
# O(diagnostics x file size) line lookup: 32 k findings in one file took
# ~32 s to lint, while the same file with a single finding took 0.35 s.
# The fourth leg feeds one big file full of findings to both commands.
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
diag_corpus_findings=20000

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

# --- DIAGNOSTICS-HEAVY GATE ------------------------------------------------
# The corpus above is clean, so nothing so far times per-diagnostic work.
# These two legs do: one file, `diag_corpus_findings` findings, every one
# of them *suppressed*. Suppressing them is deliberate — it keeps the
# renderer (which formats one snippet per reported diagnostic, and is
# linear in the file for each) out of the measurement, so what is left is
# exactly the per-finding bookkeeping this gate is about. Both commands
# still compute every finding and resolve every suppression directive.
diag_lint_budget=$(awk -v b="$diag_lint_budget_base_ms" -v f="$factor" 'BEGIN { printf "%d", b * f }')
diag_check_budget=$(awk -v b="$diag_check_budget_base_ms" -v f="$factor" 'BEGIN { printf "%d", b * f }')

diag_dir="$corpus_dir/diagnostics-heavy"
mkdir -p "$diag_dir/lint/src" "$diag_dir/check/src"
for project in lint check; do
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
if [[ "$fail" -eq 0 ]]; then
  echo "perf-gate: ALL GATES PASSED"
else
  echo "perf-gate: GATES FAILED"
fi
exit "$fail"
