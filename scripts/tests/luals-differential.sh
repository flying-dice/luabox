#!/bin/bash
# luals parity differential: for every case in scripts/tests/luals-differential/,
# the `luabox check` verdict and the lua-language-server verdict are both
# re-derived and compared to the committed expectations in expected.tsv.
#
# Why this exists (#57). Twice during the PR #55 burn-down, luals parity had
# to be asserted by hand or guessed: the string-receiver (#48) and
# nil-admitting-arity (#49) gaps were found by a reviewer running luals
# manually, and the non-trailing nil-param bound was taken conservatively
# because no luals binary existed in the dev environment. "We believe this
# matches luals" was a prose claim. This makes it a measured gate:
#
#   luabox column   `luabox check --format json` (strict) over a temp project
#                   containing the case plus every module its .deps sidecar
#                   names — `diag` (+ the sorted, deduped set of LB codes
#                   reported) if the project has any diagnostic, else `clean`.
#   luals column    `lua-language-server --check` over this directory,
#                   filtered to the type-parity diagnostic code set below,
#                   unioned over the SAME file set the luabox column just
#                   typechecked (the case plus its .deps modules) — `diag` if
#                   any such code fires anywhere in that set, else `clean`.
#
# The two columns typecheck the SAME files for the SAME reason: luabox runs
# each case as its own one-project temp build (main.lua + copied deps) and
# reads the whole build's diagnostics; luals checks this whole directory as
# one workspace and every file's diagnostics are real, but a case's OWN
# verdict must be judged against its OWN file set, not just its own
# basename -- a diagnostic in a case's dependency (e.g. a generic support
# module luals cannot parse) is exactly as much this case's problem as a
# diagnostic in the case file itself, because that is what the luabox column
# already measures. Collecting only the case's own basename here silently
# dropped every such diagnostic; see the git history of this file for the
# reproduction (a live divergence on generic_box_mod.lua's `undefined-doc-name`
# for `T`, attributed to nothing because "generic_box_mod" is never a row).
#
# The two columns are NOT asserted equal, and must not be: a row where they
# differ is a reviewed, intentional divergence whose justification lives in
# expected.tsv (e.g. luals 3.13.5 has no generic-class support). What the
# script enforces is that both columns match what is written down — so a
# parity drift in EITHER tool shows up as a diff against a claim. This is
# the lb0510-matrix discipline applied to luals parity. A row whose two
# columns disagree with no note is refused (below) rather than silently
# trusted — the "justified row, not a hidden allowlist" rule is enforced,
# not just written in the header.
#
# The luabox column also carries the exact set of diagnostic codes it
# expects, not just clean/diag: "not clean" alone can't tell an intended
# LB0306 apart from an accidental LB0300 or a regressed LB0305, and two of
# the corpus's own claims (the computed-key and __index narrowings) have no
# other pin anywhere in the test suite. Comparing the code SET (not just
# presence) is also what lets duplicate_field_conflict_first_wins tell
# first-wins from last-wins apart: LB0311 alone fires either way, but the
# LB0300 on top of it only fires when the FIRST declaration's type is what's
# actually kept.
#
# Three things are checked before any row is measured, because a driver that
# cannot tell "measured and clean" from "did not measure" is the defect this
# whole directory exists to refuse: luals's VERSION must be the one the
# parity claims name (LUALS_EXPECTED_VERSION), every per-case `luabox check`
# runs under `timeout` (LUALS_CASE_TIMEOUT), and a nonzero luabox exit with
# an empty diagnostics array fails the measurement instead of reading as
# `clean`. The sibling driver (verdict-differential.sh) grew the last two as
# M31/M32 and the version pin lived only in the workflow; all three are here
# now (#58 review round 8, F12).
#
# No lua-language-server on PATH (and no LUALS env) means the luals column
# SKIPs, loudly, and the luabox column still runs — UNLESS LUALS_REQUIRED=1
# is set, in which case a missing luals binary is a hard failure rather than
# a silent smoke test. CI's luals-parity job always installs the pinned
# version and should set LUALS_REQUIRED=1 so a tarball layout change or a
# stripped LUALS= prefix cannot downgrade a merge-blocking parity gate into
# a green luabox-only run without anyone noticing (this script cannot force
# that from inside CI's job definition — see the workflow for the actual
# switch).
set -u

