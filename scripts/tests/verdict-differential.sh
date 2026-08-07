#!/bin/bash
# Verdict regression oracle: for every case in
# scripts/tests/verdict-differential/, the CURRENT `luabox check --format
# json` diagnostic-code set is re-derived and compared to the committed
# expectation in expected.tsv.
#
# Why this exists. Five review rounds of PR #61 each found the type checker
# silently changing a shipped verdict against develop — a read that used to
# be clean started erroring, or an error went silent — and every time it was
# found by a reviewer building binaries at two git refs and diffing the
# output BY HAND (board entry 2026-08-06T16:00, "the luals differential
# measured different file sets" and its siblings; 2026-08-07T02:00's
# generic-diamond field-erasure regression, found only because a reviewer
# happened to build both heads). That capability was not in the repo, so
# every change to the checker was unverifiable against "what did this do
# before" until a reviewer went looking. This script is that capability,
# following the shape scripts/tests/luals-differential.sh and
# scripts/tests/control-flow-differential.sh already established rather than
# inventing a third one:
#
#   codes column   `luabox check --format json` (strict) over a temp project
#                  containing the case plus every module its .deps sidecar
#                  names, exactly as luals-differential.sh's luabox column
#                  builds it — `diag` (+ the sorted, deduped set of LB codes)
#                  if the project has any diagnostic, else `clean`.
#
# --- Design decision: committed expectations, not a live rebuilt baseline --
#
# luals-differential.sh and control-flow-differential.sh both diff luabox
# against an INDEPENDENT, live oracle (lua-language-server, `luac -p`) on
# every run — that live comparison IS the point, because the two tools are
# expected to agree and any run can catch new drift from either side. This
# gate's oracle is not independent: it is an OLDER BUILD OF THE SAME TOOL.
# Rebuilding that baseline on every CI run would mean two release builds per
# invocation (the current tree plus a `git worktree` checkout of develop),
# needs network/git-history access this job would not otherwise need, and
# ties a merge-blocking gate's runtime to develop's own build health — a
# broken develop build would fail every PR's verdict gate for a reason that
# has nothing to do with the PR. Committed expectations avoid all three and
# add the property the WHY above actually asks for: a verdict change becomes
# a diff against a CLAIM SOMEONE WROTE DOWN in expected.tsv, reviewable in
# the PR that changes it, rather than something a reviewer discovers by
# re-deriving it. This is the same tradeoff luals-differential.sh already
# made (expected.tsv there is not re-derived from a rebuilt luals corpus
# either) and control-flow-differential.sh's exceptions.tsv makes for its one
# documented divergence — this file extends the same discipline to the
# self-regression axis. See "Regenerating expected.tsv" below for how the
# committed values get produced from a real baseline build, deliberately, not
# automatically.
#
# --- Design decision: the code SET, not file/line -----------------------
#
# luals-differential.sh's header (F19) already settled this argument for the
# same reason it applies here: "not clean" alone can't tell an intended
# LB0306 apart from an accidental LB0300 or a regressed LB0305, so a bare
# clean/diag check would have passed every one of the five PR #61 rounds'
# silent-regression incidents just as it passed before them. The code SET
# (sorted, deduped, exact match) is loose enough that renaming a message,
# reordering diagnostics, or adding an unrelated column to the JSON output
# does not fail a row, and tight enough that a class-merge precedence flip —
# first-listed-wins becoming last-listed-wins, an LB0300 appearing on a
# different line for the same reason — changes the set and fails the row.
# File/line was considered and rejected: it is what luals-differential.sh
# explicitly does NOT assert (that gate compares against a SECOND tool whose
# line numbers do not correspond to luabox's own), and here it would make
# every unrelated formatting or column-numbering tweak a failure, which is
# exactly the "too tight" failure mode the task brief warns about. A shape
# whose PIN specifically depends on which line fires (first-wins vs
# last-wins) is instead pinned by writing the case so that only ONE of two
# candidate calls can be clean under the correct resolution — see
# indexer_diamond_last_listed_wins.lua for the pattern.
#
# --- Regenerating expected.tsv -------------------------------------------
#
# expected.tsv is not re-derived automatically, ever — regenerating it is a
# deliberate, reviewed act, the same discipline control-flow-differential.sh
# asks of exceptions.tsv. To produce fresh values from a real baseline:
#
#   git -C ~/Projects/luabox worktree add /tmp/luabox-baseline origin/develop
#   (cd /tmp/luabox-baseline && cargo build --release --bin luabox)
#   LUABOX=/tmp/luabox-baseline/target/release/luabox \
#       VERDICT_PRINT=1 bash scripts/tests/verdict-differential.sh \
#       > /tmp/verdict-regenerated.tsv
#
# VERDICT_PRINT=1 short-circuits the comparison entirely (see below): it
# walks every *.lua case in the corpus, measures it against whatever LUABOX
# points at, and prints a ready expected.tsv body — verdict and codes
# freshly measured, the note column copied verbatim from the CURRENT
# expected.tsv (a re-measurement is not a re-justification: the prose
# explaining why a case is in the corpus does not change because its code
# moved) or `TODO: <case>` for a case with no existing row, which is the
# loud reminder that a brand new row still needs a human sentence before it
# can be pasted in — see the note-is-mandatory rule below. Diff the output
# against the current expected.tsv like any other reviewed change; nothing
# writes the file directly and nothing runs this mode in CI.
#
# --- Corpus provenance -----------------------------------------------------
#
# Seeded from the shapes named in docs/03-reference/02-limitations.md's
# "Duplicate `---@class` declarations union" section (first-listed-wins vs
# last-listed-wins indexer inheritance, positional generic-parameter
# unification, the scoped-unknown-name and surplus-parameter edges) plus the
# `__index`-less carrier fall-through — exactly the class-merge precedence
# machinery PR #61's five review rounds kept finding drift in. Every row's
# expected verdict was MEASURED against the shipped binary, not derived from
# reading the prose (the same discipline the doc itself states: "Both
# inherited rules are measured against the pre-change binary rather than
# derived, because an intermediate build of this release collapsed them into
# a single last-wins rule and silently changed verdicts in both
# directions").
#
# --- No silent caps ---------------------------------------------------------
#
# Same discipline as luals-differential.sh: no LUABOX binary, no corpus
# directory, no expected.tsv, an expected.tsv with zero case rows, a corpus
# *.lua file with no row claiming it, and a row naming a *.lua file that does
# not exist are all hard failures, not silent skips. A row's note column
# must be non-empty — there is no "divergence" axis here to gate the
# requirement on (unlike luals-differential.sh, which only requires a note
# when the two columns disagree), so EVERY row must say what regression
# shape it pins; an empty note is exactly the "cannot fail" gate shape the
# task brief warns a five-round reviewer will be looking for.
set -u

