---
column: review
labels: [docs]
priority: med
updatedAt: 2026-07-29T05:20:00.000Z
---
# Bootstrap RepoDoc in this repo

Adopt RepoDoc: board, decisions, docs tree (long-form root docs moved in per owner decision), workflow skill installed. Awaiting human sign-off.

## Comments

- **claude** (2026-07-29T05:20:00.000Z): Bootstrapped RepoDoc - board config at boards/improvement-sprint/.config.json, 11 cards covering the improvement sprint (done waves are verified merges, not aspirations), 6 decisions backfilled from DIRECTION.md and this session's owner-approved choices, docs/ tree created by MOVING SPEC/LIMITATIONS/RELEASING/BACKLOG/PRODUCTION-READINESS in (owner chose the move over thin index pages; root stubs keep code-comment citations followable, and the one mechanical reference - include_str in crates/luabox-manifest/src/parse.rs:740 - was retargeted). Workflow skill installed verbatim from flying-dice/repodoc src/core/skillContent.ts @ d9b6a00. Board deliberately carries sprint state only - GitHub issues remain the tracker of record (owner decision).
