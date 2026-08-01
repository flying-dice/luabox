---
column: done
labels: [release, review]
priority: high
agent: opus-w20
live: true
status: Done — PR #47 APPROVED (round 11) and merged as a63842d; issues #44/#33/#38/#39/#30 closed with evidence
updatedAt: 2026-08-01T15:05:00.000Z
---
# Wave 20: Shockwave round-9 threads on PR #47

Round 9 @ d32ffd8. Verdict FAIL, 3 open threads: 1 LB0510 bug (one-sided prune + a doc that overclaims), 1 LSP bug (B1, the second create window — filed NOT FIXED), 1 LSP issue (B2, the never-answers flag bypasses the refusal check). refs #47

Round 9's meta-finding is the operating rule for this wave: a fix scoped to one direction of a symmetric mechanism is half a fix, and a doc that describes the pass its author intended is not a doc. Round 8 pruned the `then` of a literal-false `if` and never the `else` of a literal-true one; round 8's B1 fix aborted one create window when `run` opens two.

## Scope

- LB0510 (a): branch pruning is one-sided — literal-true never prunes its `else`
- LB0510 (b): the ternary `cond and ctor or other` — `And` seeds its rhs unconditionally, `is_falsy_literal` never reaches the BinOp path
- LB0510 (c): remaining FP root causes — flow-insensitive derivation, two `setmetatable` on one table, dead numeric-for headers, after-return reachability
- LB0510 (d): undisclosed FNs — insert-last-wins fn maps, `local m = c.reset; m(c)`
- LB0510 (e): named twin/row gaps
- LB0510 (f): LIMITATIONS rewrite — ":312 one approximation remains" has overclaimed four rounds running
- B1: `run` opens TWO create windows before `main_loop`; window 1 aborts and leaves `exit` on the channel, window 2 drains it into `pending`, `handle_shutdown` reads the raw channel → exit 1, byte-identical to pre-fix. The round-8 fixture skipped `bootstrap`
- B2: the never-answers flag skips the wait on create #2, so an error response to #2 lands in `main_loop`'s discard arm → `$/progress` under a refused token

## Comments

- **claude** (2026-08-01T15:10:00.000Z): Worktree /home/user/wt-w20, branch sprint/w20-shockwave-round9 off develop@d32ffd8. `df -h /` first: 22G free, no stale worktree targets over 5G (wt-repodoc 578M, left alone). Baseline captured before touching anything: matrix **94/94 green on both columns** against Lua 5.4.6, 94 fixture programs.

