# Backlog

**The backlog is the issue tracker on `origin` —
`gitlab.beluga-sirius.ts.net/flying-dice/luabox` (project 40).** That is the
only live tracker (decision 14). This file is a pointer and a citation
convention; it holds no work items of its own.

## Citing an issue

- **`#N`** — a GitLab issue on project 40. Numbers `#1`–`#74` are the items
  migrated from GitHub `flying-dice/luabox` with their numbers preserved, so
  every `#N` written while GitHub was the tracker still resolves. Migrated
  GitHub *pull requests* appear as issues labelled `migration-placeholder`;
  they are history, not work.
- **`GL#NNN`** — an issue on the earlier, archived GitLab project the codebase
  lived in before its GitHub period (`GL#94`, `GL#102`, `GL#137`, …). Kept in
  comments only where it is the honest provenance of a decision; never as
  somewhere to go and look.

The two ranges collide (GitLab-era `GL#23` was differential execution; `#23` is
a manifest-parsing rule), which is why the prefix exists. New references need
no prefix.

## Where GitHub still matters

`github.com/flying-dice/luabox` is the public mirror and the release surface:
`.github/workflows/release.yml` cuts a release when the owner pushes a tag
(#27), and the Actions CI covers the macOS/Windows matrix the GitLab runners
cannot yet. Its issue tracker is closed — do not file there.

## Archived: the first GitLab backlog

The launch-gate milestone, the checker-deepening and LSP build-out waves, the
`.luab` removal and the release machinery all closed there, and their outcomes
are recorded where they belong: [CHANGELOG.md](../../CHANGELOG.md) for what
shipped, [DIRECTION.md](../../DIRECTION.md) for why. Two items outlived it:
marketplace publication of the editor extensions (#34, was `GL#102`), and
registry UX (`search`, `login`) — parked post-v1 with dependency management
itself (was `GL#137`), recorded in [DIRECTION.md](../../DIRECTION.md) and SPEC §6.

## Planning artefacts

Root `STATUS.md` (current state), `docs/bots/ROADMAP.md` (milestones),
`boards/improvement-sprint/` (sprint cards), `decisions/` (engineering
decisions). Do not add items here — open an issue on `origin`.
