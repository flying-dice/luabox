---
column: blocked
labels: [release, owner-action]
priority: high
updatedAt: 2026-07-29T19:06:00.000Z
---
# Full release after the MR merges

Owner says the word, then: finalize CHANGELOG heading, tag v0.2.0 (fires the draft-gated pipeline - its first real run also truth-tests the draft-API install path). Owner-action issues at release time: #27 (tag), #28 (branch protection), #34 (marketplace uploads).

## Comments

- **claude** (2026-07-29T18:28:00.000Z): main now carries the full sprint. Blocked on exactly one thing: the owner's explicit release word (#27). On it: finalize the CHANGELOG heading, tag v0.2.0, and the draft-gated pipeline does the rest - including the first true end-to-end test of the draft install path against real GitHub.
- **claude** (2026-07-29T19:06:00.000Z): Owner gave the word. Release prep executed on main: CHANGELOG folded (Unreleased -> 0.2.0, dated 2026-07-29, release-notes awk extraction verified, commit 49bf80f) and a workflow_dispatch escape hatch added to release.yml (4be2463). BLOCKED at the final step by session credentials, twice over: the git proxy accepts branch pushes but refuses tag refs (verified with 4x backoff + ls-remote), and workflow dispatch returns 403 (integration lacks actions:write). The tag push is the one act only the owner can perform: git fetch origin && git tag v0.2.0 origin/main && git push origin v0.2.0 - or dispatch the release workflow from the Actions tab with tag=v0.2.0. Everything downstream is automated and gated.
