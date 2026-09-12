#!/bin/bash
# Self-test for prose-wrap-guard.sh (#87). Each case invokes the real guard
# against a generated Markdown tree, so its exit status and exclusions cannot
# silently drift away from the CI gate (decision 12).
set -u

here="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$here/../.." && pwd)"
guard="$here/prose-wrap-guard.sh"
ci_yml="$repo_root/.gitlab-ci.yml"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# shellcheck source=scripts/tests/selftest-lib.sh
. "$here/selftest-lib.sh"

make_root() {
    local dir="$1"
    rm -rf "$dir"
    mkdir -p "$dir/docs"
}

run_guard() {
    local dir="$1" log="$2"
    PROSE_WRAP_REPO_DIR="$dir" PROSE_WRAP_BASELINE_FILE=/dev/null \
        bash "$guard" >"$log" 2>&1
}

case_dir="$work/at-limit"
make_root "$case_dir"
printf '%079d\n' 0 >"$case_dir/CHANGELOG.md"
log="$work/s1.log"
run_guard "$case_dir" "$log"
assert_exit S1_79_columns_passes 0 "$?" "$log"

case_dir="$work/over-limit"
make_root "$case_dir"
printf '%080d\n' 0 >"$case_dir/CHANGELOG.md"
log="$work/s2.log"
run_guard "$case_dir" "$log"
assert_exit S2_80_columns_fails 1 "$?" "$log" \
    "CHANGELOG.md:1:80: prose exceeds 79 columns"

case_dir="$work/url"
make_root "$case_dir"
printf 'Short prose.\nhttps://example.invalid/%0100d\n' 0 >"$case_dir/CHANGELOG.md"
log="$work/s3.log"
run_guard "$case_dir" "$log"
assert_exit S3_url_only_line_passes 0 "$?" "$log"

case_dir="$work/fence"
make_root "$case_dir"
printf 'Short prose.\n```text\n%0100d\n```\n' 0 >"$case_dir/docs/code.md"
log="$work/s4.log"
run_guard "$case_dir" "$log"
assert_exit S4_fenced_code_passes 0 "$?" "$log"

case_dir="$work/table"
make_root "$case_dir"
printf 'Short prose.\n| %0100d |\n' 0 >"$case_dir/docs/table.md"
log="$work/s5.log"
run_guard "$case_dir" "$log"
assert_exit S5_table_row_passes 0 "$?" "$log"

case_dir="$work/missing"
make_root "$case_dir"
log="$work/s6.log"
run_guard "$case_dir" "$log"
assert_exit S6_missing_files_fail_closed 1 "$?" "$log" \
    "found no CHANGELOG.md or docs/**/*.md files"

case_dir="$work/no-prose"
make_root "$case_dir"
printf '```text\n%0100d\n```\n| %0100d |\nhttps://example.invalid/%0100d\n' \
    0 0 0 >"$case_dir/docs/excluded.md"
log="$work/s7.log"
run_guard "$case_dir" "$log"
assert_exit S7_zero_eligible_prose_fails_closed 2 "$?" "$log" \
    "found no eligible prose lines"

if [ ! -f "$ci_yml" ]; then
    echo "FAIL  S8_ci_job_is_merge_path_blocking: no CI config at $ci_yml" >&2
    fail=$((fail + 1))
