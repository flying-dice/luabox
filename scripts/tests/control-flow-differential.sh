#!/bin/bash
# Control-flow legality differential (#44): for every program in
# scripts/tests/control-flow-matrix/ and every Lua version with a reference
# compiler on PATH, `luac<V> -p` is the GROUND TRUTH and `luabox check`
# (edition = V) must reach the same accept/reject verdict.
#
# Verdicts are derived live, never hardcoded — including the one genuine
# dialect split (5.4 rejects a nested label shadowing an outer one where
# 5.2/5.3 accept it): whatever the reference compilers say per version is
# what luabox is held to. The single documented divergence lives in
# exceptions.tsv, direction-checked: luabox may ACCEPT where luac rejects
# (the under-approximation LIMITATIONS records), never reject what luac
# accepts.
#
# Two columns per version, because luabox has two ways to be asked about V:
#
#   edition=V   `luabox check`               — "is this source legal as written?"
#   target=V    `luabox check --target V`    — "would this source be legal there?"
#
# The target column pins the manifest edition to the MOST PERMISSIVE edition
# that parses every program in the matrix (5.2: it has goto/labels, and its
# duplicate-label scope is the loose pre-5.4 one), so the only thing that can
# move a verdict is the target. It exists because the target column was the
# hole Shockwave round 2 found: the control-flow legality pass ran for the
# edition only, so `edition = "5.2"` + `--target 5.4` accepted
# split_label_shadow_nested.lua — which luac5.4 rejects.
#
# That "most permissive" claim is an INVARIANT OF THE MATRIX, and it is
# enforced below rather than asserted here (Shockwave round 4). A future
# program using a 5.3+ construct — `local x = 7 // 2` is the easy one — would
# still parse under its own edition column, but under the target column it
# would be rejected for being illegal 5.2 rather than illegal on the target.
# The cell then reads luabox=reject / luac=accept, which this script calls
# never-OK, and it would blame luabox for a defect in the matrix. So before
# the columns run, every program is checked once at TARGET_COLUMN_EDITION
# with no target: any parse error (LB0001) or edition-dialect error
# (LB0010-LB0019) fails the script loudly, naming the program. Control-flow
# rejections (LB0020-LB0022) are expected — half the matrix is illegal
# programs — and are not invariant violations.
#
# exceptions.tsv cannot express this: it is keyed program+version, with no
# column dimension. The fix when this fires is to change the program, or to
# give the matrix a per-program column edition — not to except the cell.
#
# Versions without a luac on PATH SKIP loudly (this box may only carry
# 5.1/5.4); CI's differential job installs 5.1-5.4 so every column runs
# there. LuaJIT has no luac; its label semantics follow 5.2, which the 5.2
# column covers.
set -u

# The edition the target column pins; see the header.
TARGET_COLUMN_EDITION=5.2

here="$(cd "$(dirname "$0")" && pwd)"
matrix="$here/control-flow-matrix"
luabox="${LUABOX:-$here/../../target/release/luabox}"

if [ ! -x "$luabox" ]; then
    echo "error: no luabox binary at $luabox (build with: cargo build --release --bin luabox, or set LUABOX)" >&2
    exit 1
fi

fails=0
cells=0
skipped_versions=""

# exceptions.tsv: program<TAB>space-separated-versions<TAB>reason
exception_for() { # $1=program $2=version -> prints reason, rc 0 if excepted
    local prog="$1" ver="$2" name vers reason
    while IFS=$'\t' read -r name vers reason; do
        case "$name" in ''|'#'*) continue ;; esac
        if [ "$name" = "$prog" ]; then
            case " $vers " in *" $ver "*) printf '%s' "$reason"; return 0 ;; esac
        fi
    done < "$matrix/exceptions.tsv"
    return 1
}

# --- matrix invariant: every program parses under the target column edition
# See the header. This needs no reference compiler, so it runs even when every
# version SKIPs, and it runs before any cell so a violation is reported once
# rather than once per column.
preflight="$(mktemp -d)"
mkdir -p "$preflight/src"
printf '[package]\nname = "cfdiff"\nversion = "0.1.0"\nedition = "%s"\n' \
    "$TARGET_COLUMN_EDITION" > "$preflight/luabox.toml"
