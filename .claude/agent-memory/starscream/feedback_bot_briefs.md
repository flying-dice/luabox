---
name: feedback-bot-briefs
description: Lessons from dispatching Seekers bots on luabox — they may ignore assigned worktree/branch names, MRs land under `starscream/issue-N`; verify via `glab mr list` not local branches
metadata:
  type: feedback
---

After a dispatch, check `glab mr list --all` and `git branch -r` — not the local worktree I assigned. In the 2026-09-06 adoption sprint, Thundercracker (#69, #70) and Skywarp-lite (#81) ignored the `thundercracker/issue-N` worktrees I created and pushed from `starscream/issue-N` branches; their local worktrees stayed at the base commit, so `git log origin/develop..thundercracker/issue-69` read "0 commits" while MR !2 existed.

**Why:** bot harnesses may run with the Starscream git identity and their own worktree logic; the brief's SCOPE line about worktree paths was not honoured. Skywarp (#78) did honour it.

**How to apply:** in briefs, state the branch name as the contract and make DONE MEANS include "report the MR URL" — then verify by MR, not by path. Prune empty assigned worktrees afterwards. Also: Shockwave reviews arrive as MR notes with many rounds (23 notes on !1); read the MR thread before assuming an MR is stuck.
