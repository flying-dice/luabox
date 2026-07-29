# Backlog

**The backlog is [GitHub issues on `flying-dice/luabox`](https://github.com/flying-dice/luabox/issues).**
That is the only live tracker. This file is a pointer and a citation
convention; it holds no work items of its own.

## Citing an issue

The project moved from a private GitLab instance to GitHub mid-flight, and
both trackers number from 1 — so a bare `#23` is genuinely ambiguous in
anything written before the move. The convention, applied wherever the two
ranges actually collide:

- **`#N`** — a GitHub issue or PR on `flying-dice/luabox`. This is the
  default; new references need no prefix.
- **`GL#NNN`** — a historical GitLab issue, **archived and unreachable**.
  Kept in comments only where it is the honest provenance of a decision;
  never as somewhere to go and look.

The collision is real today: GitLab #23 was differential execution, GitHub
#23 is a manifest-parsing rule; GitLab #14 was the LSP tranche, GitHub #14 is
a `---@source` bug. Anything above the GitHub high-water mark is
unambiguously GitLab and left bare.

## Archived: the GitLab backlog

The GitLab instance (`gitlab.beluga-sirius.ts.net/flying-dice/luabox`) is
**archived**. Its issues — the launch-gate milestone, the checker-deepening
and LSP build-out waves, the `.luab` removal, the release machinery — all
closed before the move, and their outcomes are recorded where they belong:
[CHANGELOG.md](../../CHANGELOG.md) for what shipped, [DIRECTION.md](../../DIRECTION.md)
for why. Two items outlived the instance and were re-filed on GitHub rather
than left behind:

- Marketplace publication of the editor extensions →
  [#34](https://github.com/flying-dice/luabox/issues/34) (was GL#102).
- Registry UX (`search`, `login`/auth) — **parked post-v1** with dependency
  management itself (was GL#137). Not re-filed: the commands no longer
  exist, and the luarocks.org direction that would bring them back is
  recorded in [DIRECTION.md](../../DIRECTION.md) and SPEC.md §6.

Nothing else from GitLab is pending. Do not add items here — open a GitHub
issue.
