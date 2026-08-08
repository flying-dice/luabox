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

Five cells, however, were genuine, measured, previously-unenumerated gaps —
identical in both binaries (not a regression), and not stated anywhere in
`CHANGELOG.md` or `docs/03-reference/02-limitations.md`. Findings 1 through 4
below are now **fixed**: this page's own contradictions, once found, are not
left standing — the seams they named have been changed to agree with the
majority rule for their arrival shape (or, for 1 and 2, to actually do the
write-back their own doc comments already promised), and the matrix tables
further down reflect the new, consistent verdict, not the one that was
measured.

Finding 6, added later (production-readiness-assessment-9natxz A1), is a
different kind of check than 1-5: those five were only ever compared against
`develop`, luabox's own prior binary — a self-consistency check, not a
luals-parity one. Finding 6 was found by reading lua-language-server 3.13.5's
own source and measuring the pinned binary directly, and it overturns a
"current vs develop agree" cell rather than a "current vs develop disagree"
one — both binaries resolved field/method's unrelated-parents shape
last-listed, and both were wrong against luals. Read it as a correction
against the reference implementation, not against this repository's own
history.

1. **FIXED. Same-file inheritance of a carrier-attached method through
   `: Parent` used to not resolve at all.** `---@class C : P1` where `P1`'s
   carrier attaches `function T1:m()` in the *same file* read `c.m` as
   `LB0306` undefined field — even though `m` visibly existed on `P1`.
   Moving `P1` to its own file made the identical shape resolve cleanly.
   Field inheritance through the identical `: Parent` shape was unaffected —
   only carrier *methods* hit this. Root cause: `collect_class`'s ancestry
   walk only ever reads `self.classes[name].methods`, and a project source
   file's own classes only got that map populated by `FileTypes::collect`'s
   post-inference carrier fold — a step that runs to build this file's
   *exported* surface for other files. It reads `env.classes` (already
   correctly seeded for a checked file, since the CLI batch path folds every
   project file's surface, itself included, into the ambient before
   checking any file — `check_cmd.rs`'s `run_passes`), but `absorb_block`'s
   handling of a first-time local `---@class` declaration threw the seeded
   `ClassDef` away wholesale and replaced it with an empty one, discarding
   the `methods` map that seeded value already carried. Nothing else in
   `absorb_block` (or anywhere before this file's own obligations are
   checked) ever repopulates it, so the loss was permanent for the rest of
   that file's own check. `absorb_block` now carries the pre-existing
   entry's `methods` forward across that overwrite instead of discarding it
   — the one axis safe to preserve, since nothing else in `absorb_block`
   ever writes to `methods` in the first place (every other field — parents,
   fields, indexers, operators, visibility — is correctly re-derived from
   scratch in the same pass, so resetting those was never the bug).
   Cross-file consumption was already correct because a class this file does
   *not* declare is left exactly as the ambient seeded it — only the
   locally-declared-class overwrite discarded anything.
2. **FIXED. A carrier-attached method's signature used to not cross the
   same-project file boundary at all, even with zero duplication.** `f:m()`
   typed as `unknown` when `Foo`'s carrier method lived in a different file
   than the read — one declaration, no conflict, still `unknown` — which
   contradicted `ClassDef.methods`'s own doc comment, promising these
   "resolve on reads and method calls exactly like `---@field` members"
   workspace-global. Root cause: reifying an unannotated function's return
   type stamps `FunctionTy::has_return_annotation` with
   `returns_set && self.mode.seeds_params()` (`infer::reify::reify_func`) —
   `false` in `InferMode::Check`, the mode both the checker and
   `module_surface_from_env`'s surface pass run in. That flag conflates two
   different questions: whether an unannotated *parameter* was seeded from
   call-site argument types (a genuine guess, rightly `Display`-mode-only
   per SPEC §19) and whether a function's *return* type is known at all — a
   plain deduction from its own `return` statements, no guessing about other
   call sites involved, and the identical deduction a same-file `f:m()` call
   already rests a diagnostic on via the live (not-yet-reified) inference
   path. Reusing the parameter-seeding flag to also gate the return type
   erased a carrier method's return type the moment it was published into a
   class's `methods` surface — the one seam `ClassDef.methods` explicitly
   promises behaves like a `---@field`, and unlike `---@field`, whose type
   is annotation-derived and never touches this flag. `FileTypes::collect`
   now promotes `has_return_annotation` to `true` when folding a carrier
   method into `def.methods` and its return type was actually inferred
   (`returns` non-empty) — narrowly, at the one seam the doc comment's
   promise concerns, not in `reify_func` generally: a `require`'d free
   function's unannotated return type still does not seed a consumer's
   diagnostics (module exports, inlay display, and
   `check::conformance`'s same-file `carrier_class_final` fallback all read
   through the un-promoted value, untouched), matching #56's "annotations
   are authoritative" rule for that seam. Only a class's own carrier-attached
   *methods* — declarations, not call-site guesses — cross the boundary with
   their inferred return type now.
3. **FIXED. Operator diamond-conflict used to resolve first-visited-edge-wins;
   field/indexer diamond-conflict resolves last-visited-edge-wins — the
   identical arrival shape, opposite winners** — and the operator rule was
   not stated anywhere. Cause: operators accumulate as a list resolved by
   "first whose input accepts the operand" (#114) rather than overwriting
   by key, so the first-added signature always won ties — an artifact of
   that data model, not a decision anyone had written down or could check
   code against. `TypeEnv::collect_operators` now tags each accumulated
   overload with its owning ancestor name and, on a genuinely competing
   repeat visit of that ancestor (a diamond conflict — the same
   `DiamondGuard::visit`-provided `is_first_binding: false` signal
   `collect_class`'s field/indexer merge already reads), removes that
   ancestor's earlier entries before appending the fresh ones. The
   first-match scan is unaffected — it still finds the first entry whose
   input accepts the operand — but for a diamond-conflicting ancestor there
   is now only ever one entry, the most-recently-visited edge's, so the
   scan cannot land on a superseded one. Operator diamond-conflict now
   resolves **last-visited-edge-wins**, matching field and indexer for the
   identical shape. The *other* operator rows this page measured
   (dup-same-file, dup-cross-file, unrelated-parents, diamond-identical) are
   untouched: none of them puts two entries for the same ancestor in the
   list, so the new supersede logic never triggers for them.
4. **FIXED. Visibility's unrelated-parents shape used to resolve
   last-listed-parent-wins — opposite of indexer/operator's first-listed rule
   for the identical shape** — via a third, independent mechanism
   (`walk_ancestor_names` pushed parents onto a stack in declaration order
   and popped LIFO, so the last-declared parent was visited, and could
   `Break` the search, first). `walk_ancestor_names` now pushes each class's
   parents in *reverse* declared order, so the LIFO pop yields the
   first-declared parent first — ordinary left-to-right preorder traversal.
   Visibility's unrelated-parents shape now resolves **first-listed-parent-
   wins**, matching indexer and operator for the identical shape. At the
   time this finding was fixed, field's own unrelated-parents rule was
   believed to be a deliberate last-listed exception to this same-shape
   agreement — it was not; see finding 6, which found that belief backwards
   against luals and brought field (and method) onto the same first-listed
   rule this finding already gave visibility.
   `walk_ancestor_names`'s other two consumers (`class_method_names`,
   `is_subclass`) are order-independent and unaffected.
5. **FIXED. A bare (unbound) generic parameter used to leak its literal name
   (e.g. `T`) into user-facing `LB0300` text on both binaries.** Still
   documented as "stays free" in `docs/03-reference/02-limitations.md` — that
   resolution rule is correct and unchanged — but the exact *wording*, naming
   an internal type variable to the reader as if it meant something, was
   never itself promised anywhere, and is not something a reader can act on:
   `T` is not a value a Lua reference can name or produce. This is the
   identical shape the #56 export-seam fix (`CHANGELOG.md` Unreleased,
   "A generic `---@class` carrier no longer exports its unbound type
   parameter") already solved at the `require` boundary —
   `TypeEnv::class_shape_bound_export`'s existing erase-to-`unknown`
   substitution is reused, not reinvented, at every same-file
   reference-consuming site that can put a member's type in front of a
   reader: `crate::infer::Infer::lookup_shape_field`/`lookup_ty_field`'s
   field reads, an `ipairs`/indexer element read, `check::conformance`'s
   `: Parent` obligation, and `Checker::field_shape`'s table-literal
   obligation. `Checker::table_shape`/`check::conformance`'s own
   `class_shape_bound`/`class_shape_bound_export` split keeps each
   obligation's *presence* gate (`field.optional`/`field.ty.admits_nil()`)
   reading the raw, non-erasing resolution — erasing the gate itself, not
   just the displayed text, was measured to silently turn a genuinely
   missing required member into a "no obligation" (`unknown` admits `nil`)
   and drop the diagnostic outright, a regression worse than the leak it
   replaced. Only the *text* is substituted; the verdict — whether a
   diagnostic fires at all — is unchanged. `Checker::class_shape`/
   `class_shape_bound` (ambient/LSP template display, e.g. hovering
   `Box<T>`'s own declaration) and `TypeEnv::resolve_named`'s raw baseline
   (the `require`-seam's own `erased != resolved` comparison) are
   deliberately untouched — they answer a different question ("what does
   this class's template look like") than a value reference does.
6. **FIXED. Field and method's unrelated-parents shape resolved
   last-listed-parent-wins — backwards against lua-language-server, which
   this whole page exists to be a drop-in for** (production-readiness-
   assessment-9natxz A1). Unlike findings 1-5, which were measured only
   against luabox's own prior binary (`develop`), this one was checked
   against luals 3.13.5 itself, by reading `script/vm/compiler.lua:369-509`
   and then confirming the reading by measurement: `---@class C : P1, P2`
   where both parents declare the same member resolves it to **P1's**
   type in luals, and swapping the parent list to `C : P2, P1` flips the
   winner to P2's — first-listed, not last-listed, and this holds
   identically for a plain `---@field` and for a carrier-attached method
   (the method/carrier lookup runs inside the same `searchClass` recursion
   the `extends` walk drives, so it inherits the identical gate). luabox
   previously resolved last-listed for both, and `docs/03-reference/
   02-limitations.md` defended that as an "intentional asymmetry" against
   indexer/operator/visibility's first-listed rule for the identical
   shape — reasoning that traced back to luabox's own code comments, never
   to luals. It was wrong: luals has exactly one rule here, first-listed,
   and applies it uniformly across every member kind that inherits at
   all. Field and method now resolve first-listed too, closing the
   asymmetry findings 3 and 4 already narrowed to "field is the one
   deliberate exception" — field was never a deliberate exception, it was
   the one cell nobody had checked against the actual reference
   implementation. This is a **verdict-changing** fix: a project with
   `---@class C : P1, P2` where P1 and P2 disagree on a shared member's
   type, previously clean under `luabox check` reading P2's type, now
   resolves to P1's — matching what `lua-language-server --check` already
   told that project. See `CHANGELOG.md` for the user-facing statement.

**A further, previously unfixtured asymmetry is also fixed by this change:**
`push_operator_overload` deduped a byte-identical `---@operator` repeat when
called from `merge_file_types` (cross-file) but not from `absorb_block`
(same-file) — the same-file seam could carry two identical copies of one
overload where the cross-file seam collapsed them to one, an inconsistency
of the same kind as findings 3 and 4 even though it never showed up as a
different *winner* (a repeat identical to the winning entry cannot change
which signature the first-match scan finds). Both seams now dedupe
identically, matching every other member kind's "the file boundary does not
change the merge" rule
(`docs/03-reference/02-limitations.md`).

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
| unrelated-parents | **first**-listed parent (finding 6, fixed: was last-listed — see Findings) | **no** — this rule is `current`-only, and is a correction against luals rather than against `develop`: `develop` also resolved last-listed, matching luals 3.13.5's own `compiler.lua:369-375`/`424` (confirmed by direct measurement against the pinned binary) is what changed, not this page's earlier `develop` comparison | `field-D-unrelated-parents` (+ `-swapped`) |
| diamond-identical | resolves to the agreed value | **no** — develop leaks the raw type-parameter name instead of resolving (declared fix, see Findings) | `field-E-diamond-identical-binding` |
| diamond-conflicting | **last**-visited ancestry edge | **no** — develop unobservable (leaks raw name); current confirmed both directions | `field-F-diamond-conflicting-binding` (+ `-swapped`) |
| bound-vs-bare (bound half) | parent argument substitutes correctly | **no** — develop never binds it (declared fix) | `field-G-generic-bound-vs-bare` |
| bound-vs-bare (bare half) | stays free; reads `unknown` in diagnostic text (finding 5, fixed: used to leak the literal parameter name) | **no** — develop still leaks the raw name, this rule is `current`-only | `field-G-generic-bound-vs-bare` |

### Method (carrier-attached: `function Class:method()`)

| Arrival shape | Winner | Same in develop? | Fixture id |
|---|---|---|---|
| single | resolves (baseline) | yes | `method-A-single` |
| dup-same-file (two carriers, one class) | **first**-declared carrier, in statement order | yes | `method-B-two-carriers-same-file` |
| dup-cross-file | **first**-processed file, matching every other member kind's dup-cross-file rule (finding 2, fixed: was unobservable — both binaries used to type the call `unknown`) | **no** — develop still unobservable, this rule is `current`-only | `method-C-merge-file-types-cross-file` (+ `-reversed`), zero-dup repro: `method-cross-file-signature-gap` (+ field control) |
| unrelated-parents | **first**-listed parent, matching field-D (finding 1 fixed the N/A gap — `LB0306` regardless of order; finding 6 then flipped the winner from last-listed to first-listed, same root cause and same fix as field-D) | **no** — this rule is `current`-only for the same reason as field-D: luals resolves the first-listed parent (compiler.lua:369-375, and the method/carrier lookup runs inside the identical `searchClass` recursion the `extends` walk drives, so it is subject to the same gate — see Findings) | `method-D-unrelated-parents` (+ `-swapped`), repro: `method-inheritance-gap-same-file` (+ cross-file control) |
| diamond-identical | resolves to the agreed value (finding 1, fixed: was N/A, same reason) | **no** — develop still N/A | `method-E-diamond-identical` |
| diamond-conflicting | **N/A** — methods carry no type parameter to bind two ways. The substitute test this row used to pin (declaration overriding an inherited attachment) was retired: that shape is not actually a diamond-conflict at all — it is two *unrelated* parents (one inheriting a carrier attachment, the other declaring its own field), so finding 6 governs it, and it now resolves first-listed-wins like every other unrelated-parents cell, not "declaration always wins" | yes | `method-F-diamond-conflicting-unrelated-ancestor-declaration` |
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
| bound-vs-bare (bare half) | reads `unknown` (finding 5, fixed: used to leak the literal parameter name) | **no** — develop still leaks the raw name, this rule is `current`-only | `indexer-G-generic-bound-vs-bare` |

**The field/indexer asymmetry on unrelated-parents that used to be noted
here was a bug, not a deliberate design axis — it is gone.** Prior editions
of this page described field's unrelated-parents rule as last-listed-wins,
called it "the one precedence axis `CHANGELOG.md`/`collect_class`'s own doc
comment states explicitly," and reasoned it was intentionally the mirror of
indexer/operator/visibility's first-listed rule. That reasoning was built
entirely from luabox's own code comments and never checked against
lua-language-server's actual behaviour. It was checked
(production-readiness-assessment-9natxz A1, source-read against luals
3.13.5's `script/vm/compiler.lua:369-375`/`424` and confirmed by direct
measurement: swapping a class's parent order flips which parent luals
enforces, and the first-listed one always wins) and found backwards: luals
has exactly one rule for "two unrelated parents disagree on a member,"
first-listed-wins, applying identically to fields, carrier methods, and
indexers. Field (finding 6) and method (the same finding, same root cause)
are now first-listed-wins too, so **field, method, indexer, and visibility
all agree on unrelated-parents: first-listed wins**, with no remaining
exception among the member kinds luals actually resolves this way. Operator
(below) resolves unrelated-parents first-listed too, and stays consistent
with the rest of this list on this axis — but that agreement is luabox's own
internal-consistency choice, not luals parity: luals does not inherit
`---@operator` through `: Parent` at all, so there is no luals rule for
operator's unrelated-parents to match in the first place. See the operator
table below.

### Operator (`---@operator op(...): R`)

**Inherited operators are a luabox extension beyond lua-language-server
3.13.5, not a parity claim.** Earlier editions of this page and table
described the four inheritance-shaped rows below (unrelated-parents,
diamond-identical, diamond-conflicting, bound-vs-bare) as "matching
field/indexer for the identical shape" — implying luals has some inherited-
operator behaviour luabox reproduces. It does not: `vm.runOperator`
(`script/vm/operator.lua:101-120`) reads only the value's *own* class's
`.operators`, never `set.extends`, and `class.operators` is populated purely
per-`doc.class`-block at parse time (`script/parser/luadoc.lua:2084`) with no
code path anywhere in `script/vm/` that copies a parent's `---@operator`
onto a child (production-readiness-assessment-9natxz C1, confirmed by
direct measurement: a subclass inheriting an operator from its only
operator-declaring ancestor produces no diagnostic at all downstream in
luals, on either operand, while the identical class declaring and using the
operator itself flags correctly). `---@class Vector : Shape` where only
`Shape` declares `---@operator add` simply never resolves `+` on a `Vector`
in luals — there is no rule to match, agree with, or diverge from. luabox
resolving it anyway is real, deliberate, additional capability, not a bug
and not a parity gap; it just has no oracle to check its own precedence
against, which is why its internal first/last-listed choices below are
luabox's own and were modelled on the field rule that finding 6 found
backwards — worth a second look on its own terms, not assumed correct
because it "matches" a rule that turned out to be wrong. The first three
rows (single, dup-same-file, dup-cross-file) are the genuine luals-parity
part of this table: luals resolves a single class's *own* declared
overloads by the identical "first whose input accepts the operand" scan
(`checkOperators`, `operator.lua:66-95` — it walks `operators` in declaration
order and returns on the first whose operand `vm.isSubType` accepts; the
`getSets` loop at `:111-116` is only what calls it, once per `doc.class`
set), a union-of-declarations-then-first-match mechanism luabox's own
accumulation matches.

| Arrival shape | Winner | Same in develop? | Fixture id |
|---|---|---|---|
| single | resolves (baseline) — luals parity: both scan the class's own declared overloads | yes | `operator-A-single` |
| dup-same-file | **first** overload matched by the "first input that accepts" scan (#114) — luals parity, same scan mechanism (`checkOperators`, `operator.lua:66-95`) | yes | `operator-B-absorb-block-dup-same-file` |
| dup-cross-file | **first**-processed file's overload, same scan mechanism — luals parity | yes | `operator-C-merge-file-types-dup-cross-file` |
| unrelated-parents | **first**-listed parent — luabox-only extension, no luals rule to match (see above) | yes | `operator-D-unrelated-parents` (+ `-swapped`) |
| diamond-identical | resolves to the agreed value — luabox-only extension, no luals rule to match | **no** — develop leaks raw name (declared fix) | `operator-E-diamond-identical-binding` |
| diamond-conflicting | **last**-visited ancestry edge — internally consistent with field/indexer's rule for the identical shape (finding 3, fixed: was first-visited), confirmed both directions, but luabox-only: no luals rule to match | **no** — develop unobservable (leaks raw name in both checks, not just one) | `operator-F-diamond-conflicting-binding` (+ `-swapped`) |
| bound-vs-bare | not separately fixtured; substitution follows the same `collect_operators` binding as fields/indexers — luabox-only extension | — | — |

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
| unrelated-parents, conflicting scopes | **first**-listed parent — matches indexer/operator's first-listed rule for the identical shape (finding 4, fixed: was last-listed via a LIFO `walk_ancestor_names` stack pop order; the stack now pushes reversed, so ordinary declaration-order traversal decides it, the same left-to-right rule every other seam in this document already reads for "first") | yes | `visibility-D-unrelated-parents-conflicting-scope` (+ `-swapped`) |
| diamond, same owner reached twice | unambiguous — both edges reach the identical declaration, nothing to arbitrate | yes | `visibility-E-diamond-identical-owner` |
| diamond-conflicting | **N/A** — visibility isn't parameterized by generic arguments, so a diamond cannot bind it two different ways | — | — |

## Cross-references

- `docs/03-reference/02-limitations.md` — "Duplicate `---@class` declarations
  union" states the field/indexer/type-parameter rules in prose; this page
  is its measured, tabular backing plus the method/operator/visibility rows
  it does not cover.
- `CHANGELOG.md` Unreleased — the declared fixes this page's "Same in
  develop?" column checks every disagreement against.