here="$(cd "$(dirname "$0")" && pwd)"
# LUALS_CORPUS overrides the corpus directory — unused in normal operation
# (every real invocation wants the committed corpus), but it is what lets
# luals-differential-selftest.sh (#57's answer to F25) point this script at
# a small, disposable fixture corpus instead of the real 24-row one, the
# same way the sibling gates' env seams let their self-tests
# substitute fixtures without hand-rolling a second copy of the gate.
corpus="${LUALS_CORPUS:-$here/luals-differential}"
expected="$corpus/expected.tsv"
luabox="${LUABOX:-$here/../../target/release/luabox}"
luals="${LUALS:-lua-language-server}"
luals_required="${LUALS_REQUIRED:-0}"
# LUALS_EXPECTED_VERSION — the version every "measured against 3.13.5" claim
# in this file, in expected.tsv's divergence notes and in
# docs/02-limitations.md is a claim ABOUT (#58 review round 8, F12). Until
# now this script checked only that SOME lua-language-server was on PATH:
# the pin lived exclusively in .github/workflows/luals-parity.yml's
# LUALS_VERSION/LUALS_SHA256 download step, so the workflow measured 3.13.5
# and every local re-run — the artifact a reviewer actually reaches for when
# checking one of those notes — measured whatever the developer's package
# manager installed, under the same banner. Overridable, because bumping the
# pin is a deliberate change that re-measures every row (see this file's
# header and the intentional-divergence rows); keep it in step with the
# workflow's LUALS_VERSION.
luals_expected_version="${LUALS_EXPECTED_VERSION:-3.13.5}"
# LUALS_CASE_TIMEOUT — per-case wall-clock budget for the `luabox check`
# below, seconds. verdict-differential.sh's sibling driver has carried this
# since M32; this one did not, so a hang here wedged the merge-blocking job
# until the workflow's own timeout-minutes killed it with no per-case
# attribution at all (#58 review round 8, F12). Same name shape and same
# default as VERDICT_CASE_TIMEOUT.
case_timeout="${LUALS_CASE_TIMEOUT:-30}"

# The diagnostic codes that constitute the type-parity surface. Everything
# else luals reports (style, unused locals, …) is out of scope for the
# comparison — .luarc.json in the corpus disables the noisiest, and this set
# is the positive filter.
# circle-doc-class is in the set as of round 8 (F9): the corpus carries two
# cyclic-class rows, and without the code here a luals cycle emission is
# filtered out and reads as "clean" — the exact silent gap F9 named.
parity_codes="param-type-mismatch missing-parameter undefined-field undefined-doc-name duplicate-doc-field assign-type-mismatch return-type-mismatch missing-return missing-fields cast-local-type undefined-global circle-doc-class"

if [ ! -x "$luabox" ]; then
    echo "error: no luabox binary at $luabox (build with: cargo build --release --bin luabox, or set LUABOX)" >&2
    exit 1