here="$(cd "$(dirname "$0")" && pwd)"
# VERDICT_CORPUS overrides the corpus directory — unused in normal operation
# (every real invocation wants the committed corpus), the same convenience
# LUALS_CORPUS gives luals-differential.sh's self-test.
corpus="${VERDICT_CORPUS:-$here/verdict-differential}"
expected="$corpus/expected.tsv"
luabox="${LUABOX:-$here/../../target/release/luabox}"
print_mode="${VERDICT_PRINT:-0}"

if [ ! -x "$luabox" ]; then
    echo "error: no luabox binary at $luabox (build with: cargo build --release --bin luabox, or set LUABOX)" >&2
    exit 1
fi
# Resolve a relative LUABOX against the caller's cwd, validated just above —
# every case below runs from inside a temp project directory, so a relative
# path would not exist there and every row would fail on a binary the guard
# just confirmed (the same fix luals-differential.sh carries for the same
# reason).
case "$luabox" in
/*) ;;
*) luabox="$(cd "$(dirname "$luabox")" && pwd)/$(basename "$luabox")" ;;
esac

if [ ! -d "$corpus" ]; then
    echo "error: no corpus directory at $corpus" >&2
    exit 1
fi
if [ ! -f "$expected" ]; then
    echo "error: no expectations at $expected" >&2
    exit 1
fi
# python3 parses `luabox check --format json` below; declare the dependency
# loudly and up front rather than let a missing interpreter surface many
# lines later as a wall of "expected.tsv says X" failures with no indication
# why every measured verdict came back wrong (the same guard
# luals-differential.sh carries, F24).
if ! command -v python3 >/dev/null 2>&1; then
    echo "error: python3 not found on PATH — required to parse 'luabox check --format json' output" >&2
    exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/src"
printf '[package]\nname = "vdiff"\nversion = "0.1.0"\nedition = "5.4"\n\n[types]\nstrict = true\n' \
    > "$work/luabox.toml"

# copy_case_deps <case> — populates $work/src for <case>: the case's own
# file as main.lua, plus every module its .deps sidecar names, copied under
# their own names so `require("name")` resolves. Every dep name is checked
# against a path-traversal shape before it is used as a cp destination —
# read verbatim from a PR-authored .deps sidecar, the same guard
# luals-differential.sh carries (F27). `rm -f` first so a previous case's
# dependency files can never leak into this one's verdict (the same bug
# luals-differential.sh's self-test pins as stale_dep_file_cleared_between_cases).
copy_case_deps() {
    local case_name="$1" src="$corpus/$1.lua" dep
    rm -f "$work/src"/*.lua
    if [ ! -f "$src" ]; then
        echo "FAIL  $case_name: expected.tsv names it, but $src does not exist" >&2
        return 1
    fi
    cp "$src" "$work/src/main.lua"
    if [ -f "$corpus/$case_name.deps" ]; then
        while IFS= read -r dep || [ -n "$dep" ]; do
            [ -n "$dep" ] || continue
            case "$dep" in
            */*|.|..)
                echo "FAIL  $case_name: unsafe dependency name '$dep' in $case_name.deps — must be a plain filename in this directory, no path separators" >&2
                return 1
                ;;
            esac
            if ! cp "$corpus/$dep" "$work/src/$dep" 2>/dev/null; then
                echo "FAIL  $case_name: could not copy dependency '$dep' named in $case_name.deps" >&2
                return 1
            fi
        done <"$corpus/$case_name.deps"
    fi
    return 0
}

