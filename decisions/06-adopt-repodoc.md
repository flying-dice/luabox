---
status: Accepted
date: 2026-07-29
---
# Decision 06 — Adopt RepoDoc for planning and documentation

## Context

The owner asked for RepoDoc bootstrap at the tail of the improvement sprint, choosing: the board carries sprint/session state only (GitHub issues stay the tracker of record), and long-form root docs MOVE into docs/ rather than being index-paged.

## Decision

boards/, decisions/, docs/ at the repo root per the RepoDoc convention; SPEC, LIMITATIONS, RELEASING, BACKLOG and PRODUCTION-READINESS relocated under docs/ with root stubs preserving prose citations (code comments cite 'SPEC.md SN' in ~40 files); the repodoc-workflow skill installed at .claude/skills/repodoc-workflow/SKILL.md.

## Consequences

Cards move by editing files and decisions are append-only. The stubs are permanent residents unless the ~200 prose citations are ever mass-updated. CHANGELOG.md and DIRECTION.md deliberately stay at root: release.yml extracts notes from CHANGELOG.md by path, and DIRECTION.md is the governing decision record these files summarize rather than replace.
