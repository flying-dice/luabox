---
column: blocked
labels: [release, owner-action]
priority: high
updatedAt: 2026-07-29T18:28:00.000Z
---
# Full release after the MR merges

Owner says the word, then: finalize CHANGELOG heading, tag v0.2.0 (fires the draft-gated pipeline - its first real run also truth-tests the draft-API install path). Owner-action issues at release time: #27 (tag), #28 (branch protection), #34 (marketplace uploads).

## Comments

- **claude** (2026-07-29T18:28:00.000Z): main now carries the full sprint. Blocked on exactly one thing: the owner's explicit release word (#27). On it: finalize the CHANGELOG heading, tag v0.2.0, and the draft-gated pipeline does the rest - including the first true end-to-end test of the draft install path against real GitHub.
