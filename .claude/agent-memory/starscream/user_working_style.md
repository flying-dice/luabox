---
name: user-working-style
description: How the owner runs Starscream sessions — unattended runs, direct-to-main for meta/config, expects STATUS.md + issue sync + push before exit
metadata:
  type: user
---

The owner (GitLab `jonathanturnock`) owns luabox. Runs Starscream sessions **unattended**: "do not wait for user input; proceed until blocked by human input on all items."

- Harness/skill/agent config (`.claude/`, `.agents/`, `.codex/`) may be committed straight to `main` when he asks — no MR ceremony for meta files.
- Product code goes through MRs with CI green; he is the CODEOWNER approver (human gate).
- Session exit contract he stated (2026-09-06): update RepoDoc cards, sync GitLab issues (notes/state/links), commit + push intermediate progress, keep root `STATUS.md` current per the `project-status` skill.
- Session goal framing he used: "sign this repo off as adopted to your standards" — he wants the Seekers to own quality, not just ship features.
