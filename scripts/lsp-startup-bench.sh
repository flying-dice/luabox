#!/usr/bin/env bash
# Language-server startup benchmark: `initialize` -> the FIRST
# `publishDiagnostics` for the opened document, over a penlight-scale
# vendored rock tree.
#
# This exists because the number in `luabox_lsp`'s `harvest_rock_tree` comment
# and in the CHANGELOG was unreproducible: the corpus was described in prose
# and the driver lived in a scratch directory (Shockwave round 4). Everything
# needed to re-measure is now committed:
#
#   scripts/lsp-startup-bench.sh   this — builds, generates, runs, aggregates
#   scripts/lsp-startup-bench.py   the stdio LSP client that owns the clock
#   tools/gen-corpus --rock-tree   the corpus, into a real rock-tree layout
#
# It is NOT a CI gate. SPEC.md's LSP perf budget is future work, and the
# absolute number is host-dependent (Shockwave measured 2.81x on 4 vCPU where
# this box measures 3.84x). The harness is the point: the claim has to be
# re-runnable by whoever doubts it.
#
# Usage:
#   scripts/lsp-startup-bench.sh [--runs N] [--files N] [--lines-per-file N]
#                                [--threads N] [--keep]
#
#   --threads N   pin RAYON_NUM_THREADS for the run; `--threads 1` reproduces
#                 the sequential baseline from the same binary, which is what
#                 makes the parallel/sequential comparison a measurement of
#                 the parallelism rather than of a commit range.
#   --keep        leave the generated corpus in place and print its path.
set -euo pipefail

runs=7
files=50
lines_per_file=2000
threads=""
keep=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --runs) runs="$2"; shift 2 ;;
    --files) files="$2"; shift 2 ;;
    --lines-per-file) lines_per_file="$2"; shift 2 ;;
    --threads) threads="$2"; shift 2 ;;
    --keep) keep=1; shift ;;
    -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
    *) echo "lsp-startup-bench: unknown argument \`$1\` (see --help)" >&2; exit 2 ;;
  esac
done

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

echo "lsp-startup-bench: building release binaries..."
cargo build --release -p luabox-cli
cargo build --release --manifest-path tools/gen-corpus/Cargo.toml --target-dir target/gen-corpus

luabox_bin="$repo_root/target/release/luabox"
gen_corpus_bin="$repo_root/target/gen-corpus/release/gen-corpus"
if [[ -f "${luabox_bin}.exe" ]]; then
  luabox_bin="${luabox_bin}.exe"
  gen_corpus_bin="${gen_corpus_bin}.exe"
fi

corpus_dir="$(mktemp -d)"
if [[ "$keep" -eq 0 ]]; then
  trap 'rm -rf "$corpus_dir"' EXIT
fi

echo "lsp-startup-bench: generating the rock-tree corpus into ${corpus_dir} ..."
"$gen_corpus_bin" --out "$corpus_dir" --rock-tree 5.4 \
  --seed 42 --files "$files" --lines-per-file "$lines_per_file"

kloc=$(cat "$corpus_dir"/lua_modules/share/lua/5.4/*.lua | wc -l)
echo "lsp-startup-bench: ${files} rock modules, ${kloc} lines under lua_modules/share/lua/5.4/"

if [[ -n "$threads" ]]; then
  export RAYON_NUM_THREADS="$threads"
  echo "lsp-startup-bench: RAYON_NUM_THREADS=$threads"
fi

samples=()
for ((i = 1; i <= runs; i++)); do
  line="$(python3 "$repo_root/scripts/lsp-startup-bench.py" \
    --luabox "$luabox_bin" --project "$corpus_dir")"
  ms="$(printf '%s' "$line" | python3 -c 'import json,sys; print(json.load(sys.stdin)["ms"])')"
  printf '  run %d/%d: %.1f ms\n' "$i" "$runs" "$ms"
  samples+=("$ms")
done

printf '%s\n' "${samples[@]}" | python3 -c '
import sys, statistics
xs = sorted(float(line) for line in sys.stdin if line.strip())
print()
print("lsp-startup-bench: %d runs — median %.1f ms, min %.1f ms, max %.1f ms"
      % (len(xs), statistics.median(xs), xs[0], xs[-1]))
'

if [[ "$keep" -eq 1 ]]; then
  echo "lsp-startup-bench: corpus kept at $corpus_dir"
fi
