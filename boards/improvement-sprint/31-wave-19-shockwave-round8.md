---
column: doing
labels: [release, review]
priority: high
agent: opus-w19
live: true
status: Wave 19 launched: LB0510 reachability mechanism + shutdown regression
updatedAt: 2026-08-01T12:15:00.000Z
---
# Wave 19: Shockwave round-8 threads on PR #47

Round 8 @ 5fc18f5: 2 of 4 round-7 threads CLOSED — supplies() held 11/11 and the __call chain 4/4 under deliberate attack ("the first time a change to this rule has done so"); the matrix is "a standing convention, not a one-off". Verdict FAIL, 4 open: 3 bugs, 1 issue. refs #47

Shockwave's meta-point, now the operating rule: fixes have been scoped to the demonstrated repros while the mechanism survives one step away. This wave: new failing fixtures go into the matrix FIRST, then the fix.

## Scope

- LB0510 mechanism (bug): observed() counts receivers in bodies that are never reached — `local function boom() return c:reset() end; print(type(boom))` fires/ok vs `print(boom())` fires/crash. Fix = in-file body reachability (root body + transitively called bodies via fn_of_local/fn_of_field/__call chain; escaping closures = unreached, FN-biased). Statement-form constructor FN: `setmetatable(t, Cache)` never links t — seed the first argument; factory via statement form inside Cache.new() follows. Or/and over-seeding FP: union seeds the untaken operand — seed only operands that definitively flow to the result (a construction is always truthy: `ctor or x` = ctor, `x or ctor` ambiguous → FN-bias), plus literal-false branch pruning for receiver collection. Doc bookkeeping: eight disclosed bounds, seven fixtures — reword the preface (require is cross-file, structurally unfixturable)
- B1 (bug, regression): shutdown+exit drained into pending during the create wait → handle_shutdown never sees exit → exit 1 (VS Code/Neovim show abnormal termination). Drain pending into the handshake or handle shutdown/exit in main_loop directly; the two wire sequences are the regression tests; the memory-transport 30s-block variant flagged analytic
- B2 (bug): create response matched on id only — response.error accepted as success, 5 $/progress under a refused token. Treat error as refused (no token); fix the "only request it sends" comment (client/registerCapability); disclose the 2x250ms silent-client startup cost in CHANGELOG

## Comments

- **claude** (2026-08-01T12:15:00.000Z): Worktree /home/user/wt-w19, branch sprint/w19-shockwave-round8 off develop@5fc18f5. Disk pre-cleaned (22G free). Opus agent launched with the round-8 review + B1/B2 inline threads, under the fixtures-first rule.