fi
# The luabox column runs each case from a temp project directory, so a
# *relative* LUABOX — validated just above against the caller's cwd — would not
# exist at the call site, and every row would fail on a binary the guard just
# confirmed. Resolve it once here, against the cwd the guard used, so the
# documented recovery path ("set LUABOX") cannot fabricate parity failures.
case "$luabox" in
/*) ;;
*) luabox="$(cd "$(dirname "$luabox")" && pwd)/$(basename "$luabox")" ;;
esac
if [ ! -f "$expected" ]; then
    echo "error: no expectations at $expected" >&2
    exit 1
fi

luals_column=1
if ! command -v "$luals" >/dev/null 2>&1; then
    if [ "$luals_required" = 1 ]; then
        echo "error: LUALS_REQUIRED=1 and no $luals on PATH — refusing to downgrade" >&2
        echo "error: this merge-blocking gate to a luabox-only smoke test" >&2
        exit 1
    fi
    luals_column=0
    echo "SKIP  luals column: no $luals on PATH — the parity half of every"
    echo "SKIP  claim below is UNVERIFIED in this run. Install"
    echo "SKIP  lua-language-server (or set LUALS) to check it; CI's"
    echo "SKIP  luals-parity job always does."
    echo
fi
# python3 parses luals's --check JSON below; declare the dependency loudly
# and up front rather than let a missing interpreter surface 46 lines later
# as a wall of "expected.tsv says X" failures with no indication why every
# luals verdict came back wrong (F24 — ci.yml has an explicit "assert python3
# is available" step for exactly this reason; this workflow has none, so the
# check lives here instead).
if [ "$luals_column" = 1 ] && ! command -v python3 >/dev/null 2>&1; then
    echo "error: python3 not found on PATH — required to parse $luals's --check output" >&2
    exit 1
fi
# M32's `timeout` guard, applied to this driver too (#58 review round 8,
# F12): declared up front, like python3 above, rather than surfacing as
# "timeout: command not found" once per row.
if ! command -v timeout >/dev/null 2>&1; then
    echo "error: timeout not found on PATH — required to bound each case's luabox invocation" >&2
    exit 1
fi
# The version this run's luals column is actually measured against must be
# the version this repo's parity claims name. PATH presence alone let a
# local re-run answer a "measured against 3.13.5" note with a different
# tool and report "both columns match" (F12). Checked, not assumed — and a
# mismatch is fatal rather than advisory, for the same reason LUALS_REQUIRED
# exists: a parity gate that quietly measures something else is worth less
# than one that refuses.
if [ "$luals_column" = 1 ]; then
    luals_version_out="$("$luals" --version 2>&1 | tr -d '\r')"
    case "$luals_version_out" in
    *"$luals_expected_version"*) ;;
    *)
        echo "error: $luals reports version '${luals_version_out}', which does not contain the" >&2
        echo "error:   expected '${luals_expected_version}'. Every parity note in expected.tsv and every" >&2
        echo "error:   'measured against ${luals_expected_version}' claim in the docs is a claim about THAT" >&2
        echo "error:   version; a run against another one is not evidence for or against them. Install the" >&2
        echo "error:   pinned build (see .github/workflows/luals-parity.yml's LUALS_VERSION), or set" >&2
        echo "error:   LUALS_EXPECTED_VERSION deliberately if you are re-measuring the corpus against a new one." >&2
        exit 1
        ;;
    esac
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/src"
printf '[package]\nname = "lualsdiff"\nversion = "0.1.0"\nedition = "5.4"\n\n[types]\nstrict = true\n' \
    > "$work/luabox.toml"

# One luals run over the whole corpus; per-case verdicts parsed out of the
# JSON afterwards. (--check writes to <out>/check.json. MEASURED against
# lua-language-server 3.13.5, 2026-08-09: a run with zero problems DOES write
# the file — it writes the two bytes `[]`, a JSON ARRAY, where a run with
# problems writes an OBJECT keyed by file URI. An earlier revision of this
# comment claimed the file was not written at all on a clean run, and the
# parser below trusted that by calling .items() unconditionally, so the
# all-clean corpus shape died with an AttributeError instead of reading as
# "nothing flagged". Both shapes are handled below.) Both streams are
# captured to a file, not /dev/null: a bad flag, an unwritable
# --check_out_path, a workspace load error and an OOM all currently produce
# the same one-line "failed to run" message with nothing to tell them apart
# (F26) — dumping the tool's own output on failure fixes that without
# needing a CI artifact upload.
luals_json=""
if [ "$luals_column" = 1 ]; then
    luals_out="$work/luals-out"
    mkdir -p "$luals_out"
    luals_log="$work/luals.log"
    luals_rc=0
    "$luals" --check "$corpus" --checklevel=Warning \
        --check_out_path="$luals_out/check.json" >"$luals_log" 2>&1 || luals_rc=$?
    if [ "$luals_rc" != 0 ]; then
        echo "error: $luals --check failed to run (exit $luals_rc) — output:" >&2
        sed 's/^/      /' "$luals_log" >&2
        exit 1
    fi
    luals_json="$luals_out/check.json"
fi

# The set of cases luals flagged with a parity-set code, plus a per-case
# detail line naming which file(s)/code(s) tripped it, for failure messages.
# A case's file set is itself plus every module its .deps sidecar names —
# the SAME set the luabox column below typechecks as one project (F17: a
# by-basename-only lookup here silently drops every diagnostic that lands in
# a support module instead of the case file itself).
luals_flagged=""
if [ "$luals_column" = 1 ] && [ -f "$luals_json" ]; then
    py_err="$work/luals-parse.err"
    if ! python3 - "$luals_json" "$corpus" "$work/luals-flagged.txt" "$work/luals-detail.tsv" $parity_codes \
        <<'PYEOF' 2>"$py_err"
import glob
import json
import os
import sys

check_path, corpus_dir, flagged_out, detail_out = sys.argv[1:5]
codes = set(sys.argv[5:])

with open(check_path) as f:
    data = json.load(f)

# lua-language-server 3.13.5 writes an OBJECT (file-URI -> diagnostics) when
# it found problems and the empty ARRAY `[]` when it found none — measured,
# see the shell comment above. An empty mapping is the same fact, so both
# normalise to "no file has any hit"; anything else is a shape this parser
# does not understand and must not silently read as clean.
if isinstance(data, list):
    if data:
        sys.exit(
            "luals --check wrote a non-empty JSON array, which this parser "
            "does not understand (3.13.5 writes `[]` for a clean run and an "
            "object keyed by file URI otherwise)"
        )
    data = {}
elif not isinstance(data, dict):
    sys.exit(f"luals --check wrote unexpected JSON of type {type(data).__name__}")

# Parity-set codes actually reported, per underlying file (basename, no
# .lua). This is per-FILE — the union into per-CASE file sets happens below.
file_hits = {}
for uri, diags in data.items():
    name = uri.rsplit("/", 1)[-1]
    if name.endswith(".lua"):
        name = name[: -len(".lua")]
    hits = sorted({d.get("code") for d in diags if d.get("code") in codes})
    if hits:
        file_hits[name] = hits

flagged = set()
detail = []
for lua_path in sorted(glob.glob(os.path.join(corpus_dir, "*.lua"))):
    case = os.path.basename(lua_path)[: -len(".lua")]
    fileset = [case]
    deps_path = os.path.join(corpus_dir, case + ".deps")
    if os.path.isfile(deps_path):
        with open(deps_path) as f:
            for line in f:
                dep = line.strip()
                if not dep:
                    continue
                fileset.append(dep[: -len(".lua")] if dep.endswith(".lua") else dep)
    parts = [f"{f_}:{code}" for f_ in fileset for code in file_hits.get(f_, ())]
    if parts:
        flagged.add(case)
        detail.append(case + "\t" + ",".join(parts))

with open(flagged_out, "w") as f:
    f.write("\n".join(sorted(flagged)))
with open(detail_out, "w") as f:
    f.write("\n".join(detail))
PYEOF
    then
        echo "error: parsing $luals_json failed:" >&2
        sed 's/^/      /' "$py_err" >&2
        exit 1
    fi
    luals_flagged="$(cat "$work/luals-flagged.txt" 2>/dev/null || true)"
fi

fails=0
rows=0
declare -A seen=()

printf '%-40s %-10s %-24s %s\n' "CASE" "LUABOX" "CODES" "LUALS"
printf '%-40s %-10s %-24s %s\n' "----" "------" "-----" "-----"

while IFS=$'\t' read -r case_name want_luabox want_codes want_luals note; do
    case "${case_name:-}" in ''|'#'*) continue ;; esac
    # Tab is an "IFS whitespace" character to bash's `read`, so a run of
    # adjacent tabs collapses instead of yielding an empty field the way a
    # comma or other non-whitespace IFS delimiter would — an all-tab TSV
    # cannot represent an empty column positionally. expected.tsv spells a
    # clean row's (empty) codes column as the literal `-` for exactly this
    # reason; normalise it back to empty here, once, rather than special-case
    # it at every comparison site below.
    [ "$want_codes" = "-" ] && want_codes=""
    rows=$((rows + 1))
    seen["$case_name"]=1
    src="$corpus/$case_name.lua"
    if [ ! -f "$src" ]; then
        echo "FAIL  $case_name: expected.tsv names it, but $src does not exist" >&2
        fails=$((fails + 1))
        continue
    fi
    # A row whose two columns disagree is only a justified, reviewed
    # divergence if it says so — an empty note on a diverging row is a
    # silent, unenforced allowlist entry wearing this file's clothes (F28).
    if [ "$want_luabox" != "$want_luals" ] && [ -z "${note:-}" ]; then
        echo "FAIL  $case_name: luabox ($want_luabox) and luals ($want_luals) disagree but the note column is empty — a divergence must be justified in writing, not left implicit" >&2
        fails=$((fails + 1))
    fi

    # --- luabox column ----------------------------------------------------
    rm -f "$work/src"/*.lua
    cp "$src" "$work/src/main.lua"
    # Cross-file cases: a `<case>.deps` sidecar names sibling module files
    # (one per line) copied in under their own names, so the case's
    # `require("name")` resolves for luabox exactly as it does for luals
    # (whose workspace is the corpus directory itself). `|| [ -n "$dep" ]`
    # on the read keeps a final line with no trailing newline (none exist
    # today, but nothing enforced that). Every dep name is checked against a
    # path-traversal shape before it is used as a cp destination — read
    # verbatim from a PR-authored .deps sidecar, `$work/src` is only three
    # path components deep, and an unvalidated `../../..` would clamp at `/`
    # and make the destination fully attacker-chosen (F27). A dep that fails
    # to copy — missing, renamed, unsafe — fails the row instead of leaving
    # `require` silently unresolved (F18: an ignored `cp` failure here used
    # to make luabox report LB0302 for the wrong reason, and the row still
    # read "diag" — verifying nothing about the case it names).
    dep_fail=0
    if [ -f "$corpus/$case_name.deps" ]; then
        while IFS= read -r dep || [ -n "$dep" ]; do
            [ -n "$dep" ] || continue
            case "$dep" in
            */*|.|..)
                echo "FAIL  $case_name: unsafe dependency name '$dep' in $case_name.deps — must be a plain filename in this directory, no path separators" >&2
                dep_fail=1
                break
                ;;
            esac
            if ! cp "$corpus/$dep" "$work/src/$dep" 2>/dev/null; then
                echo "FAIL  $case_name: could not copy dependency '$dep' named in $case_name.deps" >&2
                dep_fail=1
                break
            fi
        done <"$corpus/$case_name.deps"
    fi
    if [ "$dep_fail" = 1 ]; then
        fails=$((fails + 1))
        continue
    fi

    # `--format json` keeps stdout pure JSON (never mixed with the human
    # summary), so the exact diagnostic code SET can be asserted, not just
    # "the summary line wasn't all-zero" (F19: that check alone passes for
    # ANY diagnostic at all — an LB0306 the row names, an unrelated LB0300
    # regression, a parse error — indistinguishably, and it is the only
    # automated pin for the computed-key and __index narrowings the
    # limitations doc cites this gate for).
    out="$( (cd "$work" && timeout "$case_timeout" "$luabox" check --format json) 2>"$work/luabox.err" )"
    luabox_rc=$?
    # A kill from `timeout` (rc=124, its own sentinel) leaves $out empty or
    # mid-write — never valid JSON — so it is checked BEFORE the parse, not
    # folded into the generic parse-failure branch below where it would
    # report as a confusing JSON error instead of the hang it actually is.
    # Same handling, same ordering, same reasoning as the sibling driver's
    # measure_case (verdict-differential.sh, M32).
    if [ "$luabox_rc" = 124 ]; then
        echo "FAIL  $case_name: luabox timed out after ${case_timeout}s (LUALS_CASE_TIMEOUT) — raise the budget or fix the underlying perf bug" >&2
        sed 's/^/      /' "$work/luabox.err" >&2
        fails=$((fails + 1))
        continue
    fi
    py_codes_err="$work/luabox-codes.err"
    got_codes="$(printf '%s' "$out" | python3 -c '
