#!/usr/bin/env bash
#
# The Seekers' instruction set is checked in more than once, because each
# harness only reads its own root:
#
#   .claude/skills/<name>/      Claude resolves `skill:<name>` here
#   .agents/skills/<name>/      Codex resolves the same key here
#   .claude/agents/bots/<n>.md  Claude agent definition
#   .codex/agents/<n>.toml      Codex agent definition (body in
#                               `developer_instructions`)
#
# The duplication is the harnesses' requirement, not a defect. The defect is
# that nothing failed when the copies drifted: an edit that tightened a rule
# for Claude-run agents left Codex-run agents on the old rule, silently, and
# the repo could not tell you. This script is that missing signal (decision 14).
#
# It checks rules, not an inventory, so it does not need updating when a skill
# or a bot is added:
#
#   1. Skill trees are byte-identical, except SKILL_ONE_SIDED below. Every
#      compared skill file is non-empty.
#   2. Agent definitions exist on both sides (a Codex-only or Claude-only
#      agent fails, except AGENT_ONE_SIDED below); a Codex agent body equals
#      its Claude twin plus an appended "## Operating in Codex" section
#      (the section must actually be present — deleting it and having the
#      truncated body happen to match is not parity), once the harness-API
#      terms in CLAUDE_TO_CODEX_TERMS are translated; neither body is empty.
#   3. A skill that mirrors an agent definition equals that agent's Codex
#      body, and is non-empty.
#
# Run it directly; it takes under a second and needs nothing but coreutils.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

CLAUDE_SKILLS_DIR=.claude/skills
CODEX_SKILLS_DIR=.agents/skills
CLAUDE_AGENTS_DIR=.claude/agents/bots
CODEX_AGENTS_DIR=.codex/agents

# Skills that exist under one harness root only, and why.
#   lead — the tech-lead role. Claude loads it as an agent definition
#   (.claude/agents/bots/lead.md) and has no .claude/skills twin; Codex
#   has the agent definition too (.codex/agents/lead.toml) and ALSO
#   exposes it as a skill so `$lead` adopts the role in a primary
#   session. Rule 3 below is what keeps that extra copy honest.
SKILL_ONE_SIDED=lead

# Agents that exist under one harness root only, and why. Space-separated
# stems (no extension). Empty at present — every .codex/agents/*.toml has a
# .claude/agents/bots/*.md twin, and vice versa.
AGENT_ONE_SIDED=

# Wording that legitimately differs between the two harnesses because the
# underlying capability is named differently. One entry per line,
# "<claude text>\t<codex text>". Anything NOT listed here must match.
CLAUDE_TO_CODEX_TERMS=$(
    printf '%s\n' \
        $'your agent memory\tyour notes file for this project' \
        $'(AskUserQuestion)\t(a direct question to the user)'
)

fails=0
fail() {
    printf 'FAIL: %s\n' "$1" >&2
    fails=$((fails + 1))
}

# A byte-identical (or otherwise equal) empty pair still passes an equality
# check. Catch that separately: content must have at least one non-blank
# character.
assert_nonblank() {
    local content="$1" label="$2"
    if ! printf '%s' "$content" | grep -q '[^[:space:]]'; then
        fail "$label is empty or whitespace-only"
    fi
}

is_one_sided() {
    local needle="$1" list="$2" name
    for name in $list; do
        [ "$name" = "$needle" ] && return 0
    done
    return 1
}

# Body of a markdown definition: everything after the YAML frontmatter, with
# the blank line that always follows the closing `---` dropped.
markdown_body() {
    awk 'NR == 1 && $0 == "---" { in_fm = 1; next }
         in_fm && $0 == "---"   { in_fm = 0; next }
         in_fm                  { next }
         !seen && $0 ~ /^[[:space:]]*$/ { next }
         { seen = 1; print }' "$1"
}

# Body of a Codex agent definition: the developer_instructions heredoc.
codex_body() {
    awk '$0 == "developer_instructions = \"\"\"" { in_body = 1; next }
         in_body && $0 == "\"\"\""               { in_body = 0; next }
         in_body                                  { print }' "$1"
}

# The Codex-only trailer and everything after it.
without_codex_trailer() {
    awk '$0 == "## Operating in Codex" { exit } { print }'
}

