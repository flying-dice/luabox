---
status: Accepted
date: 2026-08-07
---
# Decision 10 — Class-merge precedence is derived from lua-language-server's implementation, published as a matrix, and enforced by a verdict oracle

## Context

Merging two `---@class` declarations for one name happens at three seams — `TypeEnv::absorb_block` (two blocks, one file), `TypeEnv::merge_file_types` (two files), `TypeEnv::collect_class` (the consume-site ancestor fold) — and each decides independently, per member kind (`---@field`, carrier-attached method, indexer, `---@operator`, type-parameter list, visibility), who wins when declarations disagree. That is roughly fifty decisions.

None of them were written down. Each was implemented from local reasoning about the code in front of the author, and five review rounds on PR #61 each found a different cell wrong: fields keeping the first declaration while indexers kept the last, four of five member kinds getting a positional rename and one not, operators skipped at both seams, indexer precedence inverted between unrelated parents and diamonds. Individually defensible; mutually contradictory.

Two facts made this avoidable and were missed. First, luabox's stated goal is parity with lua-language-server — so for most cells an answer already existed. Second, luals ships its implementation as readable Lua (`script/vm/compiler.lua`, `doc.lua`, `field.lua`, `generic.lua`, `operator.lua`, `visible.lua`), and the pinned 3.13.5 distribution was already on disk for the parity gate. The semantics were derived from first principles while the reference implementation sat unread.

Deriving rules by black-box probing of our own binaries produced a matrix that was internally consistent and, in at least one cell, backwards versus luals: unrelated-parent field resolution is first-listed-wins upstream (`compiler.lua:369-375`, `:495-509`) and was last-listed-wins here, defended in the limitations doc by reasoning taken from our own code comments.

## Decision

1. **The reference implementation is the source of truth for any cell where luals has an opinion.** Read `script/vm/`, cite file and line. A rule inferred from behaviour is labelled as inferred; a rule read from source is preferred. Where luals is genuinely silent (unsupported construct, no ordering guarantee), luabox's rule is the specification and says so.
2. **The rules are published**, as `docs/03-reference/03-class-merge-precedence.md`: member kind × arrival shape, each cell's winner, the fixture that measures it, and whether luals agrees. Cells that cannot be observed are marked unobservable rather than guessed.
3. **One owner per member kind**, called by all three seams, so a cell cannot be decided twice. Where a kind legitimately has different rules per arrival shape, the owner takes the shape as a parameter.
4. **Tests are table-driven over the matrix** (`crates/luabox-types/tests/class_merge_precedence_matrix.rs`), rows being cells, so adding a member kind or a seam forces a row instead of leaving a silent gap.
5. **A verdict oracle is committed** (`scripts/tests/verdict-differential.sh`): a corpus with per-case expected diagnostic-code sets, so a behaviour change is a diff against a written claim in CI rather than something a reviewer discovers by building two binaries.
6. **A divergence from luals is a defect by default.** Keeping one requires an argument that survives the sentence "a user migrating from luals sees their working code behave differently", recorded in the parity gate's expectations.

## Consequences

Precedence questions are answerable by reading a table rather than the merge code, and a change that moves a verdict fails a gate in seconds instead of surviving to review. The cost is that adding a member kind or a seam now requires updating the matrix, the table-driven rows, and the oracle's expectations — deliberately, since that friction is the mechanism.

The audit this decision formalises found four drop-in regressions against luals on its first run. One (unrelated-parents inversion) was fixed in #61; the duplicate-declaration family — luals unions same-rank duplicates where luabox keeps the first — is issue #67. Per-code severity control, the escape hatch that makes any deliberate strictness increase survivable for a migrating user, is issue #66.

Generalisation worth stating: for any subsystem whose behaviour is a rule table rather than an algorithm, write the table before the code, and derive it from the reference implementation when one exists. This applies beyond the class merge.
