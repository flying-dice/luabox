---
name: goal-backlog
description: Maintain a goals backlog in the repo under goals/ where each goal has a GOAL.md definition and per-day append-only journal files, so any agent can pick up work with zero verbal handover. Use this skill whenever the user asks to define, start, resume, continue, check on, hand over, or close out a goal; whenever they mention the goals/ directory, a goals backlog, journaling work, or working towards something autonomously across sessions; and whenever you begin a work session in a repo that contains a goals/ directory — read the relevant journal before doing anything else.
---

# Goal Backlog

A repo-native system for defining goals with the user, delivering them autonomously across many sessions, and journaling every session so handover (to a future you, a different agent, or a human) requires no conversation.

## Directory layout

```
goals/
├── 01-migrate-ci-to-actions/
│   ├── GOAL.md
│   ├── 2026-06-28-initial-survey.journal.md
│   ├── 2026-06-30-workflow-drafts.journal.md
│   └── 2026-07-02-flaky-test-fixes.journal.md
└── 02-public-api-docs/
    ├── GOAL.md
    └── 2026-07-01-audit-existing-docs.journal.md
```

Rules:
- Each goal lives in `goals/NN-short-name/` where `NN` is a two-digit sequence number (next available; never reuse) and `short-name` is a kebab-case slug.
- `GOAL.md` is the single source of truth for what the goal *is*. It may be edited as understanding evolves, but edits must be recorded in that day's journal.
- Journal files are named `YYYY-MM-DD-short-slug.journal.md` where the slug summarizes the session's focus. One file per work session; if you work on the same goal twice in one day, pick distinct slugs.
- **Journals are append-only.** Never edit, rewrite, or delete past journal entries or past journal files. Corrections go in a new entry that references the old one ("Correction to 2026-06-30 entry: ..."). This is what makes the history trustworthy.

## Workflow

There are four modes. Detect which one applies from the user's request and the state of `goals/`.

### 1. Defining a new goal (with the user)

Goal definition is collaborative — do not invent a goal spec unilaterally. Interview the user briefly to pin down: the outcome, how you'll both know it's done (acceptance criteria), constraints (tech, style, things not to touch), and roughly how autonomous you should be (what needs sign-off vs. what you may just do).

Then create `goals/NN-name/GOAL.md` from this template:

```markdown
# Goal: <one-line statement>

**Status:** active            <!-- active | paused | blocked | done | abandoned -->
**Created:** YYYY-MM-DD
**Owner:** <user name/handle if known>

## Outcome
What the world looks like when this goal is complete. Concrete and testable.

## Acceptance criteria
- [ ] Checkbox list of verifiable conditions

## Constraints & guardrails
Things the agent must respect: tech choices, areas of the codebase that are off-limits,
review requirements, deadlines.

## Autonomy level
What the agent may do without asking vs. what requires user sign-off
(e.g. "implement freely on a branch; ask before merging or changing public APIs").

## Context & links
Relevant files, issues, prior art, decisions already made.
```

Confirm the GOAL.md content with the user before starting work, then write an initial journal entry recording that the goal was defined.

### 2. Working a session (autonomous delivery)

This is the default mode once a goal exists.

1. **Orient first.** Read `GOAL.md`, then the most recent journal file in full, and skim earlier ones as needed. The journals are the memory — trust them over assumptions. Specifically act on the previous session's "Next steps" and "Open questions".
2. **Start the session journal.** Create `goals/NN-name/YYYY-MM-DD-slug.journal.md` (get the real current date from the environment, don't guess) and write the header before doing substantive work.
3. **Work towards the goal.** Stay within the constraints and autonomy level in GOAL.md. Prefer making real progress over asking permission for things GOAL.md already authorizes; conversely, stop and ask (or record a blocker) for anything it reserves for the user.
4. **Journal as you go, not just at the end.** Append entries at meaningful moments: decisions made and why, approaches tried and rejected, surprises, files touched. A session that dies mid-way should still leave a usable journal.
5. **Close the session.** Always end with the Next steps, Open questions, and Follow-ups & improvements sections, and tick off any acceptance criteria in GOAL.md that are now met (with a journal entry noting it).

### 3. Handover / status check

When the user asks "where are we on X", produce a summary from GOAL.md status + acceptance checkboxes + the latest journal's Next steps. Never summarize from memory of previous conversations — the files are canonical.

### 4. Closing a goal

When all acceptance criteria are met (or the user abandons the goal): update `Status:` in GOAL.md, write a final journal entry summarizing the arc of the work and where everything landed, and leave the directory in place — done goals are history, not clutter.

## Journal file template

```markdown
# Journal: <goal one-liner> — YYYY-MM-DD (<slug>)

**Goal:** goals/NN-name/GOAL.md
**Session focus:** what this session set out to do

## Log
### HH:MM — <short heading>
What was done / decided / discovered. Why. Files touched.

### HH:MM — <short heading>
...

## Next steps
- Ordered, concrete, resumable by someone with no other context

## Open questions
- Things needing user input or future investigation (write "none" if none)

## Follow-ups & improvements
- Ideas outside the current scope: refactors noticed along the way, tech debt,
  nicer approaches, adjacent problems worth a future goal. Not commitments —
  a parking lot so good ideas aren't lost. (Write "none" if none.)
```

Timestamps can be approximate; their job is ordering, not precision.

## Writing journals for handover

The test for every entry: *could a competent agent with no chat history resume from this?* That means:
- Record **why**, not just what. Rejected approaches are as valuable as chosen ones.
- Name exact files, commands, branch names, and error messages instead of "fixed the config".
- Note anything you promised the user or the user told you mid-session — the conversation vanishes; the journal doesn't.
- Keep entries terse. This is a lab notebook, not a report.

## Edge cases

- **No `goals/` directory yet:** create it at the repo root when defining the first goal.
- **Numbering collision or gaps:** use the lowest unused number; gaps from abandoned goals are fine.
- **User asks to change a goal's scope:** edit GOAL.md, and journal the change with the rationale ("Scope change: ...").
- **Blocked:** set `Status: blocked` in GOAL.md, journal the blocker precisely, and surface it to the user.
- **Multiple active goals:** ask which one to work unless the user specified or one is clearly implied.
