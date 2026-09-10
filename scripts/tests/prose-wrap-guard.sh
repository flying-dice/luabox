#!/bin/bash
# Enforce the repository's 79-column prose convention for CHANGELOG.md and
# docs/**/*.md (#87). Fenced code, Markdown table rows, and URL-only lines are
# excluded because wrapping them changes syntax or usability.
#
# Existing files that predate the rule are content-pinned in
# prose-wrap-legacy-baseline.txt. A matching whole-file hash is accepted, but
# any edit invalidates the exemption and makes the complete file subject to
# the guard. This keeps the initial gate green without allowing new long prose
# to hide behind a permanent path or line-number exception.
set -euo pipefail

repo_dir="${PROSE_WRAP_REPO_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
default_baseline="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/prose-wrap-legacy-baseline.txt"
baseline_file="${PROSE_WRAP_BASELINE_FILE:-$default_baseline}"
cd "$repo_dir"

files=()
if [ -f CHANGELOG.md ]; then
    files+=(CHANGELOG.md)
fi
if [ -d docs ]; then
    while IFS= read -r -d '' file; do
        files+=("$file")
    done < <(find docs -type f -name '*.md' -print0 | sort -z)
fi

if [ "${#files[@]}" -eq 0 ]; then
    echo "error: prose-wrap guard found no CHANGELOG.md or docs/**/*.md files" >&2
    exit 1
fi

declare -A legacy_hashes=()
if [ -f "$baseline_file" ]; then
    while read -r hash path extra; do
        [ -z "${hash:-}" ] && continue
        case "$hash" in \#*) continue ;; esac
        if [ -n "${extra:-}" ] || ! printf '%s' "$hash" | grep -Eq '^[0-9a-f]{64}$' || [ -z "${path:-}" ]; then
            echo "error: malformed prose-wrap baseline entry: $hash ${path:-} ${extra:-}" >&2
            exit 1
        fi
        if [ -n "${legacy_hashes[$path]:-}" ]; then
            echo "error: duplicate prose-wrap baseline entry: $path" >&2
            exit 1
        fi
        legacy_hashes["$path"]="$hash"
    done <"$baseline_file"
fi

declare -A discovered=()
for file in "${files[@]}"; do
    discovered["$file"]=1
done
for path in "${!legacy_hashes[@]}"; do
    if [ -z "${discovered[$path]:-}" ]; then
        echo "error: unknown prose-wrap baseline path: $path" >&2
        exit 1
    fi
    actual="$(sha256sum "$path")"
    actual="${actual%% *}"
    if [ "${legacy_hashes[$path]}" != "$actual" ]; then
        echo "error: stale prose-wrap baseline entry: $path" >&2
        echo "       remove it after reflowing the file, or update it deliberately" >&2
        exit 1
    fi
done

scan_files=()
baselined=0
for file in "${files[@]}"; do
    if [ -n "${legacy_hashes[$file]:-}" ]; then
        baselined=$((baselined + 1))
    else
        scan_files+=("$file")
    fi
done

if [ "${#scan_files[@]}" -eq 0 ]; then
    if [ "$baselined" -gt 0 ]; then
        exit 0
    fi
    echo "error: prose-wrap guard found no eligible Markdown inputs" >&2
    exit 1
fi

awk '
function fence_marker(line, trimmed) {
    trimmed = line
    sub(/^[[:space:]]*/, "", trimmed)
    if (substr(trimmed, 1, 3) == "```") return "`"
    if (substr(trimmed, 1, 3) == "~~~") return "~"
    return ""
}

FNR == 1 { in_fence = 0; marker = "" }
{
    current = fence_marker($0)
    if (current != "") {
        if (!in_fence) {
            in_fence = 1
            marker = current
        } else if (current == marker) {
            in_fence = 0
            marker = ""
        }
        next
    }
    if (in_fence) next
    if ($0 ~ /^[[:space:]]*\|/) next
    if ($0 ~ /^[[:space:]]*<?https?:\/\/[^[:space:]]+>?[[:space:]]*$/) next
    if ($0 ~ /[^[:space:]]/) measured++
    if (length($0) > 79) {
        printf "%s:%d:%d: prose exceeds 79 columns\n", FILENAME, FNR, length($0)
        failed = 1
    }
}

END {
    if (!measured) {
        print "error: prose-wrap guard found no eligible prose lines" > "/dev/stderr"
        exit 2
    }
    exit failed
}
' "${scan_files[@]}"