# measure_case <case> — runs the CURRENT $luabox over the file set
# copy_case_deps just populated and sets MEASURED_VERDICT (clean|diag) and
# MEASURED_CODES (sorted, comma-joined LB codes, or empty for clean).
# `--format json` keeps stdout pure JSON, so the exact diagnostic code SET
# can be asserted, not just "the summary line wasn't all-zero" — see the
# header for why that distinction is the whole point of this file.
measure_case() {
    local case_name="$1" out codes_rc
    out="$( (cd "$work" && "$luabox" check --format json) 2>"$work/luabox.err" )"
    MEASURED_CODES="$(printf '%s' "$out" | python3 -c '
import json, sys
data = json.load(sys.stdin)
print(",".join(sorted({d["code"] for d in data})))
' 2>"$work/codes.err")"
    codes_rc=$?
    if [ "$codes_rc" != 0 ]; then
        echo "FAIL  $case_name: could not parse 'luabox check --format json' output as JSON:" >&2
        sed 's/^/      /' "$work/codes.err" >&2
        echo "      stderr:" >&2
        sed 's/^/      /' "$work/luabox.err" >&2
        return 1
    fi
    if [ -z "$MEASURED_CODES" ]; then
        MEASURED_VERDICT="clean"
    else
        MEASURED_VERDICT="diag"
    fi
    return 0
}