import json, sys
data = json.load(sys.stdin)
print(",".join(sorted({d["code"] for d in data})))
' 2>"$py_codes_err")"
    codes_rc=$?
    if [ "$codes_rc" != 0 ]; then
        echo "FAIL  $case_name: could not parse 'luabox check --format json' output as JSON:" >&2
        sed 's/^/      /' "$py_codes_err" >&2
        echo "      stderr:" >&2
        sed 's/^/      /' "$work/luabox.err" >&2
        fails=$((fails + 1))
        continue
    fi
    if [ -z "$got_codes" ]; then
        got_luabox="clean"
    else
        got_luabox="diag"
    fi
    # M31, applied to this driver too (#58 review round 8, F12): this gate
    # never inspected luabox's own exit status at all, so a binary that
    # failed outright — a crash, a panic, an internal error before it ever
    # typechecked anything — and printed `[]` on the way out was recorded as
    # a clean, passing verdict against a row claiming `clean`. A nonzero exit
    # paired with an empty diagnostics array is never a legitimate clean
    # reading; fail the MEASUREMENT rather than let it stand in for one.
    # (`diag` with a nonzero exit is the ORDINARY shape — `luabox check`
    # exits nonzero precisely because it found something.)
    if [ "$got_luabox" = "clean" ] && [ "$luabox_rc" != 0 ]; then
        echo "FAIL  $case_name: luabox exited $luabox_rc but printed an empty diagnostics array — a nonzero exit is never a legitimate 'clean' reading" >&2
        sed 's/^/      /' "$work/luabox.err" >&2
        fails=$((fails + 1))
        continue
    fi
    if [ "$got_luabox" != "$want_luabox" ]; then
        echo "FAIL  $case_name: luabox is '$got_luabox', expected.tsv says '$want_luabox' (codes: ${got_codes:-none})" >&2
        fails=$((fails + 1))
    elif [ "$got_codes" != "$want_codes" ]; then
        echo "FAIL  $case_name: luabox codes are '$got_codes', expected.tsv says '$want_codes'" >&2
        fails=$((fails + 1))
    fi

    # --- luals column -----------------------------------------------------
    got_luals="(skip)"
    if [ "$luals_column" = 1 ]; then
        if printf '%s\n' "$luals_flagged" | grep -qxF "$case_name"; then
            got_luals="diag"
        else
            got_luals="clean"
        fi
        if [ "$got_luals" != "$want_luals" ]; then
            detail="$(grep "^$case_name"$'\t' "$work/luals-detail.tsv" 2>/dev/null | cut -f2)"
            if [ -n "$detail" ]; then
                echo "FAIL  $case_name: luals is '$got_luals', expected.tsv says '$want_luals' (matched: $detail)" >&2
            else
                echo "FAIL  $case_name: luals is '$got_luals', expected.tsv says '$want_luals' (no parity-set diagnostic in the case file or its .deps)" >&2
            fi
            fails=$((fails + 1))
        fi
    fi

    printf '%-40s %-10s %-24s %s\n' "$case_name" "$got_luabox" "${got_codes:--}" "$got_luals"
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

