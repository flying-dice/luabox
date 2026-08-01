---
column: doing
labels: [release, review]
priority: high
agent: opus-w19
live: true
status: Wave 19 merged; all 4 threads replied+resolved; awaiting round 9
updatedAt: 2026-08-01T12:20:00.000Z
---
# Wave 19: Shockwave round-8 threads on PR #47

Round 8 @ 5fc18f5. Verdict FAIL, 4 open threads: 1 LB0510 bug (five sub-findings), 2 LSP bugs (B1 shutdown regression, B2 error-response-as-success), 1 test-tightening thread (Y). refs #47

Shockwave's meta-finding is the operating rule for this wave, verbatim: *"each fix is scoped to the reported instance... The matrix is the antidote — but only if new shapes go into it before the fix, not after the next review finds them."* Round 7 closed the reported LB0510 instances and left the mechanism, so round 8 found the same defect in new shapes.

## Scope

- LB0510 (a): `observed()` iterates receivers across every body with no reachability test — measured FP on `local function boom() return c:reset() end` + `print(type(boom))`, and on `if false then c:reset() end`
- LB0510 (b): statement-form constructor `local t = {}; setmetatable(t, C)` — 8th FN class, the idiomatic one; only the call's result was tracked
- LB0510 (c): `Or` seeds lhs ∪ rhs and `And` seeds rhs unconditionally, while doc and comment claim the evaluated operand is followed
- LB0510 (d): build an independent neighborhood sweep across the mechanisms touched; fix or disclose with fixtures. Addendum named two: global-module factory, and dot dispatch `c.reset(c)`
- LB0510 (e): the disclosed-bounds preface claims a fixture for all eight bounds; `require` has none and structurally cannot
- B1: `Connection::handle_shutdown` reads the channel directly; an `exit` drained into `Server::pending` is invisible to it → answers shutdown, times out, exits 1. Regression against the no-capability control
- B2: `response.id == *id` never inspects `response.error`; a refused token still gets 5 `$/progress`. Plus the wrong "the only request it sends" comment, and the undisclosed silent-client latency cost
- Y: the 60 ms window is a literal that a lower `PROGRESS_CREATE_TIMEOUT` would silently invalidate; the buffered-notification test asserts survival but not order

## Comments

- **claude** (2026-08-01T11:30:00.000Z): Worktree /home/user/wt-w19, branch sprint/w19-shockwave-round8 off develop@5fc18f5. Disk checked first per the wave-17 incident: 22G free at start. Baseline captured before touching anything: matrix 48/48 green on both columns against Lua 5.4.6.