else
    job_block="$(awk '
        /^prose-wrap:/ { grab=1 }
        grab && (/^$/ || (/^[^[:space:]#]/ && !/^prose-wrap:/)) { exit }
        grab { print }
    ' "$ci_yml")"
    ok=1
    report=""
    for needle in '  stage: selftest' '  extends: [.linux, .rules-ci]' \
        '    - bash scripts/tests/prose-wrap-guard-selftest.sh' \
        '    - bash scripts/tests/prose-wrap-guard.sh'; do
        echo "$job_block" | grep -qxF "$needle" || {
            ok=0
            report="$report missing:[$needle]"
        }
    done
    echo "$job_block" | grep -q 'allow_failure\|when: manual\|rules:' && {
        ok=0
        report="$report present:[escape hatch]"
    }
    if [ "$ok" = 1 ]; then
        echo "PASS  S8_ci_job_is_merge_path_blocking"
        pass=$((pass + 1))
    else
        echo "FAIL  S8_ci_job_is_merge_path_blocking:$report" >&2
        fail=$((fail + 1))
    fi
fi

# A legacy exemption is content-pinned: the recorded file passes, but adding
# one new overlong line changes the hash and activates the guard.
case_dir="$work/baseline"
make_root "$case_dir"
printf '%080d\n' 0 >"$case_dir/CHANGELOG.md"
baseline="$work/baseline.txt"
hash="$(sha256sum "$case_dir/CHANGELOG.md")"
printf '%s  CHANGELOG.md\n' "${hash%% *}" >"$baseline"
log="$work/s9a.log"
PROSE_WRAP_REPO_DIR="$case_dir" PROSE_WRAP_BASELINE_FILE="$baseline" \
    bash "$guard" >"$log" 2>&1
assert_exit S9a_exact_legacy_content_passes 0 "$?" "$log"
printf '%080d\n' 1 >>"$case_dir/CHANGELOG.md"
log="$work/s9b.log"
PROSE_WRAP_REPO_DIR="$case_dir" PROSE_WRAP_BASELINE_FILE="$baseline" \
    bash "$guard" >"$log" 2>&1
assert_exit S9b_changed_legacy_file_fails 1 "$?" "$log" \
    "stale prose-wrap baseline entry: CHANGELOG.md"

# Fence state is per file. An unclosed fence in the first document must not
# suppress an overlong prose line in the next one.
case_dir="$work/multiple-files"
make_root "$case_dir"
printf 'Short prose.\n```text\n%0100d\n' 0 >"$case_dir/docs/a.md"
printf '%080d\n' 0 >"$case_dir/docs/b.md"
log="$work/s10.log"
run_guard "$case_dir" "$log"
assert_exit S10_fence_state_resets_per_file 1 "$?" "$log" \
    "docs/b.md:1:80: prose exceeds 79 columns"

# Baseline configuration fails closed: malformed, duplicate, and unknown
# rows cannot turn into invisible exceptions.
case_dir="$work/baseline-errors"
make_root "$case_dir"
printf 'Short prose.\n' >"$case_dir/CHANGELOG.md"

baseline="$work/malformed-baseline.txt"
printf 'not-a-hash CHANGELOG.md\n' >"$baseline"
log="$work/s11.log"
PROSE_WRAP_REPO_DIR="$case_dir" PROSE_WRAP_BASELINE_FILE="$baseline" \
    bash "$guard" >"$log" 2>&1
assert_exit S11_malformed_baseline_fails 1 "$?" "$log" \
    "malformed prose-wrap baseline entry"

hash="$(sha256sum "$case_dir/CHANGELOG.md")"
hash="${hash%% *}"
baseline="$work/duplicate-baseline.txt"
printf '%s  CHANGELOG.md\n%s  CHANGELOG.md\n' "$hash" "$hash" >"$baseline"
log="$work/s12.log"
PROSE_WRAP_REPO_DIR="$case_dir" PROSE_WRAP_BASELINE_FILE="$baseline" \
    bash "$guard" >"$log" 2>&1
assert_exit S12_duplicate_baseline_fails 1 "$?" "$log" \
    "duplicate prose-wrap baseline entry: CHANGELOG.md"

baseline="$work/unknown-baseline.txt"
printf '%s  docs/missing.md\n' "$hash" >"$baseline"
log="$work/s13.log"
PROSE_WRAP_REPO_DIR="$case_dir" PROSE_WRAP_BASELINE_FILE="$baseline" \
    bash "$guard" >"$log" 2>&1
assert_exit S13_unknown_baseline_path_fails 1 "$?" "$log" \
    "unknown prose-wrap baseline path: docs/missing.md"

selftest_report prose-wrap-guard-selftest
