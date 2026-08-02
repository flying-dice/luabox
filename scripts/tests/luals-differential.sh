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
#   luabox column   `luabox check` (strict) over a one-file project —
#                   `diag` if it reports any error or warning, else `clean`.
#   luals column    `lua-language-server --check` over this directory,
#                   filtered to the type-parity diagnostic code set below —
#                   `diag` if any such code fires for the case, else `clean`.
#
# The two columns are NOT asserted equal, and must not be: a row where they
# differ is a reviewed, intentional divergence whose justification lives in
# expected.tsv (e.g. luals 3.13.5 has no generic-class support). What the
# script enforces is that both columns match what is written down — so a
# parity drift in EITHER tool shows up as a diff against a claim. This is
# the lb0510-matrix discipline applied to luals parity.
#
# No lua-language-server on PATH (and no LUALS env) means the luals column
# SKIPs, loudly, and the luabox column still runs. CI's luals-parity job
# always installs the pinned version, so both columns run there.
set -u

here="$(cd "$(dirname "$0")" && pwd)"
corpus="$here/luals-differential"
expected="$corpus/expected.tsv"
luabox="${LUABOX:-$here/../../target/release/luabox}"
luals="${LUALS:-lua-language-server}"

# The diagnostic codes that constitute the type-parity surface. Everything
# else luals reports (style, unused locals, …) is out of scope for the
# comparison — .luarc.json in the corpus disables the noisiest, and this set
# is the positive filter.
parity_codes="param-type-mismatch missing-parameter undefined-field undefined-doc-name duplicate-doc-field assign-type-mismatch return-type-mismatch missing-return missing-fields cast-local-type undefined-global"

if [ ! -x "$luabox" ]; then
    echo "error: no luabox binary at $luabox (build with: cargo build --release --bin luabox, or set LUABOX)" >&2
    exit 1
fi
if [ ! -f "$expected" ]; then
    echo "error: no expectations at $expected" >&2
    exit 1
fi

luals_column=1
if ! command -v "$luals" >/dev/null 2>&1; then
    luals_column=0
    echo "SKIP  luals column: no $luals on PATH — the parity half of every"
    echo "SKIP  claim below is UNVERIFIED in this run. Install"
    echo "SKIP  lua-language-server (or set LUALS) to check it; CI's"
    echo "SKIP  luals-parity job always does."
    echo
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/src"
printf '[package]\nname = "lualsdiff"\nversion = "0.1.0"\nedition = "5.4"\n\n[types]\nstrict = true\n' \
    > "$work/luabox.toml"

# One luals run over the whole corpus; per-case verdicts parsed out of the
# JSON afterwards. (--check writes to <out>/check.json; a run with zero
# problems writes nothing, which is a valid "all clean".)
luals_json=""
if [ "$luals_column" = 1 ]; then
    luals_out="$work/luals-out"
    mkdir -p "$luals_out"
    if ! "$luals" --check "$corpus" --checklevel=Warning \
        --check_out_path="$luals_out/check.json" >/dev/null 2>&1; then
        echo "error: $luals --check failed to run" >&2
        exit 1
    fi
    luals_json="$luals_out/check.json"
fi

# The set of cases luals flagged with a parity-set code, one name per line.
luals_flagged=""
if [ "$luals_column" = 1 ] && [ -f "$luals_json" ]; then
    luals_flagged="$(python3 - "$luals_json" $parity_codes <<'PYEOF'
import json, sys
path, codes = sys.argv[1], set(sys.argv[2:])
with open(path) as f:
    data = json.load(f)
names = set()
for uri, diags in data.items():
    name = uri.rsplit("/", 1)[-1].removesuffix(".lua")
    if any(d.get("code") in codes for d in diags):
        names.add(name)
print("\n".join(sorted(names)))
PYEOF
)"
fi

fails=0
rows=0
declare -A seen=()

printf '%-40s %-10s %s\n' "CASE" "LUABOX" "LUALS"
printf '%-40s %-10s %s\n' "----" "------" "-----"

while IFS=$'\t' read -r case_name want_luabox want_luals note; do
    case "${case_name:-}" in ''|'#'*) continue ;; esac
    rows=$((rows + 1))
    seen["$case_name"]=1
    src="$corpus/$case_name.lua"
    if [ ! -f "$src" ]; then
        echo "FAIL  $case_name: expected.tsv names it, but $src does not exist" >&2
        fails=$((fails + 1))
        continue
    fi

    # --- luabox column ----------------------------------------------------
    rm -f "$work/src"/*.lua
    cp "$src" "$work/src/main.lua"
    # Cross-file cases: a `<case>.deps` sidecar names sibling module files
    # (one per line) copied in under their own names, so the case's
    # `require("name")` resolves for luabox exactly as it does for luals
    # (whose workspace is the corpus directory itself).
    if [ -f "$corpus/$case_name.deps" ]; then
        while IFS= read -r dep; do
            [ -n "$dep" ] || continue
            cp "$corpus/$dep" "$work/src/$dep"
        done <"$corpus/$case_name.deps"
    fi
    out="$( (cd "$work" && "$luabox" check) 2>&1 )"
    summary="$(printf '%s\n' "$out" | grep -E '^check: ' || true)"
    if [ -z "$summary" ]; then
        echo "FAIL  $case_name: no 'check:' summary in luabox output" >&2
        printf '%s\n' "$out" | sed 's/^/      /' >&2
        fails=$((fails + 1))
        continue
    fi
    if printf '%s' "$summary" | grep -qE '^check: 0 errors, 0 warnings'; then
        got_luabox="clean"
    else
        got_luabox="diag"
    fi
    if [ "$got_luabox" != "$want_luabox" ]; then
        echo "FAIL  $case_name: luabox is '$got_luabox', expected.tsv says '$want_luabox' ($summary)" >&2
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
            echo "FAIL  $case_name: luals is '$got_luals', expected.tsv says '$want_luals'" >&2
            fails=$((fails + 1))
        fi
    fi

    printf '%-40s %-10s %s\n' "$case_name" "$got_luabox" "$got_luals"
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