- **claude** (2026-08-01T18:05:00.000Z): All three threads fixed across four commits, fixtures-first for both halves.

  **Commit sequence (the evidence Shockwave asked for, repeated exactly).** `7b3689e` adds 25 matrix programs and 25 expected rows while the matrix is **RED** — **9 rows disagree** with the shipped binary, and every runtime column already matches `lua5.4`; `c16e3e7` is the change that turns them green. Likewise `25f0dc7` adds 6 LSP sequence tests of which **5 are RED** against HEAD; `f4d3729` turns them green. Nothing was written down after the fact.

  ### Thread 1 — LB0510

  Runtime ground truth for every new shape, taken from `/usr/bin/lua5.4` before anything was changed. Nine lint verdicts moved:

  | shape | before | after | runtime |
  |---|---|---|---|
  | `if true then … else c:reset() end` | 1 | **0** | ok |
  | `if true then … elseif true then c:reset() end` | 1 | **0** | ok |
  | `for _ = 1, 0 do c:reset() end` | 1 | **0** | ok |
  | `for _ = 1, 10, -1 do c:reset() end` | 1 | **0** | ok |
  | `do return end` then `c:reset()` | 1 | **0** | ok |
  | `false and ctor or other` | 1 | **0** | ok |
  | `false and ctor` (guarded use) | 1 | **0** | ok |
  | `false or ctor` | 0 | **1** | crash |
  | `local m = c.reset; m(c)` | 0 | **1** | crash |

  Sixteen more rows are twins and controls that had to *hold*: the live half of every pruned shape (`for _ = 1, 1`, `if false … else`, the call before `do return end`, `true and ctor or other`), and the FPs/FNs that stay.

  **(a) symmetric pruning.** `walk_block`'s `if` arm walks branches in order and stops at the first literal-truthy one — later `elseif` *conditions* included, since Lua does not test them. Both halves come out of the same walk, so both feed the fixpoint and the use count by construction. Two more shapes were the same literal question and went in with it: `numeric_for_runs` (literal start/end/step, both directions, `step == 0` deliberately undecided because Lua errors on the header and a program that never gets that far is not dead code) and `terminates` (`Return`, `Break`, and a `Do` whose block ends in one — `do return end` is the only early return Lua's grammar accepts, which is why it recurses). `goto` is not pruned: it can jump forward to a label in the same block.

  **(b) the ternary.** New `truthiness(body, expr) -> Option<bool>`: literals decided (`nil`/`false` falsy, everything else truthy), folding through nested `and`/`or` and `Truncate`. The fold is the whole point — `cond and ctor or other` parses as `(cond and ctor) or other`, so without folding the `or` sees a `Binary` left operand it cannot read and follows it straight back into the construction. `And` with a literal-falsy lhs returns the lhs; `Or` with a literal-falsy lhs returns the rhs. Undecided operands keep the round-8 reading verbatim (six probes re-run: `ctor or x`, `x or ctor`, `x and ctor`, `ctor and x`, `and_chain`, `or_fallback` — all held). The named-literal one hop (`local cond = false` then `cond and …`) is **disclosed, not fixed**: it needs constant propagation, and a partial one that decided `and`/`or` but not `if` would have the two disagreeing about the same program. Non-literal cond in a ternary: reasoned with the oracle and **kept seeding** — following the lhs of `or` into an `and` whose truthy value *is* the construction is a genuine may-analysis, and it is the same trade `x and ctor` already takes. Both directions fixtured (`metafield_ternary_named_true_cond` 1/crash, `metafield_ternary_named_false_cond` 1/ok, disclosed).

  **(c)/(d) fixed or disclosed, each with a twin.** Fixed: dead numeric-for headers, after-return, and `local m = c.reset; m(c)` (`dot_dispatched` became `dot_read`, answering the *read*; `seed_dot_handles` binds the answer to a name; both call sites ask it. `local m = Cache.reset; m(c)` is correctly silent — the base is not derived). Disclosed with twins: flow-insensitive derivation (`metafield_flow_reassigned_before_use` 1/ok vs `_after_use` 1/crash); two `setmetatable` calls on one table — measured with the oracle, and the FP is exactly the superseded first site, one-finding-per-site stays right (`_second_wins` 1/ok vs `_second_bare` 1/crash); insert-last-wins fn maps in **both** directions (`metafield_fn_replaced_after_call` 0/crash FN, `_before_call` 1/ok FP).

  **(f) LIMITATIONS.** ":312 one approximation remains" is gone. In its place: an enumerated two-direction bounds section — four FP classes and twelve FN classes, each naming its fixture — plus a table at :223 giving the full branch story (five rows: literal-false `if`, literal-true `if`, `while false`, dead numeric-for headers, after a terminating statement) with the non-prunes named (`repeat`, zero step, `goto`). Written against the code first, then the code re-read against it: `walk_block`, `numeric_for_runs`, `terminates`, `truthiness` and the `BinOp` arm were each checked back line by line, and the fixture-count claims were verified by listing the matrix directory rather than by memory. The rule's own rustdoc gets the same treatment — a `# What this does not decide` section replacing "with the one approximation that remains".

  ### Thread 2 — B1, the second create window

  Reproduced first, through `run()` itself. `crates/luabox-lsp/tests/shutdown_windows.rs` drives the production sequence — real handshake, real `Server::new`, real `bootstrap`, real loop — and measures time-to-finish on a completion channel so a 30 s block fails as a *hang* rather than as a slow pass. Against HEAD the filed repro produced exactly the filed behaviour: window 1 aborted, `bootstrap` opened window 2, emitted `begin` + 3 × `report` + `end` under `luabox/bootstrap-1`, answered the shutdown, and then never returned.

  **Design, stated.** Minimal set is (i) + (ii); (iii) is belt-and-braces and was kept because one sequence needs it.

  - (i) `session_is_ending()` — sticky `shutting_down` flag OR a scan of `pending` — at the head of `await_progress_create`, returning without touching the channel;
  - (ii) the same guard at the head of `create_progress_token`, which returns `None` **before sending anything**. New `CreateOutcome::Ending` separates "the session is ending" from "the client is slow", so the abort path returns no token: a shutting-down client gets no `begin`/`report`/`end` and no further create. This is Shockwave's fold-in;
  - (iii) `shutdown_handshake` replaces `Connection::handle_shutdown`. Two things it does that cannot be wrapped from outside: it looks in `pending` before the channel, and it **answers** a repeated `shutdown` rather than failing the session. The second is load-bearing — `shutdown,shutdown,exit` passed at HEAD only *by accident* (window 2 happened to swallow the second shutdown, leaving it unanswered), and with (i)+(ii) removing window 2 it would have hit `lsp-server`'s "unexpected message during shutdown" and become exit 1. `SHUTDOWN_EXIT_TIMEOUT` is 30 s, matching what it replaced.

  **All five sequences, measured:**

  | sequence | HEAD | after |
  |---|---|---|
  | shutdown+exit inside window 1 (filed repro) | hang → exit 1 | **clean**, 0 progress under bootstrap's token |
  | shutdown, shutdown, exit | shutdown #2 never answered | **clean**, both answered |
  | exit after the create response (lands in window 2) | begin/3×report/end to a leaving client | **clean**, silent |
  | single reload | clean | **clean** (the control) |
  | two queued reloads, shutdown in the first window | hang → exit 1 | **clean** |

  ### Thread 3 — B2

  Took shape **(a)**: once a create goes unanswered, none is *sent* for the rest of the session. Reason stated in the code: (b) keeps sending and polls for a late answer, which leaves the window open and needs a second mechanism to consult `pending` mid-flight; (a) removes the token entirely, so there is nothing to report under and no traffic at all to a client that is not listening. It keeps the latency win (no wait) and kills the class rather than narrowing it. Semantics disclosed rather than hidden, in the field doc and in the CHANGELOG: **one timeout mutes progress for the session**, and that client was already receiving `$/progress` under tokens it never acknowledged. The falsified comment ("Only a *timeout* sets this … the next create is waited for normally") is rewritten.

  Test: ignore create #1, refuse anything that follows, assert zero `$/progress` under any refused token — and, for the chosen semantics, exactly one create for the whole session. Against HEAD it failed with `$/progress` under `luabox/bootstrap-1`, which the client had refused; the filed defect verbatim.

  **Harness fallout, stated because it is behaviour.** Four unit tests rode a client that never answered a create, which under (a) means no second create exists to assert about. They now use `AnsweringClient` — `Server::new` → `bootstrap` → `main_loop` on a worker thread with the client here — which also makes `consecutive_reloads_do_not_reuse_a_progress_id_or_token` (unique ids, do-not-touch) go through the real loop instead of `handle_notification` directly. `tests/server.rs`'s `recv` answers creates as it reads them, which is what an editor does.

  ### Gates on this head, exit codes direct

  `df -h /` 22G free; `cargo fmt --all --check` **0**; `cargo clippy --workspace --all-targets -- -D warnings` **0**; `cargo test --workspace` **0**, 63 binaries all ok (luabox-lint 174, luabox-lsp lib 308 + server 133 + shutdown_windows 6 + pinned_stack 1); release acceptance **860 scenarios / 4678 steps**, lsp_acceptance **194 / 1406**; `control-flow-differential.sh` **112/112 cells agree with luac**; `lb0510-matrix.sh` **all 119 shapes match, lint AND runtime** (Lua 5.4.6), from 9 red rows at `7b3689e`. Matrix grew 94 → 119 programs. `target/scratch` removed. Pushed to origin; **no PR opened**.

  ### Not verifiable locally

  - the differential skips Lua 5.2 and 5.3 (not installed here); CI's job installs 5.1–5.4 and runs both columns of the matrix and all four dialect rows
  - `exception_goto_into_local_scope` remains the documented accepts-where-luac-rejects cell, unchanged by this wave
  - the `client/registerCapability` gap round 8 stated is still open and still undetected — out of scope for round 9, and named here so it is not mistaken for closed

