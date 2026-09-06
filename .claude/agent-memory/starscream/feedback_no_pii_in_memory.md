---
name: feedback-no-pii-in-memory
description: Agent memory is committed and mirrored publicly — never write emails, real names beyond tracker usernames, or credentials into it (raised by Shockwave on MR !5, 2026-09-06)
metadata:
  type: feedback
---

Never put personal data in agent memory: no email addresses, no names beyond the tracker username, no hostnames-with-credentials, no tokens. Refer to people by role or GitLab username.

**Why:** memory lives in the repo (decision 14 clause 6) and `origin/main` push-mirrors to public GitHub; Shockwave caught the owner's email in `user_working_style.md` during the !5 review. The history still carries it — the owner decides whether to rewrite.

**How to apply:** before saving any memory, scan it for `@`, phone-like strings, secrets. The userEmail in the session context is for attribution only, never for memory.
