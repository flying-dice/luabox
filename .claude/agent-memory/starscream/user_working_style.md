---
name: user-working-style
description: How the owner runs Starscream sessions — unattended runs, direct-to-main for meta/config, approvals only on main promotions + tags, expects STATUS.md + issue sync + push before exit
metadata:
  type: user
---

The owner (GitLab `jonathanturnock`) owns luabox. Runs Starscream sessions **unattended**: "do not wait for user input; proceed until blocked by human input on all items."

- Harness/skill/agent config (`.claude/`, `.agents/`, `.codex/`, agent memory) may be committed straight to `main` when he asks — no MR ceremony for meta files.
- **Approvals (his call, 2026-09-06):** he does NOT review MRs into `develop` — Shockwave approves, Starscream merges. He approves only the `develop → main` promotion MR and pushes release tags. Written into decision 14 clause 3. Don't ask him to review feature MRs.
- Infra: he will hand over root SSH to the runner host in-session and expects a diagnosis + concrete fix, applied where automation is allowed and handed back as exact commands where it is not. Prefers aggressive eviction ("older than 24 hours, nuke").
- Session exit contract he stated: update RepoDoc cards, sync GitLab issues (notes/state/links), commit + push intermediate progress, keep root `STATUS.md` current per the `project-status` skill.
- Session goal framing he used: "sign this repo off as adopted to your standards" — he wants the Seekers to own quality, not just ship features.
