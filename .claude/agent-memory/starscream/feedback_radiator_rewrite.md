---
name: feedback-radiator-rewrite
description: STATUS.md/card must be rewritten from the tracker at every push, never patched line-by-line — Shockwave blocked !5 twice on stale radiator claims (2026-09-06)
metadata:
  type: feedback
---

Before every push that touches `STATUS.md` or the sprint card, regenerate the `## Now` / `## In flight` / checklist sections from live tracker state (`glab mr view`, `glab issue view`, pipeline ids) and drop any SHA pins from headers. Do not patch individual lines — the untouched neighbours go stale and a reviewer measures every claim.

**Why:** MR !5 went three rounds on radiator staleness alone: a "pipeline 20256 green" line that was known false, header SHAs wrong on every push, MR states that had merged since. Each round cost ~15 min of runner time plus a review.

**How to apply:** treat the radiator as a snapshot with a timestamp; write it last, after all merges/notes of the turn; verify with a one-shot script that greps every `!N`/`#N`/pipeline id against the API before committing.