violations=0
for prog in "$matrix"/*.lua; do
    base="$(basename "$prog" .lua)"
    cp "$prog" "$preflight/src/main.lua"
    out="$( (cd "$preflight" && "$luabox" check) 2>&1 )"
    if printf '%s\n' "$out" | grep -Eq 'error\[LB0001\]|error\[LB001[0-9]\]'; then
        echo "INVARIANT  $base does not parse cleanly at edition $TARGET_COLUMN_EDITION:" >&2
        printf '%s\n' "$out" | grep -E 'error\[LB0001\]|error\[LB001[0-9]\]' >&2
        violations=$((violations + 1))
    fi
done
rm -rf "$preflight"
if [ "$violations" -gt 0 ]; then
    echo >&2
    echo "control-flow differential: $violations matrix program(s) violate the target-column invariant." >&2
    echo "  The target column pins edition $TARGET_COLUMN_EDITION so that only --target can move a verdict." >&2
    echo "  A program that is not legal $TARGET_COLUMN_EDITION source makes that column measure" >&2
    echo "  ${TARGET_COLUMN_EDITION}-legality AND target legality, and fails as luabox=reject/luac=accept —" >&2
    echo "  blaming luabox for a defect in the matrix. Rewrite the program in $TARGET_COLUMN_EDITION," >&2
    echo "  or give the matrix a per-program column edition. exceptions.tsv cannot express this." >&2
    exit 1
fi

for ver in 5.1 5.2 5.3 5.4; do
    luac="luac$ver"
    if ! command -v "$luac" >/dev/null 2>&1; then
        echo "SKIP  version $ver: no $luac on PATH"
        skipped_versions="$skipped_versions $ver"
        continue
    fi

    work="$(mktemp -d)"
    mkdir -p "$work/src"
    edition_toml="$work/edition.toml"
    target_toml="$work/target.toml"
    printf '[package]\nname = "cfdiff"\nversion = "0.1.0"\nedition = "%s"\n' "$ver" > "$edition_toml"
    printf '[package]\nname = "cfdiff"\nversion = "0.1.0"\nedition = "%s"\n' \
        "$TARGET_COLUMN_EDITION" > "$target_toml"

    for prog in "$matrix"/*.lua; do
        base="$(basename "$prog" .lua)"

        if "$luac" -p "$prog" >/dev/null 2>&1; then ref=accept; else ref=reject; fi
        cp "$prog" "$work/src/main.lua"

        for column in edition target; do
            cells=$((cells + 1))
            if [ "$column" = edition ]; then
                cp "$edition_toml" "$work/luabox.toml"
                if (cd "$work" && "$luabox" check >/dev/null 2>&1); then lb=accept; else lb=reject; fi
            else
                cp "$target_toml" "$work/luabox.toml"
                if (cd "$work" && "$luabox" check --target "$ver" >/dev/null 2>&1); then
                    lb=accept
                else
                    lb=reject
                fi
            fi

            if [ "$ref" = "$lb" ]; then
                continue
            fi
            if reason="$(exception_for "$base" "$ver")"; then
                # Direction check: the documented divergence is accept-where-
                # luac-rejects ONLY. luabox rejecting what luac accepts is
                # never OK.
                if [ "$ref" = reject ] && [ "$lb" = accept ]; then
                    echo "EXCEPT  $base @ $ver ($column): luac=$ref luabox=$lb ($reason)"
                    continue
                fi
            fi
            echo "FAIL  $base @ $ver ($column): luac=$ref luabox=$lb"
            fails=$((fails + 1))
        done
    done
    rm -rf "$work"
done

echo
if [ "$cells" -eq 0 ]; then
    echo "control-flow differential: NO CELLS RAN (no reference luac at all?)" >&2
    exit 1
fi
if [ "$fails" -gt 0 ]; then
    echo "control-flow differential: $fails/$cells cells DISAGREE with reference luac" >&2
    exit 1
fi
echo "control-flow differential: all $cells cells agree with reference luac${skipped_versions:+ (skipped versions:$skipped_versions)}"