# --- VERDICT_PRINT: regenerate mode, see "Regenerating expected.tsv" above.
# Never compares, never fails a row — it only re-measures and prints. Not run
# in CI; a human invokes it against a deliberately chosen LUABOX and reviews
# the diff by hand.
if [ "$print_mode" = 1 ]; then
    for f in "$corpus"/*.lua; do
        base="$(basename "$f" .lua)"
        # A file only ever named by another case's .deps sidecar is a
        # support module, not a case of its own — skip it the same way the
        # unclaimed-corpus sweep below does.
        if grep -qxF "$base.lua" "$corpus"/*.deps 2>/dev/null; then
            continue
        fi
        if ! copy_case_deps "$base"; then
            echo "error: regeneration aborted on $base" >&2
            exit 1
        fi
        if ! measure_case "$base"; then
            echo "error: regeneration aborted on $base" >&2
            exit 1
        fi
        note="$(awk -F'\t' -v c="$base" '$1==c{print $4; found=1} END{if (!found) print "TODO: " c}' "$expected")"
        codes_field="${MEASURED_CODES:--}"
        printf '%s\t%s\t%s\t%s\n' "$base" "$MEASURED_VERDICT" "$codes_field" "$note"
    done
    exit 0
fi

fails=0
rows=0
declare -A seen=()

printf '%-45s %-8s %-30s %s\n' "CASE" "VERDICT" "CODES" "NOTE"
printf '%-45s %-8s %-30s %s\n' "----" "-------" "-----" "----"

while IFS=$'\t' read -r case_name want_verdict want_codes note; do
    case "${case_name:-}" in ''|'#'*) continue ;; esac
    # Tab is an "IFS whitespace" character to bash's `read`, so a run of
    # adjacent tabs collapses instead of yielding an empty field the way a
    # comma or other non-whitespace IFS delimiter would — expected.tsv
    # spells a clean row's (empty) codes column as the literal `-` for
    # exactly this reason (the same convention luals-differential.sh uses);
    # normalise it back to empty here, once.
    [ "$want_codes" = "-" ] && want_codes=""
    rows=$((rows + 1))
    seen["$case_name"]=1

    # Every row must document, in writing, what it pins — there is no
    # "divergence" axis here to gate this on (unlike luals-differential.sh,
    # which only requires a note where its two columns disagree), so an
    # empty note is unconditionally refused (see header).
    if [ -z "${note:-}" ]; then
        echo "FAIL  $case_name: note column is empty — every row must say what regression shape it pins, in writing" >&2
        fails=$((fails + 1))
    fi

    if ! copy_case_deps "$case_name"; then
        fails=$((fails + 1))
        continue
    fi
    if ! measure_case "$case_name"; then
        fails=$((fails + 1))
        continue
    fi

    if [ "$MEASURED_VERDICT" != "$want_verdict" ]; then
        echo "FAIL  $case_name: luabox is '$MEASURED_VERDICT', expected.tsv says '$want_verdict' (codes: ${MEASURED_CODES:-none})" >&2
        fails=$((fails + 1))
    elif [ "$MEASURED_CODES" != "$want_codes" ]; then
        echo "FAIL  $case_name: luabox codes are '$MEASURED_CODES', expected.tsv says '$want_codes'" >&2
        fails=$((fails + 1))
    fi

    printf '%-45s %-8s %-30s %s\n' "$case_name" "$MEASURED_VERDICT" "${MEASURED_CODES:--}" "$note"
done < "$expected"

# Every corpus file must be claimed by a row — an unclaimed case is coverage
# that silently isn't (the no-silent-caps rule). Files named by a `.deps`
# sidecar are support modules for a cross-file case, not cases.
support="$(cat "$corpus"/*.deps 2>/dev/null || true)"
for src in "$corpus"/*.lua; do
    base="$(basename "$src")"
    name="$(basename "$src" .lua)"
    if printf '%s\n' "$support" | grep -qxF "$base"; then
        continue
    fi
    if [ -z "${seen[$name]:-}" ]; then
        echo "FAIL  $name.lua exists in the corpus but expected.tsv has no row for it" >&2
        fails=$((fails + 1))
    fi
done

# A corpus with no rows (an empty or comments-only expected.tsv, or an empty
# corpus directory) would otherwise sail through as "0 failures across 0
# rows" — the exact "cannot fail because it measures nothing" shape the task
# brief calls out. Refuse it outright rather than let it read as green.
if [ "$rows" -eq 0 ]; then
    echo "verdict-differential: NO ROWS MEASURED — expected.tsv has no case rows (only comments/header?) or the corpus is empty; this gate must never pass by measuring nothing" >&2
    exit 1
fi

echo
if [ "$fails" -gt 0 ]; then
    echo "verdict-differential: $fails failure(s) across $rows row(s)"
    exit 1
fi
echo "verdict-differential: $rows row(s) match expected.tsv"