translate_claude_to_codex() {
    local line claude codex
    local body
    body=$(cat)
    while IFS=$'\t' read -r claude codex; do
        [ -n "$claude" ] || continue
        body=${body//"$claude"/"$codex"}
    done <<<"$CLAUDE_TO_CODEX_TERMS"
    printf '%s\n' "$body"
}

# Trailing blank lines are formatting, not instruction. Compare without them.
strip_trailing_blanks() {
    awk '{ lines[NR] = $0 }
         END { last = NR
               while (last > 0 && lines[last] ~ /^[[:space:]]*$/) last--
               for (i = 1; i <= last; i++) print lines[i] }'
}

# --- Rule 1: the two skill trees are byte-identical -------------------------

# `diff -r` reports both failure modes itself: "Only in <root>: <name>" for a
# one-sided skill, and a per-file diff for a drifted pair.
if ! diff -r -x "$SKILL_ONE_SIDED" "$CLAUDE_SKILLS_DIR" "$CODEX_SKILLS_DIR" >&2; then
    fail "the skill trees diverge (diff above). Apply the edit under both roots, or add the skill to SKILL_ONE_SIDED with its reason"
fi

skill_pairs=$(find "$CLAUDE_SKILLS_DIR" -mindepth 1 -maxdepth 1 -type d | wc -l)
if [ "$skill_pairs" -eq 0 ]; then
    fail "no skills found under $CLAUDE_SKILLS_DIR — this gate would pass vacuously"
fi

# A byte-identical empty file passes `diff -r` too. Every compared skill
# file, on both roots, must actually have content.
while IFS= read -r -d '' skill_file; do
    assert_nonblank "$(cat "$skill_file")" "$skill_file"
done < <(find "$CLAUDE_SKILLS_DIR" "$CODEX_SKILLS_DIR" -type f -print0)

# --- Rule 2 (forward): every Claude agent has a Codex twin, body parity -----

bots=0
for claude_def in "$CLAUDE_AGENTS_DIR"/*.md; do
    name=$(basename "$claude_def" .md)
    codex_def="$CODEX_AGENTS_DIR/$name.toml"

    if [ ! -f "$codex_def" ]; then
        is_one_sided "$name" "$AGENT_ONE_SIDED" && continue
        fail "$claude_def has no Codex twin at $codex_def"
        continue
    fi
    bots=$((bots + 1))

    claude_body=$(markdown_body "$claude_def")
    codex_body_raw=$(codex_body "$codex_def")
    assert_nonblank "$claude_body" "$claude_def body"
    assert_nonblank "$codex_body_raw" "$codex_def body"

    if ! printf '%s\n' "$codex_body_raw" | grep -qxF '## Operating in Codex'; then
        fail "$codex_def is missing the '## Operating in Codex' trailer required by rule 2"
    fi

    want=$(printf '%s\n' "$claude_body" | translate_claude_to_codex | strip_trailing_blanks)
    got=$(printf '%s\n' "$codex_body_raw" | without_codex_trailer | strip_trailing_blanks)

    if [ "$want" != "$got" ]; then
        fail "$codex_def has drifted from $claude_def (< Claude, > Codex):"
        diff <(printf '%s\n' "$want") <(printf '%s\n' "$got") >&2 || true
    fi

    # Rule 3: a skill copy of an agent definition tracks the Codex body.
    mirror="$CODEX_SKILLS_DIR/$name/SKILL.md"
    [ -f "$mirror" ] || continue
    mirrored=$(markdown_body "$mirror" | strip_trailing_blanks)
    full_codex=$(printf '%s\n' "$codex_body_raw" | strip_trailing_blanks)
    assert_nonblank "$mirrored" "$mirror body"
    if [ "$mirrored" != "$full_codex" ]; then
        fail "$mirror has drifted from $codex_def (< skill copy, > agent definition):"
        diff <(printf '%s\n' "$mirrored") <(printf '%s\n' "$full_codex") >&2 || true
    fi
done

if [ "$bots" -eq 0 ]; then
    fail "no agent definitions found under $CLAUDE_AGENTS_DIR — this gate would pass vacuously"
fi

# --- Rule 2 (reverse): every Codex agent has a Claude twin -------------------

for codex_def in "$CODEX_AGENTS_DIR"/*.toml; do
    name=$(basename "$codex_def" .toml)
    is_one_sided "$name" "$AGENT_ONE_SIDED" && continue
    claude_def="$CLAUDE_AGENTS_DIR/$name.md"
    if [ ! -f "$claude_def" ]; then
        fail "$codex_def has no Claude twin at $claude_def"
    fi
done

printf '%d skill tree(s) compared, %d agent definition pair(s) compared\n' \
    "$skill_pairs" "$bots"

if [ "$fails" -gt 0 ]; then
    printf '%d parity failure(s)\n' "$fails" >&2
    exit 1
fi

printf 'harness parity: OK\n'
