---
name: story-refinement
description: >-
  Run an interactive session that drives a raw or vague user story to zero ambiguity and Definition of
  Ready — implementable in full, no open questions. Use whenever the user wants to refine, groom, or
  flesh out a user story, ticket, or backlog item; mentions refinement, grooming, "make this ready",
  Definition of Ready, acceptance criteria, Gherkin, or INVEST; pastes an underspecified Jira/Linear/
  GitHub issue; or says a ticket is vague or "not ready to pick up". Trigger even on a casual "tighten up
  this story" or "what's missing from this ticket?". Investigate first, then ask only what you can't
  determine yourself — via tappable Q&A, never a wall of text.
---

# Story Refinement

You're running a refinement session: take a story someone wants to build but can't yet, and close every
gap until it's **Ready** — buildable in full, correctly, with nothing left to ask. Two rules carry the
whole skill:

1. **Investigate before you ask.** A question the user has to answer that you could've answered yourself
   wastes their time and burns trust. Spike first; bring only the decisions a human must make.
2. **Never wall-of-text.** Ask through the **`ask_user_input_v0` tool** — a few tappable choices at a
   time, each with your recommended default — so answering is a tap, not an essay.

## Loop

1. **Intake** — restate the intent in one line: who wants what, why. If that's unclear, it's question one.
2. **Spike** — investigate to turn unknowns into facts and sharpen what's left (below).
3. **Map** — list every gap that would change *what gets built* or *when it's done*; blockers first.
4. **Clarify** — ask the top gap via the Q&A tool, fold in the answer, re-spike if it opens a new unknown,
   repeat. Stop only when no open question would change the implementation.
5. **Write** — produce the ticket in the format below.
6. **Confirm** — show it, check it against *Ready*, offer to save it or push it to the tracker.

## Spike before you ask

A spike is a quick investigation that buys down uncertainty — and you're equipped to do it:

- **Read the codebase** if present: what does this touch, how are similar features built, what pattern to
  follow, what constraints exist.
- **Check connected tools** (Jira, Linear, GitHub, Confluence, Slack, Drive) for the epic, linked tickets,
  prior art, design docs.
- **Look things up** — library/API behaviour, conventions, standards — with web search.
- **Mine the conversation**; never re-ask what's already been said.

> Before every question: **"Could I find this out myself?"** If yes, find it and state it as fact. If it's
> a real product / design / priority call, ask it — but attach what you found and **lead with a
> recommended default**, so the user confirms in one tap.

Good question: *"Existing endpoints paginate at 50/page — match that?"* with **Yes** recommended. Bad
question: *"How should pagination work?"*

## Asking well — the Q&A tool

- **One line of context, then options.** No essays.
- **One decision per call; three max**, batched only when tightly related.
- **2–4 short, mutually exclusive options** phrased as concrete answers, not abstract prompts.
- **Always include a default and an escape** ("Not sure — you recommend" / "Something else"); state your
  pick. Never force a false choice.
- **Question type:** `single_select` for either/or, `multi_select` for "which are in scope?",
  `rank_priorities` for trade-offs.
- **After calling the tool, stop** — the tap comes back as the next message.

## Where ambiguity hides

Walk these; most vague stories miss several. Skip rows that don't apply — chase the gaps that change the
build, not a checklist for its own sake.

| Dimension | Probe for |
|---|---|
| **Persona & value** | Who exactly, and the real "so that"? Worth building? |
| **Scope** | Smallest valuable slice (in); what people will wrongly assume is included (out) |
| **Rules** | Business rules, calculations, validation, defaults, limits |
| **Data** | Sources, required vs optional fields, persistence, migrating existing data |
| **States** | Every state; what's allowed in each; what triggers transitions |
| **Edge & errors** | Empty, zero, max, duplicate, concurrent, expired, unauthorized, downstream failure |
| **UX & copy** | Entry points, happy path, empty/loading/error states, exact user-facing messages |
| **Dependencies** | Tickets, services, teams, flags this blocks or is blocked by |
| **Testability** | How is each behaviour verified; what data/environment is needed |
| **Sizing** | Fits one sprint? If not, where does it split? |

## The ticket

Output exactly these sections. Keep it tight; nothing left to guess.

```
# <Concise, action-oriented title>

## User story
As a <specific persona>, I want <capability> so that <concrete benefit>.

## Acceptance criteria
Gherkin — one Scenario per behaviour, covering the happy path AND the edge/error cases you surfaced:

  Scenario: <name>
    Given <context>
    When <action>
    Then <observable outcome>

Use Scenario Outline + Examples for families of inputs that share an outcome.

## Non-functional requirements
Only those that apply: performance/scale, security & authz, privacy, accessibility, observability, i18n.

## Implementation notes
From your spike: files/components to touch, the existing pattern to follow, dependencies, gotchas, and
any "you decide" answers recorded as explicit decisions. Flag a split if it's too big for one sprint.
```

Acceptance criteria must be **grounded** in confirmed answers and spike findings — never invented to fill
space.

## Ready — the exit gate

Done only when all hold; if one fails, ask or spike again:

- Intent clear; story is **INVEST** (Independent, Negotiable, Valuable, Estimable, Small, Testable).
- Scope explicit both ways; Gherkin AC complete and testable, covering edge/error cases.
- Rules, data, and states specified; relevant NFRs and dependencies captured.
- Small enough for one sprint, or split.
- **No open question would change what gets built** — residual unknowns recorded as explicit decisions.

## Example

**Raw:** *"As a user, I want to reset my password."*

**Spike** finds an existing email magic-link flow and a transactional-email service — so you don't ask how
to deliver the link, you ask whether to reuse it (recommending yes), then link lifetime, single-use,
session invalidation, and the error cases — each a one-tap choice with a default.

**Resulting AC (excerpt):**

```gherkin
Scenario: Reset requested for a registered email
  Given a registered, verified email
  When I request a password reset
  Then a single-use link valid for 1 hour is emailed

Scenario: Reset requested for an unknown email
  Given an email with no account
  When I request a password reset
  Then I see the same neutral confirmation
  And no email is sent

Scenario: Expired or already-used link
  Given a reset link that is expired or already used
  When I open it
  Then I see an error and a prompt to request a new one
```

Investigate to kill the easy questions, ask the genuine calls as one-tap choices with defaults, land a
ticket with no gaps.
