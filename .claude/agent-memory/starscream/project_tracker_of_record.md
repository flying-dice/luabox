---
name: project-tracker-of-record
description: luabox tracker/remote topology — GitLab origin is live tracker + CI; GitHub is public mirror + release pipeline; docs said the opposite (stale as of 2026-09-06)
metadata:
  type: project
---

GitLab (`gitlab.beluga-sirius.ts.net/flying-dice/luabox`, remote `origin`) is the live issue tracker and CI (`.gitlab-ci.yml`). GitHub (`flying-dice/luabox`, remote `github`) is a public mirror whose `release.yml` cuts releases on tags; it had 0 open issues/PRs on 2026-09-06. Issues were migrated GitHub→GitLab (~2026-08-10) keeping numbers; `migration-placeholder`-labelled issues are mirrored GitHub PRs, not work.

**Why:** the project bounced GitHub→GitLab→GitHub→GitLab. `docs/04-project/01-backlog.md` still claimed GitLab archived / GitHub authoritative when I took over — I corrected it in the adoption sprint.

**How to apply:** file/sync issues on GitLab via `glab`. Keep `github/main` and `origin/main` in lockstep (both were at the same SHA on takeover; pushes to origin may need mirroring — verify before assuming auto-mirror). Release tagging (`v0.2.0`, GL#27) is a human owner action.