- **claude** (2026-08-01T13:45:00.000Z): Wave 20 merged to develop (8a41b38; card conflict resolved keeping the agent journal). Spot-checks green: fmt 0, clippy 0, lint 174 tests, lsp 308+133 unit/integration, acceptance 860/4678, lsp_acceptance 194/1406, differential 112/112, lb0510-matrix 119/119 lint AND runtime (Lua 5.4.6). All 3 round-9 threads replied (symmetric pruning + truthiness() + honest two-direction bounds doc; B1 guard+stickiness+own shutdown_handshake with the accidental-pass insight; B2 option (a) no-create-after-timeout) and resolved. Both halves fixtures/tests-first: matrix RED 9, LSP tests RED 5. Awaiting Shockwave round 10.

- **claude** (2026-08-01T14:45:00.000Z): Round 10 @ 9f72beb: all 3 carried threads CLOSED on measurement (Shockwave: "the strongest yet"... "Close these three and I expect to approve"). Its final ~120-execution sweep found 3 defects the PR introduced, all small, fixed DIRECTLY on develop at 580ec8b per the small-item rule: (1) differential.yml paths widened to crates/luabox-lint/** in both lists (the LB0510 runtime gate could be skipped by editing facts.rs — the file this PR changed); (2) check --watch now reruns on versioned rock-tree changes via new layout::is_rock_source (the #30 harvest made the old blanket lua_modules/ exclusion rationale false), unit-pinned both directions; (3) the bundle loader gate carries "at line N" from the finding's own span, pinned at the library boundary. Spot-checks: bundle 69 tests, manifest 125, cli lib, acceptance 860/4678 all green; fmt 0; clippy 0. The 5 pre-existing raise-don't-block observations filed as #50 (global ---@class carrier members), #51 (project-walk symlink guard), #52 (gitlab format path/fingerprint), #53 (lint --format missing), #54 (require-binding hover). All 3 threads replied and resolved. Awaiting round 11 — expected approval.
