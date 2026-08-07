# Class-merge precedence matrix

Two `---@class` declarations for one name — same file, another file, or
reached twice through inheritance — are merged by three separate seams:
`TypeEnv::absorb_block` (two blocks, one file), `TypeEnv::merge_file_types`
(two files), and `TypeEnv::collect_class` (the consume site, folding an
ancestor chain into one shape). Each seam decides, independently, per member
kind — `---@field`, a carrier-attached method, a `---@field [K] V` indexer,
a `---@operator` overload, a class's own type-parameter list, and member
visibility — who wins when two declarations disagree. That is on the order of
fifty independent decisions, and until this page none of them were written
down in one place: `docs/03-reference/02-limitations.md` states several by
prose, `CHANGELOG.md`'s Unreleased section documents several fixes, and the
rest lived only in code comments or nowhere.

Every cell below was **measured**, not reasoned about: a minimal Lua fixture
was built for it and run through `luabox check --format json` on two release
binaries — this worktree's current tree, and a binary built at
`git merge-base HEAD origin/develop` — and the actual diagnostics compared.
"Winner" is read off which of two candidate values (usually `number` vs.
`string`, or a `private` vs. `protected` visibility scope) the checker
actually enforced, by feeding the contended member to a parameter typed as
the *other* candidate and reading whether `LB0300` fires. A cell with no way
to construct two genuinely different candidates (nothing to disagree about)
is marked **N/A**; a cell where the checker's output cannot be attributed to
either candidate — it names neither — is marked **unobservable**.

The full fixture corpus (every Lua source file), raw JSON verdicts from both
binaries, and the current/develop diff live in the scratch report this page
was built from — `merge-matrix.md`, produced alongside this page but not
committed to the repository. Every cell in the tables below names its
fixture id so the corpus can be turned into table-driven tests directly.

## Arrival shapes

| Shape | Meaning | Seam it exercises |
|---|---|---|
| single | one declaration, no conflict | n/a — sanity baseline |
| dup-same-file | two `---@class Name` blocks, one file, conflicting bodies | `absorb_block` |
| dup-cross-file | two files each declare `---@class Name`, conflicting bodies | `merge_file_types` |
| unrelated-parents | `C : P1, P2`, two distinct classes both declare the key | `collect_class` |
| diamond-identical | `C : A, B`, both `: Base<X>` with the *same* `X` | `collect_class` |
| diamond-conflicting | `C : A, B`, both `: Base<X>` with *different* `X` | `collect_class` |
| bound-vs-bare | `Sub : Base<number>` vs. `Sub : Base` (parent named without args) | `collect_class` (parent-argument substitution) |

## Findings — read this first