# Corpus rule: class names are unique per FILE, not just per case — luals
# checks this whole directory as one workspace, so a class name declared in
# two different corpus files would leak one file's members and diagnostics
# into the other's verdict. Multiple declarations of the SAME class in the
# SAME file (the intentional merge fixtures, e.g. duplicate_class_union) are
# the point of their own rows and are not a conflict. Enforced here, not
# just stated in this file's header comment (F28).
class_files="$(for f in "$corpus"/*.lua; do
    grep -hoE '^---@class[[:space:]]+[A-Za-z_][A-Za-z0-9_]*' "$f" \
        | awk -v file="$(basename "$f")" '{print $2, file}'
done | sort -u)"
dup_classes="$(printf '%s\n' "$class_files" | awk '{print $1}' | sort | uniq -d)"
if [ -n "$dup_classes" ]; then
    while IFS= read -r cls; do
        [ -n "$cls" ] || continue
        files="$(printf '%s\n' "$class_files" | awk -v c="$cls" '$1==c{print $2}' | paste -sd, -)"
        echo "FAIL  class '$cls' is declared in more than one corpus file ($files) — luals treats the corpus as one workspace; give it a per-file-unique name" >&2
        fails=$((fails + 1))
    done <<<"$dup_classes"
fi

echo
if [ "$fails" -gt 0 ]; then
    echo "luals-differential: $fails failure(s) across $rows row(s)"
    exit 1
fi
if [ "$luals_column" = 0 ]; then
    echo "luals-differential: $rows row(s), luabox column green; luals column SKIPPED"
else
    echo "luals-differential: $rows row(s), both columns match expected.tsv"
fi
