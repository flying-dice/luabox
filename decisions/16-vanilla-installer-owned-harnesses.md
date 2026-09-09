---
status: Accepted
date: 2026-09-09
---
# Decision 16 — Vanilla installer-owned harnesses, not repository agent memory

## Context

The owner replaced LuaBox's custom persona roster, skills and harness state with
the vanilla roster and shared skills from `flying-dice/bots`, for both Claude
and Codex. Decision 14 still required committed lead memory and described a
persona-specific dispatch process, despite those customizations being removed.
Historical sprint journals remain evidence of the earlier process.

## Decision

1. **The installed vanilla roster is authoritative.** Project-scope agent and
   skill definitions come from `flying-dice/bots` with `--variant vanilla` and
   `--harness all`. Refer to roles such as lead, architect, developer and tester,
   not retired persona names, in current operating instructions. Do not add
   custom agents, copied legacy skills or local edits to generated definitions.
   Changes to those definitions belong upstream in the installer.
2. **No repository agent memory.** This supersedes decision 14 clause 6's
   requirement to commit `.claude/agent-memory/<agent>/`. Do not recreate or
   commit that directory or equivalent custom memory under another harness.
   Durable project knowledge belongs in reviewed `docs/`, `decisions/` and
   tracker discussions, not a bot's private/session memory. Machine-specific
   installer manifests remain local as in decision 14.
3. **Harness parity remains a gate, not a customization mechanism.** Decision
   14 clause 5 remains applicable to generated copies. The installer-owned
   Codex `.agents/skills/lead/SKILL.md` dispatcher is a legitimate one-sided
   surface; it is not retained custom memory or an extra legacy persona.
4. **Reviewer identity is separate from the local roster.** The deployed
   Shockwave service and GitLab `shockwave` account are an active review
   integration, not a custom LuaBox harness agent. Assign that account and
   mention it on the MR when requesting its review. This supersedes decision
   14 clause 3's account-stood-down and persona-dispatched-review statements.
   Integration remains on `develop`; promotion to `main` and publication
   require the owner's authorization. This decision grants no new merge,
   deployment or release permissions.
5. **Preserve historical records.** Do not rename identities in past audits,
   branch names, journals or accepted decisions. Mark stale status snapshots
   as historical rather than inventing current tracker state. GitLab remains
   the tracker and pipeline of record.

## Consequences

Reinstalling the vanilla harnesses must not resurrect agent-memory files.
Current planning documents use role names; historical descriptions can retain
the names of their original participants. Decision 14's other tracker, branch,
quality and owner-authorization rules are unchanged.