- **claude** (2026-08-01T14:35:00.000Z): All four threads fixed across four commits, **fixtures-first by construction**. The commit sequence is the evidence Shockwave asked for: `c220593` adds 46 programs and 46 expected rows while the matrix is **RED** — 22 rows disagree with the shipped binary and every runtime column already matches `lua5.4` — and `753e7d5` is the change that turns them green. Nothing was written down after the fact.

  **Neighborhood sweep (d), run before the fix.** 45 programs in `target/scratch/sweep`, classified with `/usr/bin/lua5.4` as oracle, across reachability × derivation × operand-flow plus the addendum's two named classes. Measured before → after (lint findings; runtime never changes):

  | class | shapes | before | after |
  |---|---|---|---|
  | reachability FPs (uninvoked fn, dead `if false`/`if nil`/`while false`, escaping closure twin, two-deep twin, carrier-field-fn twin, colon-method twin, plain-table-method twin, `pcall` argument, returned-to-unknown) | 12 | 1 / ok | **0 / ok** |
  | reachability live twins (invoked fn, `if true`, two-deep, carrier field, colon method, plain-table method, `repeat`, `else` of dead `if`, immediate closure) | 9 | 1 / crash | 1 / crash (held) |
  | statement-form constructor (direct, factory, alias, expression form) | 4 + 3 twins | 0 / crash | **1 / crash** |
  | `x or ctor` FP (truthy lhs; and the reported dead-branch composite) | 2 | 1 / ok | **0 / ok** |
  | `ctor and x`, `x and ctor` with falsy guard | 3 | already right | held |
  | global-module factory (addendum 1) | 1 + twin | 0 / crash | **1 / crash** |
  | dot dispatch `c.reset(c)` (addendum 2) | 1 | 0 / crash | **1 / crash** |
  | dot **reads** (`type(c.reset)`, `local r = c.reset`, `type(c.__mode)`) | 3 | 0 / ok | 0 / ok (held) |
  | `__call` dead-branch `self:m()` — round 7's last disclosed FP | 1 | 1 / ok | **0 / ok** |
  | `__call` chain via instance alias — regression pin | 1 | 1 / crash | 1 / crash (held) |

  Four shapes are **not** fixed and are disclosed with fixtures instead: an escaping closure (0/crash — no call site names the body, the FN-biased edge of reachability); `x or ctor` with a falsy lhs (0/crash — the price of closing the truthy-lhs FP, since deciding it needs truthiness this pass does not have); a non-literal guard `local on = false; if on then c:reset() end` (1/ok — the prune reads literals only, the same bound the `__index`-write side has always carried); and a construction whose *metatable argument* is an alias of the carrier, `local mt = C; setmetatable({}, mt)` (0/crash — `class_carrier` is looked up on the unrooted binding, and rooting it would also root a reassigned `mt = {}`, which is an FP, so the trade goes the other way). Each has both verdicts and, where a twin exists, its twin.

  **753e7d5 — the mechanism, four parts.** REACHABILITY: uses are counted only in bodies the file *enters* — the chunk plus a fixpoint over the in-file call graph across the edges already tracked (named function, field of a named table, function expression at the call site, `__call` dispatch on a derived value, the colon method an instance call reaches) — and statement collection moved from the expression arena to a block walk that prunes literal-`false`/`nil`/`while false` blocks. Two edges the walk gets right and a flatter one would not: the `else` of a literal-false `if` is the branch that runs, and `repeat` tests after its body. STATEMENT-FORM: `setmetatable(t, C)` links `t` alias-rooted and on evaluation, which also feeds factory detection through the existing return-a-derived-local path. OR/AND: a construction is a table and therefore always truthy, which decides all four shapes — follow lhs of `or` and rhs of `and`, spelled out operand by operand in the doc. The `x and ctor` case was reasoned through with the oracle rather than assumed: a falsy `x` makes the name falsy and `c:reset()` still crashes, so seeding is sound on both sides of the guard (`metafield_and_rhs_ctor_falsy_guard`, 1/crash). DISPATCH: `fn_of_field` re-keyed from `BindingId` to `ValueRef` so a global module table resolves, and `c.m(c)` counts when `m` names a function attached to the carrier — a bare field read, a metafield read and an unknown key do not. Nine unit tests alongside so `cargo test` catches a regression without a release binary and an interpreter.

  **1e21cc8 — B1 and B2.** B1 took the *abort* option, not the re-implement-the-handshake option: `await_progress_create` returns the moment it drains a `shutdown` request or an `exit` notification, pushing it to `pending` first, so `main_loop` drains it into `Connection::handle_shutdown` unchanged — which then finds the `exit` on the channel where it looks for it. `Connection`'s contract is untouched. Confirmed the mechanism against the vendored `lsp-server-0.7.9`: `handle_shutdown` sends the response (line 353) and then does `recv_timeout(30s)` on the receiver, so an `exit` sitting in `pending` is a guaranteed `ProtocolError` → `run()` returns `Err` → exit 1, with the shutdown response already on the wire. Both of Shockwave's wire sequences are regression tests over `Connection::memory` — the transport it flagged analytically, where `exit` does not close the channel and the failure is a 30 s block rather than a fast error. They assert on *time to finish* via a completion channel, not on exit code alone, so they fail on the hang; verified failing against the reverted code. B2: the wait now reports Accepted / Refused / Unanswered, and Refused takes the `progress: false` path exactly (`create_progress_token` yields no token, so no begin, no report, no end); test drives a client that errors every create and asserts zero `$/progress`. The "only request it sends" comment is corrected and the `client/registerCapability` gap is stated rather than papered over — a client rejecting the watcher registration leaves this server believing file watching is live, and nothing detects it. Took the optional `Cell<bool>`: a create that *times out* stops later creates from waiting, halving the silent-client penalty; only a timeout sets it, since a client that answers has said something. Y: the 60 ms window is now `PROGRESS_CREATE_TIMEOUT / 4`, and the buffered-notification test queues two `didOpen`s and asserts publish order (fails against a queue drained from the back).

  **Gates on this head, exit codes direct.** df 22G free; `cargo fmt --all --check` 0; `cargo clippy --workspace --all-targets -- -D warnings` 0; `cargo test --workspace` 2439 passed / 0 failed across 59 binaries; control-flow differential 0; `lb0510-matrix.sh` **all 94 shapes match, lint AND runtime** against real Lua 5.4.6. Matrix grew 48 → 94 programs; disclosed-miss list 8 → 11 entries, and its preface no longer claims a fixture for `require`, which is cross-file by nature while the matrix runs one file at a time. No PR opened.

- **claude** (2026-08-01T12:20:00.000Z): Wave 19 merged to develop (0aa68de; card conflict resolved keeping the agent journal). Spot-checks green: fmt 0, clippy 0, lint 169 tests, acceptance 860/4678, lsp 194/1406, differential 112/112, lb0510-matrix 94/94 lint AND runtime (Lua 5.4.6). All 4 round-8 threads replied — the LB0510 reply leads with the fixtures-first evidence (c220593 RED with 22 disagreeing rows before 753e7d5 turns them green) — and resolved. Awaiting Shockwave round 9.
