---
status: Accepted
date: 2026-09-06
---
# Decision 14 — The Seekers own luabox; GitLab is the tracker and pipeline of record; `develop` integrates, `main` releases

## Context

On takeover (2026-09-06) the repo told two stories. `docs/04-project/01-backlog.md`
said the GitLab instance was archived and GitHub issues were the only live
tracker. Reality: `origin` is `gitlab.beluga-sirius.ts.net:flying-dice/luabox`
(project 40, default branch `develop`, `main` push/merge restricted to
Maintainers), every GitHub issue had been migrated there with its number
preserved (`#1`–`#74`, GitHub PRs as `migration-placeholder` issues), GitHub had
zero open issues or PRs, and `.gitlab-ci.yml` on `develop` declared itself the
pipeline of record (decision 12/13 era, GL#74). `main` was eight commits behind
`develop` — PR #61 (post-v1 hardening, 295 files) had merged to `develop` on
2026-08-09 and never been promoted. `main`'s coverage jobs were red on a cold
cache (`$CARGO_HOME/bin` absent before the cargo-llvm-cov tarball extract).

The Seekers (Starscream lead; Thundercracker, Skywarp developers; Shockwave QA;
Soundwave architecture; Thrust design) take the project over as a team. A team
needs one answer to "where is the work, where is the truth, where do decisions
live" — the repo already has RepoDoc (`boards/`, `decisions/`, `docs/`) and this
decision must not add a second system beside it.

## Decision

1. **GitLab `origin` is the tracker and pipeline of record.** Issues, MRs,
   reviews and `.gitlab-ci.yml` gates live there. Bare `#N` in prose and
   commits means a GitLab issue on project 40 (numbers `#1`–`#74` coincide with
   the migrated GitHub items by construction). `GL#NNN` keeps its historical
   meaning: an issue on the earlier, archived GitLab project.
2. **GitHub `flying-dice/luabox` is the public mirror and release surface.**
   `.github/workflows/release.yml` still cuts releases on tags; the Actions CI
   stays only for what this instance's runners cannot cover (macOS/Windows
   matrix) until GitLab has those runners. GitHub issues stay closed; do not
   file there.
3. **Branch model and who approves what.** `develop` is the integration
   branch and default; `main` is the release branch. Feature branches are
   `<persona>/issue-<N>` off `develop`, one MR each, pipeline green before
   review. Into `develop`: Shockwave — dispatched by Starscream as a
   read-only reviewer, its checklist and findings posted on the MR by
   Starscream — approves, and Starscream merges; the owner is not in that loop
   and the GitLab `shockwave` runner account is stood down (owner's call,
   2026-09-06). Into `main`: a
   `develop → main` promotion MR that the owner (`jonathanturnock`) approves,
   and release tags are pushed by the owner. There is no CODEOWNERS file; this
   clause is the rule.
4. **Planning artefacts reuse RepoDoc.** Root `STATUS.md` is the information
   radiator (`project-status` skill). `docs/bots/ROADMAP.md` holds milestones and
   order. Sprint tracking is a RepoDoc card on `boards/improvement-sprint/`
   (one card per sprint, journal in `## Comments`). Engineering decisions are
   `decisions/NN-slug.md` — not a parallel `docs/bots/decisions/`.
5. **The instruction set is checked in more than once, and CI holds the
   copies equal.** Each harness reads only its own root: skills at
   `.claude/skills/<name>/` (Claude) and `.agents/skills/<name>/` (Codex),
   agent definitions at `.claude/agents/bots/<n>.md` (Claude) and
   `.codex/agents/<n>.toml` (Codex). The duplication is the harnesses'
   requirement, not a defect — but an unguarded duplicate is two rules wearing
   one name, and the drift is silent. The selftest job `harness-skill-parity`
   runs `scripts/tests/harness-skill-parity.sh`, which enforces three rules:
   the skill trees are byte-identical apart from a named one-sided exception;
   a Codex agent body equals its Claude twin plus the appended
   `## Operating in Codex` section, once a small table of harness-API terms is
   translated; and a skill that mirrors an agent definition equals that
   agent's Codex body. The script checks rules, not an inventory, so adding a
   skill or a bot does not require editing it. Edit an instruction in one
   place only, and the pipeline says so.
6. **Harness state is not repo state.** The bot manifests
   (`.claude/bots-manifest.json`, `.codex/bots-manifest.json`) are generated
   per-machine with absolute paths and no in-repo consumer; they are
   `.gitignore`d and each harness regenerates them locally. Agent and skill
   *definitions* are repo state and are committed.
7. **Quality bar is the existing one, enforced on GitLab:** fmt, clippy
   (pedantic + restriction lints, `-D warnings`), workspace tests, unit
   coverage ≥95 (per-crate ≥92), e2e ≥83, luals parity and verdict
   differentials, self-tested gates (decision 12). Floors only move up.

## Consequences

- `docs/04-project/01-backlog.md` and `docs/01-getting-started/02-development.md`
  are corrected to name GitLab as the record; `README`'s public links stay on
  GitHub because that is where users land.
- `main` is reconciled with `develop` in this sprint (the merge keeps
  `develop`'s full pipeline; `main`'s trimmed port is superseded).
- Owner-only actions stay owner-only and are tracked as issues, never done by
  a bot: tagging `v0.2.0` (#27), marketplace publication (#34), repo settings
  (#28 — on GitLab the equivalent is *Remove all approvals when commits are
  added*, Settings → Merge requests).
- Anything that turns out to be wrong here gets a superseding decision, not a
  silent edit.