**No undeclared current-vs-develop regression was found.** Every place the
two binaries disagree is `develop`'s diamond guard / parent-argument
substitution leaking a class's own type-parameter name (`V`, `T`) into the
diagnostic instead of resolving it — `current` fixes all of them, and every
one is covered by an existing `CHANGELOG.md` Unreleased entry ("A
`---@class Sub : Base<number>` binds its parent's type parameter", "A
generic `---@class`'s type parameters are scoped to the declaration that
writes them", "Indexer precedence now follows one measured rule per seam").
None of these are winner *reversals* — `develop` never produces an
observable winner for these cells; it names a type variable, not one of the
two candidate values.

Five cells, however, are genuine, measured, previously-unenumerated gaps —
identical in both binaries (not a regression), and not stated anywhere in
`CHANGELOG.md` or `docs/03-reference/02-limitations.md`:

1. **Same-file inheritance of a carrier-attached method through `: Parent`
   does not resolve at all.** `---@class C : P1` where `P1`'s carrier
   attaches `function T1:m()` in the *same file* reads `c.m` as `LB0306`
   undefined field — even though `m` visibly exists on `P1`. Move `P1` to
   its own file and the identical shape resolves cleanly. Field inheritance
   through the identical `: Parent` shape is unaffected — only carrier
   *methods* hit this. Root cause, read from the source rather than
   measured: `collect_class`'s ancestry walk only ever reads
   `self.classes[name].methods`, and a project source file's own classes
   only get that map populated by `FileTypes::collect`'s post-inference
   carrier fold — a step that runs to build this file's *exported* surface
   for other files, and is never written back into the same `TypeEnv`
   before this file's own obligations are checked. Cross-file consumption
   works because `merge_file_types` folds the already-collected map.
2. **A carrier-attached method's signature does not cross the same-project
   file boundary at all, even with zero duplication.** `f:m()` types as
   `unknown` when `Foo`'s carrier method lives in a different file than the
   read — one declaration, no conflict, still `unknown`. This contradicts
   `ClassDef.methods`'s own doc comment, which says these "resolve on reads
   and method calls exactly like `---@field` members" workspace-global.
3. **Operator diamond-conflict resolves first-visited-edge-wins;
   field/indexer diamond-conflict resolves last-visited-edge-wins — the
   identical arrival shape, opposite winners**, and the operator rule is
   not stated anywhere. Cause: operators accumulate as a list resolved by
   "first whose input accepts the operand" (#114) rather than overwriting
   by key, so the first-added signature always wins ties — an inevitable
   consequence of that data model, but never written down as a rule anyone
   could check code against.
4. **Visibility's unrelated-parents shape resolves last-listed-parent-wins —
   opposite of indexer/operator's first-listed rule for the identical
   shape** — via a third, independent mechanism (`walk_ancestor_names`
   pushes parents onto a stack in declaration order and pops LIFO, so the
   last-declared parent is visited, and can `Break` the search, first).
5. A bare (unbound) generic parameter leaks its literal name (e.g. `T`) into
   user-facing `LB0300` text on both binaries. Documented as "stays free"
   in `docs/03-reference/02-limitations.md`, but the exact wording — naming
   an internal type variable to the user — is not itself promised anywhere.

## The matrix

Legend: **first** = first declaration / first-listed parent / first-visited
ancestry edge wins. **last** = last declaration / last-listed parent /
last-visited edge wins. **scan** = every value is kept and the first one
whose shape matches at the use site wins (operators only). "Fixture id"
names the project in `merge-matrix.md` (and `fixtures*.json` next to it)
that this cell was measured with.

### `---@field`

| Arrival shape | Winner | Same in develop? | Fixture id |
|---|---|---|---|
| single | resolves (baseline) | yes | `field-A-single` |
| dup-same-file | **first** declaration (+ `LB0311` warning) | yes | `field-B-absorb-block-dup-same-file` |
| dup-cross-file | **first**-processed file | yes | `field-C-merge-file-types-dup-cross-file` (+ `-reversed`) |
| unrelated-parents | **last**-listed parent | yes | `field-D-unrelated-parents` (+ `-swapped`) |
| diamond-identical | resolves to the agreed value | **no** — develop leaks the raw type-parameter name instead of resolving (declared fix, see Findings) | `field-E-diamond-identical-binding` |
| diamond-conflicting | **last**-visited ancestry edge | **no** — develop unobservable (leaks raw name); current confirmed both directions | `field-F-diamond-conflicting-binding` (+ `-swapped`) |
| bound-vs-bare (bound half) | parent argument substitutes correctly | **no** — develop never binds it (declared fix) | `field-G-generic-bound-vs-bare` |
| bound-vs-bare (bare half) | stays free, but leaks the literal parameter name into diagnostic text | yes (both leak it) | `field-G-generic-bound-vs-bare` |

### Method (carrier-attached: `function Class:method()`)

| Arrival shape | Winner | Same in develop? | Fixture id |
|---|---|---|---|
| single | resolves (baseline) | yes | `method-A-single` |
| dup-same-file (two carriers, one class) | **first**-declared carrier, in statement order | yes | `method-B-two-carriers-same-file` |
| dup-cross-file | **unobservable** — neither candidate survives; both binaries type the call `unknown` (finding 2) | yes | `method-C-merge-file-types-cross-file` (+ `-reversed`), control: `method-inheritance-cross-file-control` |
| unrelated-parents | **N/A — inheritance itself is broken** (finding 1); `LB0306` regardless of order | yes | `method-D-unrelated-parents` (+ `-swapped`), repro: `method-inheritance-gap-same-file` |
| diamond-identical | **N/A**, same reason | yes | `method-E-diamond-identical` |
| diamond-conflicting | **N/A** — methods carry no type parameter to bind two ways; substitute test (declaration overriding an inherited attachment) confirms declaration wins, unremarkable | yes | `method-F-diamond-conflicting-via-field-override` |
| declaration-vs-attachment (same name, same class) | `---@field` declaration's type wins over the carrier attachment | yes | `method-G-field-declaration-beats-attachment` |

### Indexer (`---@field [K] V`)

| Arrival shape | Winner | Same in develop? | Fixture id |
|---|---|---|---|
| single | resolves (baseline) | yes | `indexer-A-single` |
| dup-same-file | **first** declaration, silently (no warning, unlike named fields) | yes | `indexer-B-absorb-block-dup-same-file` |
| dup-cross-file | **first**-processed file | yes | `indexer-C-merge-file-types-dup-cross-file` |
| unrelated-parents | **first**-listed parent | yes | `indexer-D-unrelated-parents` (+ `-swapped`) |
| diamond-identical | resolves to the agreed value | **no** — develop leaks raw name (declared fix) | `indexer-E-diamond-identical-binding` |
| diamond-conflicting | **last**-visited ancestry edge | **no** — develop unobservable; current confirmed both directions | `indexer-F-diamond-conflicting-binding` (+ `-swapped`) |
| bound-vs-bare (bound half) | substitutes correctly | **no** — develop never binds it (declared fix) | `indexer-G-generic-bound-vs-bare` |
| bound-vs-bare (bare half) | leaks literal parameter name | yes | `indexer-G-generic-bound-vs-bare` |

Note the **field/indexer asymmetry on unrelated-parents** (field: last-listed
wins; indexer: first-listed wins) is intentional and is the one precedence
axis `CHANGELOG.md`/`collect_class`'s own doc comment states explicitly.

### Operator (`---@operator op(...): R`)

| Arrival shape | Winner | Same in develop? | Fixture id |
|---|---|---|---|
| single | resolves (baseline) | yes | `operator-A-single` |
| dup-same-file | **first** overload matched by the "first input that accepts" scan (#114) | yes | `operator-B-absorb-block-dup-same-file` |
| dup-cross-file | **first**-processed file's overload, same scan mechanism | yes | `operator-C-merge-file-types-dup-cross-file` |
| unrelated-parents | **first**-listed parent | yes | `operator-D-unrelated-parents` (+ `-swapped`) |
| diamond-identical | resolves to the agreed value | **no** — develop leaks raw name (declared fix) | `operator-E-diamond-identical-binding` |
| diamond-conflicting | **first**-visited ancestry edge — **opposite of field/indexer's last-wins for the identical shape** (finding 3), confirmed both directions | **no** — develop unobservable (leaks raw name in both checks, not just one) | `operator-F-diamond-conflicting-binding` (+ `-swapped`) |
| bound-vs-bare | not separately fixtured; substitution follows the same `collect_operators` binding as fields/indexers | — | — |

### Type parameter (a class's own `<T, U, ...>` list)

| Arrival shape | Winner | Same in develop? | Fixture id |
|---|---|---|---|
| dup-same-file, both non-empty, differently spelled | positional unification — slot 0 is one type variable however each spells it | yes | `typeparam-B-absorb-block-renamed-same-file` |
| dup-same-file, second declaration's list empty | first (non-empty) declaration's list is canonical | yes | `typeparam-B-absorb-block-second-empty-same-file` |
| dup-cross-file, differently spelled | same positional unification, across files | yes | `typeparam-C-merge-file-types-renamed-cross-file` |
| bound-vs-bare (parent reference) | `: Base<number>` binds the parent's parameter | **no** — develop never binds it (declared fix) | `typeparam-G-parent-bound-vs-bare` |
| unrelated-parents / diamond | **N/A** — a class's own parameter list has no analogue across parents; nothing to disagree about | — | — |

### Visibility (`---@field private/protected/package`, standalone `---@private` etc.)

| Arrival shape | Winner | Same in develop? | Fixture id |
|---|---|---|---|
| single | enforces (baseline) | yes | `visibility-A-single-private` |
| dup-same-file, conflicting scopes | **first** declaration wins **atomically** with the field itself — the second `---@field` line is skipped whole, scope included, before its own scope is ever read | yes | `visibility-B-absorb-block-conflicting-scopes` |
| dup-cross-file, conflicting scopes | **first**-processed file | yes | `visibility-C-merge-file-types-conflicting-scopes` (+ `-reversed`) |
| unrelated-parents, conflicting scopes | **last**-listed parent — **opposite of indexer/operator's first-listed rule for the identical shape** (finding 4), via a third, unrelated mechanism (LIFO ancestor-name stack) | yes | `visibility-D-unrelated-parents-conflicting-scope` (+ `-swapped`) |
| diamond, same owner reached twice | unambiguous — both edges reach the identical declaration, nothing to arbitrate | yes | `visibility-E-diamond-identical-owner` |
| diamond-conflicting | **N/A** — visibility isn't parameterized by generic arguments, so a diamond cannot bind it two different ways | — | — |

## Cross-references

- `docs/03-reference/02-limitations.md` — "Duplicate `---@class` declarations
  union" states the field/indexer/type-parameter rules in prose; this page
  is its measured, tabular backing plus the method/operator/visibility rows
  it does not cover.
- `CHANGELOG.md` Unreleased — the declared fixes this page's "Same in
  develop?" column checks every disagreement against.
