---
column: doing
labels: [release, burndown]
priority: high
agent: claude
live: true
status: Waves 21+22 launched in parallel; 23 (#54) and 24 (#46) to follow
updatedAt: 2026-08-01T15:40:00.000Z
---
# Burn-down 2: the issues the PR #47 review raised

Owner directive: burn down both what was open AND what the reviews opened. Eight burnable: #46 (round 1, re-scoped), #48/#49 (round 3 raise-don't-fix), #50/#51/#52/#53/#54 (round 10 sweep). Owner-only stays: #27 (tag), #28 (branch protection), #34 (marketplace credentials).

## Plan

- **Wave 21** (wt-w21, sprint/w21-types-cluster): #48 `---@type` over field assignment, #49 same-file `---@class` union, #50 global carrier members — all luabox-types annotation semantics.
- **Wave 22** (wt-w22, sprint/w22-cli-cluster): #51 project-walk symlink guard, #52 gitlab format (empty path + fingerprint collisions), #53 `lint --format` — manifest/CLI.
- **Wave 23**: #54 require-binding hover (luabox-lsp) — after a slot frees.
- **Wave 24**: #46 cross-module argument checking — the substantial checker feature, its own wave with design care.
- Then: develop → main PR through Shockwave (fixtures/tests-first discipline stands), deliberate closes with evidence.

## Comments

- **claude** (2026-08-01T15:40:00.000Z): Waves 21 and 22 launched in parallel (disjoint crates; CHANGELOG/board conflicts resolved keep-both at merge). Disk pre-checked: 21G free, both agents instructed to df before heavy builds and clean stale wt-*/target dirs.
