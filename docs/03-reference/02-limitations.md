# Known limitations (0.2)

luabox 0.2 checks stock LuaCATS more strictly than lua-language-server, but it
is early software. This page lists the gaps a real user is likely to hit in the
first week — each one verified against the shipping binary — so nothing here is
a surprise. It is deliberately short: small parser trivia is left out so the
handful of things that actually matter stay visible.

Where an item has a tracking issue, it is linked.

## Type system

### `---@alias` cross-file semantics (shipped — #110 closed; edge-case notes)

`---@class`, `---@enum`, and `---@alias` names are all workspace-global: an
alias declared in any project file is nameable and enforced from every other
file (luals parity), so no `require()` and no `[types] defs` package is needed
to share an alias by name. A same-name `---@alias` declared in more than one
project file (or shadowing a `[types] defs` alias) warns as
`duplicate-doc-alias` (`LB0310`) at the losing site, matching luals, while the
deterministic first-wins winner is unchanged. A self- or mutually-referential
alias (`---@alias A B` / `---@alias B A`, across files or within one, or a
bare `---@alias A A`) is reported as `LB0314`, at the alias's own declaration
— once per checked file that references it, however many places in that file
do — the recursive edge itself still terminates safely, lowering to
`unknown` rather than recursing, matching luals' `cyclic-alias` diagnostic.

### Duplicate `---@class` declarations union (#49 — one edge documented)

Two `---@class` declarations for one name are two halves of one intent, so
they **union**: parents, `---@field` members, `---@operator` overloads,
visibility modifiers and carrier attachments from every declaration land on the
same class. This holds identically whether the declarations sit in one file, in
two project files, or in a `---@meta` definition file — the file boundary does
not change the merge, which is what luals does (it resolves a member against
every `doc.class` set carrying the name). A class carried more than once
(`---@class Two` over two different tables) collects the members of both
carriers.

The union is what the cycle check (`LB0318`, below) walks too: `---@class W`
followed by `---@class W : W` is a cycle, because `W`'s parents are the union
of both declarations — it was not, for one release candidate, and the shape
was silent in luabox while luals reported it.

A same-name **field** declared twice is where luabox and luals part company.
Within one file luabox keeps the **first** declaration — inside one
`---@class` block or a second block for the same class — and warns at the
loser as `duplicate-doc-field` (`LB0311`), the same deterministic first-wins
trade it makes for duplicate aliases (`LB0310`) and enums. luals instead
*unions* the two declared types into `string|number`. Choosing the union
would make a mistyped duplicate silently widen the field rather than be
reported, so the warning plus a stable winner reads as the more useful
answer.

**Across files the story is worse, and is tracked as
[#73](https://github.com/flying-dice/luabox/issues/73): the winner is not
project-wide and nothing warns.** Measured (this release and the merge base,
byte-identical): the declaration in the *referencing* file wins, a file
declaring neither gets the alphabetically-first declaring file's binding, and
no `LB0311` fires — a project can hold `x: string` in one file and
`x: number` in another, read both, and check clean. The same rule reaches
generic parent instantiations (`C : Box<number>` here, `C : Box<string>`
there) through this release's new parent-argument substitution. One rule,
two mechanisms, only one of which diagnoses — until #73 lands, keep a class's
declarations in one file if its members must mean one thing.

**That reasoning was reached without consulting luals' implementation, and is
being revisited** ([#67](https://github.com/flying-dice/luabox/issues/67)).
Read from `script/vm/` rather than inferred, luals unions same-rank duplicates
for `---@field`, for a re-declared carrier method, and for a re-declared
indexer — so a project luals checks cleanly can pick up a fresh `LB0300` here
purely from the resolution rule, on code the author did not change. The
warning is worth keeping; the type resolution changing the verdict is the
part that breaks a drop-in migration. See the
[class-merge precedence matrix](03-class-merge-precedence.md) for every cell
and which of them luals agrees with.

**Indexers follow two rules, and which one applies depends on how the key
arrived.** A same-key `---@field [K] V` re-declared on one class name — two
`---@class` blocks, same file or across files — is **first-wins**, the same
deterministic rule a duplicate `---@field` follows, though without the
`LB0311` warning the named-field case emits. Inherited keys split by shape:
two *unrelated* parents (`---@class C : P1, P2`, each declaring the key) is
**first-listed-wins**, while one ancestor reached twice through a generic
diamond with *different* type arguments is **last-listed-wins**. These two
inherited rules are measured against the pre-change binary rather than
derived, because an intermediate build of this release collapsed them into a
single last-wins rule and silently changed verdicts in both directions.

**Correction: an earlier edition of this page described a field/indexer
asymmetry on the unrelated-parents shape as intentional. It was a bug, not a
design choice, and it has been fixed.** This page used to say a class's own
`---@field` members resolve `C : P1, P2` **last**-listed-wins — the opposite
of the indexer rule above — and called that the one deliberate asymmetry in
an otherwise-consistent precedence table. That reasoning was built from
luabox's own code comments and was never checked against
lua-language-server, the tool this project is a drop-in for. Checked against
luals 3.13.5's actual source (`script/vm/compiler.lua:369-375`, gated by
`copyToSearched` at lines 424/510) and confirmed by measurement — swapping a
class's parent order flips which parent luals enforces — luals resolves
**first**-listed-wins for this shape, uniformly, for a `---@field`, a
carrier-attached method, and an indexer alike; there is no asymmetry in the
reference implementation to reproduce. luabox's field (and, by the same
root cause, carrier-attached method) rule now matches: **two unrelated
parents disagree on a plain field or method, and the first-listed one
wins**, the identical rule indexer already had. A project with `---@class C
: P1, P2` where the parents disagree on a shared member's type, previously
resolved to P2's (last-listed) type under `luabox check`, now resolves to
P1's (first-listed) — matching what `lua-language-server --check` already
told that project. See the [class-merge precedence matrix](03-class-merge-precedence.md)
(finding 6) and `CHANGELOG.md` for the full measurement and the user-facing
statement.

**Type parameters are scoped to the declaration that writes them.** Two
declarations of a generic class may spell the parameter differently —
`---@class Boxed<T>` with `---@field value T` beside `---@class Boxed<U>` with
`---@field other U` — and each declaration's field bodies resolve against its
own list, as they do in luals. The templates are then unified *positionally*
when they merge: slot 0 is one type variable however the two spell it, so
`Boxed<string>` makes both `value` and `other` `string`. This holds in one
file and across files alike. Two consequences worth stating: a declaration
naming *another* declaration's parameter is a genuine unknown type name
(`LB0305`) — the scoping cuts both ways — and the parameter *list* follows the
same first-wins rule as every other member, so a duplicate declaring more
parameters than the first has no canonical slot for the surplus, which stays
lenient as `unknown` rather than becoming a placeholder no instantiation could
substitute.

One thing that is **not** a union: a project file's own `---@class` still
*replaces* a same-named class from the stdlib or from a `[types] defs` package,
whole. That is the escape hatch — your declaration corrects the packaged one
rather than merging with it — and it is a different axis from duplicate
declarations in code you wrote.

### `---@class` ancestry is bounded: 200 links deep, 200 re-resolutions wide (`LB0317` / `LB0318` / `LB0319`)

Resolving a `---@class` walks its ancestors recursively. Two hard limits bound
that walk, and a class that trips either one resolves **incompletely** —
members past the point the walk stopped are not in its shape, so reads of them
are unchecked. Both limits are luabox's own; there is nothing to opt out of
them beyond the strictness ladder below.

| Limit | Value | What trips it | Code | Fix |
|---|---|---|---|---|
| `MAX_ANCESTRY_DEPTH` | 200 links | a single-inheritance chain longer than 200 (`C0`, `C1 : C0`, … `C201 : C200`) | `LB0317` | flatten the hierarchy, or declare the members you actually read nearer the leaf |
| — | — | a class reachable from itself (`A : A`, or `A : B` / `B : A`) | `LB0318` | break the loop; a mutual reference is a `---@field`, not an `extends` |
| `MAX_ANCESTRY_RESOLUTIONS` | 200 re-resolutions | the same generic ancestor reached through more than one parent and **bound differently on each branch**, repeated enough times (`L1 : Box<number>`, `R1 : Box<string>`, `C1 : L1, R1`, ×N) | `LB0319` | bind a shared generic ancestor the same way on every branch, or restructure so it is reached once |

`LB0318` is the one of the three that does not wait for a walk: `luabox check`
also finds cycles in the *declared* class graph before anything is resolved,
so a cyclic class in a types file nothing references is reported too — see the
measurement below. The other two are properties of the resolution itself.

The two budgets bound different things and are reported separately on purpose:
depth protects the native stack (an unbounded walk aborts the process on a
host with a small thread stack — an editor embedding the language server is
exactly such a host), while the cost budget bounds the *re*-work a diamond
conflict forces. Flattening a hierarchy fixes the first and does nothing for
the second, so reporting one as the other advises the wrong fix. The cost
budget counts **re**-resolutions only: a hierarchy that is merely large —
hundreds of generated `---@class` declarations, each visited once — does not
trip it.

**The strictness ladder is the same as every other `LB03xx`.** `[types]
strict = true` reports `error` (exit 1); `[types] strict = false` downgrades
to `warning` (exit 0); `Strictness::None`, reachable programmatically but not
from a manifest, reports nothing. Per-rule suppression names:

| Code | `---@diagnostic disable[-line\|-next-line]:` name | whose name |
|---|---|---|
| `LB0317` | `class-ancestry-too-deep` | luabox's own — luals has no depth budget to name |
| `LB0318` | `circle-doc-class` | **luals'** — its own name for the same finding |
| `LB0319` | `class-ancestry-too-costly` | luabox's own — luals has no cost budget to name |

`LB0318` borrows luals' spelling because luals reports this shape too (see
the measurement below), so the directive a luals user already writes silences
LB0318 unchanged. It was `cyclic-class-ancestry` in pre-release drafts of this
page, on an unmeasured claim that luals had no counterpart; the name was
corrected before release, so nothing in the wild breaks.

**Suppression is scoped to the file that DECLARES the class**, not the file
that read it. All three diagnostics are anchored at the `---@class` line the
message names, so a directive has to sit in *that* file — a
`disable-next-line` above the declaration, or a file-wide `disable` at the top
of it. A directive in the file that merely consumes the class does nothing,
even though that is where the error was reported from.

**The cross-file form works under `luabox check` only.** When the declaration
is in file A and the diagnostic is reported while checking file B, `luabox
check` reads A's directives and suppresses it; the **language server does
not** — it checks one open document at a time and never sees A's directives,
so the editor keeps showing the diagnostic even though the CLI is green. This
is the one measured exception to the editor/CLI parity claim made later in
this page, it applies to all three ancestry codes, and it has no issue of its
own yet — it belongs to the same LSP-parity family as
[#70](https://github.com/flying-dice/luabox/issues/70). Same-file suppression
behaves identically in both. No workaround beyond fixing the ancestry itself
or suppressing from the file the editor has open, which only works when that
is also the declaring file.

The one exception is a class declared **only in a `[types] defs` package**.
Nothing in the project declares it, and a definition package's own comments
are never scanned for directives, so there is no declaration line to reach:
the diagnostic attaches to **line 1 of each consuming file** instead. That is
where the directive goes — a file-wide `---@diagnostic disable:
class-ancestry-too-deep` at the top of the consuming file, or a
`disable-line` on line 1 itself — both measured against the shipping binary.
A directive written next to the declaration inside the `.d.lua` has no effect:
definition packages are not project sources and are never scanned for
directives at all.

**Measured against lua-language-server 3.13.5** (the pinned binary,
`--checklevel=Warning`) — and the answer is not the same for all three:

| shape | luals 3.13.5 | pinned by |
|---|---|---|
| a 260-link straight chain | **silent** — no depth budget at all | corpus row `deep_chain_over_depth_limit` (intentional divergence) |
| `---@class A : A` | reports `circle-doc-class` | corpus row `cyclic_class_self` (agreement) |
| `A : B` / `B : A` | reports `circle-doc-class` on both | corpus row `cyclic_class_mutual` (agreement) |
| a cyclic class **nothing references** | reports `circle-doc-class` — its check is declaration-driven | corpus row `cyclic_class_unreferenced` (agreement) |
| a class declared plainly and **reopened** with a back-edge (`---@class W` then `---@class W : W`) | reports `circle-doc-class` **once**, on the reopening declaration | corpus row `cyclic_class_reopened` (agreement) |
| the same cyclic class declared in **two files** | reports `circle-doc-class` **twice**, one per declaration | `check_cmd::tests::a_cyclic_class_declared_in_two_files_reports_once_per_declaration` (the corpus compares the code *set*, never the count) |
| a conflicting generic diamond | silent — 3.13.5 has no generic classes at all | not separately pinned; see the generics rows |

So LB0317 and LB0319 are luabox being deliberately stricter (a project clean
under `lua-language-server --check` can fail `luabox check` on depth or cost
alone), while **LB0318 is parity** — both tools reject a cyclic `---@class`,
and both reject it **from the declaration alone**, whether or not anything in
the project ever resolves the class, on **both** luabox surfaces: `luabox
check` and the language server run the same declared-graph cycle pass, so a
cycle is never red in one and green in the other. Attribution matches too:
one diagnostic per declaration that carries a cycle edge, which is why the
reopened row above is one and the two-file row is two. An earlier draft of
this page said luals "reports nothing for any of these three shapes"; the
cycle half of that was asserted rather than measured, and is false. A later
one claimed the parity flatly while luabox's own LB0318 came only from a
resolution walk, so a cyclic class in an unreferenced types file was flagged
by the editor and green in `luabox check`. The round after that closed the
CLI half only, and the divergence simply changed sign — CI red, editor green
— until the cycle pass moved into `luabox-types` where both surfaces run it.
The corpus rows above are the measurement, re-derived on every run of
`scripts/tests/luals-differential.sh` rather than restated here.
`luabox explain LB0317` (and `LB0318`, `LB0319`) prints the full worked fix
for each.

### Malformed `---@class` headers: reported at the declaration, one finding per header ([#69](https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/work_items/69))

Every row above assumes `---@class` parses into a well-formed class at all.
Four ways a header can fail to — a missing name, a name token the grammar
can't lex as an identifier, a trailing comma in the `extends` list, and a
bare `---@class` with nothing after it — are now reported at the declaration
under two codes:

| Shape | luabox | lua-language-server 3.13.5 |
|---|---|---|
| `---@class : Base` (no name, colon parent) | `LB0320` at the declaration | `luadoc-miss-class-name` ("`<class name> expected`") at the declaration, plus `doc-field-no-class` on the `---@field` line beneath it |
| `---@class 123abc` (non-identifier name) | `LB0320` at the declaration, quoting the name back | identical to the row above — luals treats a name token it can't lex as an identifier the same as a missing name |
| `---@class A : P,` (trailing comma, `P` undeclared) | `LB0321` at the declaration **plus** `LB0305 unknown type name \`P\`` — two mistakes, two findings | `luadoc-miss-class-extends-name` ("`<class extends name> expected`") at the comma, **plus** an ordinary `undefined-doc-class` on `P` |
| bare `---@class` (no name, no colon) | `LB0320` at the declaration; a consumer's `---@type` reference still reports its own `LB0305` | `luadoc-miss-class-name` + `doc-field-no-class` at the declaration |

Both tools now report at the declaration for all four shapes.

**The remaining divergence is message count, not silence.** luals adds a
`doc-field-no-class` per orphaned `---@field` under a nameless header;
luabox reports the header once and leaves the fields alone. One mistake, one
diagnostic: the fields are not independently wrong, and a generated block
with twenty fields under one typo'd header would otherwise produce twenty-one
findings for one edit. `LB0320` and `LB0321` are deliberately separate for
the opposite reason — an unusable class *name* and a bad entry in the
*extends list* have different causes and different fixes, and a header can
carry both at once (`---@class :`), so each gets its own.

**`LB0321` covers more than a stray separator, and says so carefully.** The
type parser has one recovery node for an entry it cannot read, and it does not
record *why* — so `---@class A : ?`, where a token is present and merely
unreadable, is indistinguishable from `---@class A : P,`, where nothing is
there at all. The message says the entry is not a class name, which holds for
both, rather than claiming a name is missing, which would be false on the
first. This is an error under `[types] strict = true`, so the wording has to
be true of every shape that reaches it.

**Where `LB0321` stops: a parent whose name reads.** `---@class A : Base<?>`
is *not* this finding. The entry heads with `Base`, which resolves and whose
members are inherited exactly as `: Base`'s are — nothing is dropped, so the
message ("an entry that is not a class name"), the note ("the entry is
ignored") and the remedy ("replace the entry with the parent it was meant to
name") would all be false of it, and at `Severity::Error` the last one tells
the user to delete a parent that works. The rule looks through the wrappers a
name can be written under (`?`, `[]`, parentheses) and stops at the name; an
entry that is no name at all — a union, a table literal, a `fun` type —
contributes no parent whether or not the parser choked inside it (measured:
`: { x: number }` leaves its class with no members, exactly as no extends list
would), so the note holds there and any unreadable token in one is reported. An unreadable *type argument* is a real mistake and is currently
reported nowhere: `luacats`' recovery errors do not leave the syntax crate,
so `---@field x Base<?>` is silent on the same axis. That gap is the type-
argument axis, not the extends-list one, and it is not what this code covers.

**What a class name has to be.** One or more dot-separated segments
(`geometry.Point`), each starting with a letter, `_`, or a non-ASCII
character and continuing with letters, digits, `_` or non-ASCII. The grammar
`luacats` lexes names with is deliberately looser — any run of
`[A-Za-z0-9_.]` — so a mistyped name is captured whole and quoted back at the
user rather than truncated at the first bad character into something they
never wrote.

That rule has one owner, `luacats::is_type_name`, and it lives beside the
lexers it makes a claim about rather than in the crate that raises the
diagnostic. `LB0320`'s message tells the user that nothing can reference the
class; that is an assertion about the *reference-side* parser, so
`name_is_spellable_by_the_type_parser` pins the round trip in both directions
— every name the rule accepts parses back as itself, and every name it
rejects does not. Loosening one lexer without the other now fails a test
instead of silently turning the diagnostic into a false positive.

**Strictness ladder and suppression, as for every other `LB03xx`:**

| Code | `---@diagnostic disable[-line\|-next-line]:` name | whose name |
|---|---|---|
| `LB0320` | `luadoc-miss-class-name` | **luals'** — measured firing on all three shapes it covers |
| `LB0321` | `luadoc-miss-class-extends-name` | **luals'** — same |

`crates/luabox-types/tests/malformed_class_headers.rs` covers each shape
against a control differing in exactly one respect, and
`crates/luabox-types/tests/class_merge_precedence_matrix.rs`'s
`malformed_class_headers_m66` — which pinned the *silence* while the gap was
open, `#[ignore]`d — now pins the four shapes' diagnostics and runs in the
default suite. The `verdict-differential` corpus carries a row per shape, so
this axis is covered by the self-regression gate too. `luabox explain LB0320`
(and `LB0321`) prints the full worked fix.

### LuaCATS tags: the full vocabulary is enforced

Every LuaCATS tag now influences checking, navigation, or docs — nothing is
parsed-but-ignored: `---@class` (incl. `: Parent` conformance), `---@field`
(incl. `duplicate-doc-field`), `---@param`, `---@return`, `---@type`,
`---@alias` (same-file, defs, and cross-file by name, incl.
`duplicate-doc-alias`), `---@generic`, `---@enum`, `---@overload`, `---@cast`,
`---@meta` (a definition file's `---@field` declarations *and* its
carrier-style `function Class:method()` definitions both join the class
surface), `---@deprecated` (use sites diagnosed, luals `deprecated`),
`---@nodiscard` (discarded returns diagnosed, luals `discard-returns`),
`---@operator` (overload result types applied during inference, luals parity),
`---@private` / `---@protected` / `---@package` (member visibility enforced,
luals `invisible` — `LB0312`, via the `---@field <scope>` modifier, the
standalone tag on a `function Class:method` block, and on a bare
`Carrier.method = function() … end` assignment), `---@diagnostic` (lint +
checker suppression), inline `--[[@as T]]`, `---@vararg` (the legacy spelling
of `---@param ... T`; both on one block union, matching luals), `---@async`
(calls from non-async functions warn as luals `await-in-sync`, `LB0316`;
top-level calls are fine — the main chunk is an async context),
`---@version` (a symbol whose version set excludes the project `edition`
warns at use sites as luals does, riding the `deprecated` diagnostic —
`>5.2`/`JIT`/comma lists, and 5.1 implies LuaJIT), `---@source`
(goto-definition redirects to the annotated location), and `---@see`
(rendered in hover and as linked "See also" sections in `luabox doc`).

A `---@class` is carried by whatever its statement binds, and luabox draws no
distinction between the spellings: `local M = {}`, `Glob = {}`, and a
re-assignment of an existing name all carry the class, and every later
`function Carrier:method()`, `function Carrier.fn()` or `Carrier.const = v`
attaches to it (#50 — the global spelling used to lose its members). A variable
carried twice answers with its most recent carrier, the way Lua resolves the
name; and the carrier *variable* wins over a class of the same name, so
`---@class Wrapper` over `Glob = {}` makes `function Glob:m()` a member of
`Wrapper`, whichever order the declarations appear in.

Deliberate parity boundaries (luals behaves the same way): async-ness never
*propagates* (only an explicit `---@async` tag counts, matching luals's
default `awaitPropagate = false`), and using a `---@deprecated` class purely
as a type annotation is not flagged — luals's `deprecated` diagnostic also
fires only on value/call use sites.

One edge of member visibility (`LB0312`) is deliberately conservative. Whether
an access is "inside the class" is judged from the enclosing **carrier method**
(`function Class:method` / `function Class.fn`), matching luals's environment
rule; and `---@package` scopes to the file that declares the member's **class**
(so a class split across files treats every declaring file as in-package). Where
the receiver's class cannot be resolved to a single `---@class` — a union, or a
plain inferred table — no `invisible` is raised, keeping false positives out at
the cost of a few false negatives.

A `:` method call whose receiver resolves (through inference) to a single
declared `---@class` is now argument-checked against the method's signature and
flags a `---@deprecated` method at the call site (#118), the same as a
dotted/free call. Resolution is deliberately conservative: when the receiver is
not a single declared class — an unknown/`any`/union receiver, a plain inferred
table, an unannotated method, or an unresolved metatable — the `:` call is left
*argument*-unchecked (no false positives). The callee's own use-site tags carry
no such risk, so `---@deprecated`/`---@async`/`---@version` on a
`function Class:method()` reach `obj:method()` for **every** resolved receiver
(#33) — including a plain prototype with no `---@class`, a method that is both
`---@field`-declared and defined, a `---@class` carrier with no
`C.__index = C` link (luals folds carrier attachments into the class off the
carrier binding, with no metatable reasoning), and a carrier-style member
defined in a `---@meta` defs file (#39). A `---@deprecated` class used purely
as a type annotation (not through a value use site) is not flagged —
deliberate luals parity; its `deprecated` diagnostic also fires only on
value/call use sites.

Two boundaries here are real and deliberate. A receiver that cannot resolve to
a single declared class at all — an `any`/unknown parameter, a union, a table
built behind an unresolved metatable — surfaces nothing: the method it names is
not known, so there are no tags to report. And a *dotted* call reached through a
value expression (`w.helper(…)` where `w` is an instance) is not
argument-checked; dotted callees resolve by name, so only `M.helper(…)` on the
module table itself is. Both cost false negatives, never false positives.

### The `__index`-less carrier: member *resolution* falls through

The `C.__index = C`-less carrier is not only a tag-propagation case. **Member
resolution itself falls through**: a value derived from `setmetatable({}, C)`
resolves `C`'s carrier-attached members *as if `__index` were set*, so `c:m()`
type-checks, takes `m`'s declared signature and return type, and reports
nothing — no `LB0306` — even though no metatable link exists at runtime. This
is the deliberate luals-parity behaviour of #33 (luals folds carrier
attachments into the class off the carrier binding, with no metatable
reasoning), and it is the one place where that parity and the runtime disagree
outright:

```lua
---@class Counter
---@field n integer
local Counter = {}
function Counter:value() return self.n end

local c = setmetatable({ n = 1 }, Counter)
print(c:value())   -- luabox check: exit 0
                   -- lua5.4:       attempt to call a nil value (method 'value')
```

Adding `Counter.__index = Counter` makes the program run. The fall-through is
scoped: it only *adds* resolutions off a resolved carrier — `c:nonexistent()`
on the same value still reports `LB0306` — so a genuinely undefined member is
not hidden by it.

Note that the "false negatives, never false positives" line above is about
**unresolved** receivers (an `any`/union/unknown receiver surfaces nothing
because there is no member to speak about). It does **not** describe this
case: here the receiver resolves fine and the finding the runtime would
justify is suppressed on purpose.

The checker keeps parity — that is what makes annotations portable — and the
runtime gap is covered by the `metatable-without-index` lint (`LB0510`,
suspicious tier), which fires on `setmetatable(t, C)` where `C` is an in-file
`---@class` carrier whose `__index` is never assigned. That rule is
**luabox-specific**: luals ships no equivalent diagnostic, so
`[lint] metatable-without-index = "allow"` restores exact luals behaviour.

#### What that lint does *not* cover

`LB0510` is a partial cover, not a closure of the gap, and both of its bounds
are worth stating plainly.

**It is in-file only.** The carrier, the `setmetatable` call and any `__index`
write must all be in the file being linted. The common shape

```lua
-- src/klass.lua
---@class Klass
local Klass = {}
function Klass:value() return 1 end
return Klass

-- src/main.lua
local Klass = require("klass")
local k = setmetatable({}, Klass)
print(k:value())   -- crashes; `luabox check` and `luabox lint` are both silent
```

is not reported: the `setmetatable` argument resolves to a `require` result,
not to an in-file `---@class` carrier, and the rule stays silent on anything it
cannot see whole.

**Some in-file writes are suppressions too.** Each costs false negatives
rather than false positives, which is the trade a rule with no `LB0306` behind
it has to make:

- an `__index` write with a **computed key** (`C[k] = v`, or `rawset(C, k, v)`
  with a `k` this pass cannot evaluate) — the write *might* be the one that
  matters, so the carrier is treated as wired;
- an `__index` write **inside a function that is never called**, or **in a
  branch that never runs** — the pass has no reachability analysis, so it
  reads a dead `if false then C.__index = C end` as a write;
- an `__index` write **through a local alias in either of those positions**.
  A plain `local mt = C; mt.__index = mt` *is* followed: carrier identity
  propagates along local aliases, including chains, so a write through any
  link counts against the carrier it ultimately names. What buys silence is
  an actual `__index`-shaped write — a bare `local mt = C` with nothing
  written through it settles nothing, and `mt.n = 1` is a plain-field write
  that settles nothing either. Aliasing into a dead branch inherits the
  dead-branch suppression above and nothing more;
- **reassignment of the carrier** (`C = <anything>`): the value the `---@class`
  annotation described is not necessarily the one that reaches `setmetatable`.
  Reassigning an *alias* (`mt = {}`) is not a write to the carrier's table and
  does not suppress.

**A carrier that declares another metafield fires only on an observed
instance-side use.** Any `__`-prefixed key other than `__index` — `__call`,
`__tostring`, `__add`, `__mode`, `__gc`, `__name`, … — marks the carrier as
declaring a metafield, and from there the rule asks a *behavioural* question
rather than a structural one: does this file **reach** a method on an instance
of that carrier? If it does, the finding stands — the idiomatic Vector2
tutorial class (a dot constructor, `:length()`, `__tostring` and `__add`) is
exactly that shape, and `v:length()` does crash. If nothing reaches a method,
the carrier reads as a deliberate operator metatable and stays silent.

*Reaching* is the operative word, and it means a call site the file can
actually get to, not a line of source. Three things are not uses:

- a **declaration**. `Cache.__mode = "k"` beside a `function Cache:reset()`
  that nothing ever invokes is a weak-keyed table with a helper on it, and the
  program runs fine; the rule warned on eight such carriers until the gate
  stopped counting colon-method declarations and started counting reached
  calls (Shockwave round 6);
- a **colon call inside a body nothing enters**. Add one line to the example
  above — make it `function Cache:reset() self:clear() end` — and the file
  still runs to completion, because `reset` is still never called. Round 7
  measured the rule firing on it. The body of an unreached function does not
  execute, so a `self:m()` written in it is not a lookup; what would make it
  one is `c:reset()` on a derived value, which the derivations below see
  directly;
- a **call the file never performs**, which is the general form of the one
  above and is what round 8 measured. `local c = setmetatable({}, Cache)`
  beside `local function boom() return c:reset() end` and `print(type(boom))`
  is a colon call on a derived value that no execution reaches, and so is
  `if false then c:reset() end`.

That last one is answered by two mechanisms, both deliberately shallow:

- **reached bodies.** The rule starts at the chunk and follows the in-file
  call graph to a fixpoint — a plain call of a named function or of a field of
  a named table (local *or* global), a function expression the call site
  writes out, the `__call` dispatch on a derived value, and the colon method a
  receiver's own table or its carrier declares. Uses are counted only in
  bodies that set contains;
- **literal pruning of dead statements.** Everything this decides, it decides
  by reading literals in the source; nothing is propagated, and a condition
  that is a name, a comparison or a call is left alone. Four constructs are
  pruned, and the `if` is read on **both** sides:

  | written                                    | what does not run                                    |
  | ------------------------------------------ | ---------------------------------------------------- |
  | `if false then A end` / `if nil then A end` | `A`; the `else` is the branch that *does* run         |
  | `if true then A else B end`                 | `B` — and every later `elseif` condition and block    |
  | `while false do A end`                      | `A`. `repeat` is never pruned: it tests after its body |
  | `for _ = 1, 0` / `for _ = 1, 10, -1`        | the loop body — zero iterations, from literal bounds  |
  | `do return end` then `A` (and after `break`) | `A` — nothing after a statement that leaves the block |

  Until round 9 only the first row was read, so `if true then … else
  c:reset() end` counted a call no execution performs, and neither dead loop
  headers nor statements after an early return were pruned at all. A zero
  step (`for _ = 1, 10, 0`) is deliberately *not* decided: Lua 5.4 raises
  `'for' step is zero` while evaluating the header, and a program that never
  gets that far is not one this rule should be calling dead code. `goto` is
  not pruned either — it can jump forward to a label in the same block, so
  what follows it may very well run.

So an instance-side use is a **call**, on a value the pass can *derive* from
`setmetatable(_, C)` — derivation is what keeps one class's calls from
settling another's. Three call shapes count:

- a **colon call on a derived value**: `c:m()`. This is the lookup a missing
  `__index` breaks, seen at the site that performs it;
- a **plain call on a derived value**: `c()`, when the carrier's `__call`
  metamethod is a function body in this file whose own receiver takes a colon
  call. That is the chain `c()` → `C.__call(self)` → `self:m()`, and it
  crashes. Only the `__call` body is asked, not every function attached to the
  carrier: a `__call` that reaches no method, beside a `C:reset()` nothing
  invokes, is a program that runs fine;
- **dot dispatch**: `c.m(c)`, when `m` names a function attached to the
  carrier. With no `__index`, `c.m` is `nil`, so this fails with `attempt to
  call a nil value (field 'm')` where the colon form says `method 'm'` — one
  defect, two spellings. The read may be one name back — `local m = c.reset;
  m(c)` is the same lookup with the `nil` bound to a name first — but it is
  the **call** that counts: `local r = c.reset` and `print(type(c.reset))`
  perform the lookup, get `nil`, and run fine. Reading the method off the
  **carrier** (`local m = Cache.reset; m(c)`) is a plain table read that no
  metatable serves, and is not a use of anything.

And a value is derived from `C` when it comes from one of these:

- the construction itself, called on the spot: `setmetatable({}, C):m()`;
- **the construction's first argument**. `setmetatable` mutates the table it
  is handed and returns it, so `local t = {}; setmetatable(t, C)` makes `t` an
  instance exactly as binding the result does. This is the idiomatic
  constructor spelling and was missed until round 8;
- a name bound to either — `local c = setmetatable({}, C)`, and equally
  `c = setmetatable({}, C)` on a name declared earlier or on a global — plus
  any `local d = c` alias. `and` and `or` are followed into the operand the
  expression **definitively evaluates to**, which two facts decide.

  The first is the *left operand's truthiness where the source writes it out*.
  `false and ctor` never evaluates `ctor` at all — the result is the falsy
  left operand — and `false or ctor` *is* the construction. That reading
  folds through nesting, which is what makes the ternary
  `cond and ctor or other` answerable: it parses as `(cond and ctor) or
  other`, so a literal `cond` picks one of `ctor`/`other` and the other side
  is not seeded. Until round 9 `and` seeded its right operand unconditionally
  and no literal was consulted on this path at all, so `false and ctor or
  other` warned about a construction Lua never performs.

  The second applies when the left operand is *not* written out — a name, a
  call, a comparison. A construction is a table and so always truthy, which
  decides the remaining four shapes: `ctor or x` *is* the construction (`x`
  never evaluates) and is followed; `x and ctor` is followed too, because when
  `x` is truthy the result is the construction and when it is falsy the same
  colon call fails just as hard; `x or ctor` is followed only as far as `x`,
  since whether the construction is reached depends on a truthiness this pass
  cannot decide; and `ctor and x` evaluates to `x`, so the construction is
  discarded. A ternary with an undecided `cond` is seeded from its `ctor`
  side, which is a false positive when that `cond` is always falsy and the
  `or` fallback answers the call — the same undecided-guard bound listed
  below, in a second spelling;
- the constructor pattern: a function whose body returns a construction
  (directly, or via a name it bound to one) is a factory for that carrier,
  so `local c = Counter.new(); c:value()` is a use, and so is the module-table
  spelling `local c = M.new()` whether `M` is a local or a global. Return
  **slots** are tracked, so `return 1, setmetatable({}, C)` seeds the second
  name of `local n, c = make()` and not the first. Constructor depth is one —
  a factory returning another factory's result is not chased.

**What this rule does not decide, in full.** Four rounds of review found this
section claiming "one approximation remains" while the pass carried several,
so the claim is a list rather than a count, written against the code and then
checked back against it. Both directions are here, because a rule that
discloses only its misses is describing half of itself.

**What is false-positive-shaped** — a finding on a program that runs. Each
entry has a committed matrix fixture, with the twin that shows the same shape
being read correctly on the other side of it.

- **an undecided guard.** The prune reads *literals* only, so a guard that is
  a name is not decided: `local on = false; if on then c:reset() end` still
  counts as reaching a method, and so do the two other places a guard appears
  — the ternary (`local ready = false; local c = ready and
  setmetatable({}, C) or other`) and the numeric-`for` header (`local n = 0;
  for _ = 1, n do c:m() end`). All three are the same bound the
  `__index`-write side has carried since the rule shipped ("an `__index`
  write inside a branch that never runs", above): deciding them needs
  constant propagation rather than a look at the token, and a partial
  constant propagation that decided `and`/`or` but not `if` would be worse
  than none, because the two would then disagree about the same program;
- **flow-insensitive derivation.** Which carrier a value belongs to is a
  property of the name, not of the line: a name carries *every* value ever
  bound to it. So `local c = setmetatable({}, C); c = plain; c:m()` counts
  the call on `plain` against `C` and warns about a program that runs. This
  is deliberate — the alternative is an ordering this pass does not compute —
  and the next two entries are the same fact in other clothes;
- **two `setmetatable` calls on one table.** `setmetatable` *replaces* the
  metatable, so only the last call is in effect when the lookup happens.
  Each call is a genuine construction site and the rule reports one finding
  per site (which is what pins `metafield_lt_invoked_method`, two
  constructions of one carrier, at two findings), so a superseded first call
  is reported against a lookup the winning metatable serves perfectly well;
- **an insert-last-wins name-to-body map.** Functions are mapped from name to
  body with the last attachment winning, so `local function run() … end;
  run(); run = function() c:m() end` attributes the call to the replacement
  — a body it never enters — and warns. The same map misses the mirror
  program, which is in the false-negative list below.

**And what is false-negative-shaped** — silence on a program that crashes.
Eleven of the twelve have a committed matrix fixture beside the twin that
reads the other way, so that closing one is a deliberate act rather than a
surprise; the `require` bound is the exception and structurally cannot have
one, because the matrix runs one file at a time and that bound is cross-file
by nature.

- an instance held in a **table field** (`local box = { c = setmetatable({}, C) }`,
  then `box.c:m()`) — this pass tracks names, not table contents;
- an instance arriving as a **parameter** — argument values are not propagated
  into callee bodies;
- an instance taken from a **generic-`for` variable** — the value comes from an
  iterator this pass does not evaluate;
- an instance from a **method-call factory** (`local c = factory:make()`) —
  only a plain call of a named function or a table field resolves to a body;
- an instance in a **`...` slot** — the name is paired with the right slot of
  the right expression, but nothing is known about what a vararg holds;
- a **depth-two constructor** — see above;
- a `__call` that reaches the method through a **nested closure** or a helper
  it calls, rather than in its own body;
- a use inside a **closure that escapes** — passed as an argument, stored
  under a computed key, or returned to whoever required the chunk. No call
  site in this file names that body, so the reached set does not contain it,
  and a `c:m()` written in it is not counted. This is the FN-biased edge of
  the reachability work, and it is what keeps `print(type(boom))` quiet;
- a function **called and then replaced** — the name-to-body map above, the
  other way round: `local function run() c:m() end; run(); run = function()
  end` maps `run` to the replacement, so the body the call really enters is
  never marked reached;
- an instance bound by **`x or setmetatable(_, C)`** with an undecided `x` —
  the construction is reached only when `x` is falsy, and following it
  regardless warned on a program that runs (round 8). Following `x` alone
  costs this miss. A *literal* falsy left operand is decided and does fire,
  as of round 9;
- a construction whose **metatable argument is an alias** of the carrier
  (`local mt = C; setmetatable({}, mt)`). The `---@class` annotation is looked
  up on the binding the argument names, and `mt` carries none, so the rule
  never reaches its gates. Rooting that lookup through the alias map would
  also root a *reassigned* alias (`mt = {}`), which is a false positive, so
  this stays a miss;
- a carrier reached through **`require`** — the in-file bound, above.

Those misses are the direction this arm has to err in: the carrier declared a
metafield, so silence is the plausible reading.

Two approximations previously recorded here are **closed**. The round-7 one —
a `self:m()` inside `if false then … end` within the `__call` body — closed
when the prune started applying wherever statements are collected, that body
included. The round-9 one was the prune itself being one-sided: it dropped the
`then` of a literal-false `if` and never the `else` of a literal-true one, and
it read no loop header and no early return at all.

**The behavioural gate applies to the metafield arm only.** A carrier with no
metafield at all is judged structurally — the construction alone is enough,
whether or not any method is invoked — and that is deliberate.
`local c = setmetatable({ n = 1 }, Counter)` on a `Counter` with no `__index`
and no metafield cannot serve the lookup the `---@class` annotation promises,
however little the file goes on to do with it, and that reading is pinned by
four rounds of measurement.

Two things still deliberately do not make a carrier a class:
`---@field`-declared members (the canonical carrier declares `---@field n
integer` for a data field, not a method) and dot-assigned function fields
(`C.new = function() … end` is called as `C.new()`, never through the
metatable). Neither is an instance-side use on its own.

The shape matrix behind every claim in this section is committed and runnable:
`scripts/tests/lb0510-matrix/` holds one program per shape with its expected
lint verdict, and `scripts/tests/lb0510-matrix.sh` re-derives both columns —
what `luabox lint` says, and what `lua5.4` does when the program is executed.

**It is lint-only.** `LB0510` never affects `luabox check`'s exit code; it
appears in `luabox lint` (and in the editor, on the lint channel). Wiring it
into `check` would put a heuristic behind the command CI gates on, which is
exactly the split the `check`/`lint` separation exists to keep.

Cross-file carrier analysis is not a tightening of this rule but a different
pass — one with the project's require graph in hand — and it is not shipped.
The reliable defence remains `C.__index = C`.

Every operator luals supports applies. Binary/unary operator *expressions*
(`add`, `sub`, `mul`, `div`, `mod`, `pow`, `idiv`, `concat`, `band`, `bor`,
`bxor`, `shl`, `shr`, `unm`, `bnot`, `len`) are typed on the operator-expression
path, including right-operand dispatch and overload selection by parameter type.
`---@operator call` hooks the call-evaluation path instead (#122): a value whose
type resolves to a single declared `---@class` (through inheritance) declaring a
`call` overload is itself callable — `obj(arg)` checks the argument against the
operator's input type and takes its declared result type, flowing into
assignments, returns, and further checks. A no-input `call: R` operator accepts
any arguments; multiple `call` overloads select by argument type. Resolution is
conservative: an unknown/`any`/union callee or a plain table (no declared class,
or a class with no `call` operator) is left exactly as before — no synthesized
signature and no new diagnostic.

### `goto`/label/`break` legality (shipped — #44 closed; one rule left out)

The three programs every reference Lua rejects at load time are now errors in
`check`, `lint` and the editor: a `goto` with no visible matching label
(`LB0020`, with a did-you-mean nudge for a near-miss name), a label already
defined in scope (`LB0021`), and `break` with no enclosing loop in the same
function (`LB0022`). The verdicts were built against `luac5.4 -p` and
`luac5.1 -p` on a 53-program matrix and agree with them cell for cell, with
the one exception below. Duplicate-label scope follows each edition's own
rule — Lua 5.4 rejects a nested label shadowing an outer one, 5.2/5.3/LuaJIT
do not — so luabox never rejects what your `edition`'s compiler accepts.

**Left out on purpose:** reference Lua also rejects a forward `goto` that
jumps *into* the scope of a local (`goto skip local x = 1 ::skip::` →
`jumps into the scope of local 'x'`). That rule counts active locals at the
jump and at the label, with a special case for a label that is the last void
statement of its block; `;` is erased on lowering, so the HIR cannot express
"void statement" and the check would be a re-implementation of the reference
parser rather than a reading of the resolution luabox already has. luabox
accepts such a program and your interpreter still rejects it at load time.
This is an under-approximation only — no legal program is ever rejected for
it.

### Bidirectional / contextual typing (#120)

A function *literal* written where a `fun(...)` type is expected now takes that
expected type's parameter types for its own parameters, so its body checks with
no per-parameter annotation — the canonical bidirectional win (like typing a
callback's parameters from the callback type). Two positions are covered:

- **call argument** — `higher(function(w) ... end)` where `higher` declares
  `---@param cb fun(w: Widget)` types `w` as `Widget` inside the lambda, so a
  bad field read (`w.nofield`) is flagged (`LB0306`) and misusing `w` where a
  concrete type is expected behaves as if `w` had that type; and
- **`---@type` assignment** — `---@type fun(x: number): number` on a
  `local f = function(x) ... end` types `x` as `number`, and equally on the
  assignment spelling, `M.f = function(x) ... end` (#38).

Conservative by construction: with no expected function type — an unannotated
callee/target, an `unknown`/`any` expected type, or a non-function expected type
— the parameters stay `unknown` exactly as before and no new diagnostic arises.
An explicit `---@param`/inline annotation on the lambda's parameter wins over
the contextual type (annotations are authoritative, SPEC §3).

On an assignment the `---@type` is authoritative for the *value* as well, not
just for the literal's parameters: it supplies the signature call sites are
checked against, and the block's use-site tags
(`---@deprecated`/`---@async`/`---@nodiscard`/`---@version`), which `fun(...)`
syntax has nowhere to write, ride along with it (#38). `---@type A, B` is
positional exactly as on a `local`, so a lone annotation over `a, b = f, g`
declares `a` only, and `b` keeps its inferred type. A declared signature that
disagrees with the literal's own parameter list — extra or missing parameters —
is **not** diagnosed: luals has no such rule (`---@type` simply covers the
value's type), so the declaration governs and luabox stays silent.

A `---@type` over a **non-function** value on an assignment is enforced too, as
of #48. It used to be inert — `---@type string` above `M.a = 1` declared
nothing and diagnosed nothing, so the annotation looked accepted and rotted —
and it now declares the assigned slot and checks the initializer against it
(`LB0300`), exactly as on a `local`. luals binds a doc block to the statement
rather than to the target's syntax, so every spelling gets the same treatment:
a table field (`M.a`), a bracket index with a literal key (`M["a"]`), a nested
field (`M.a.b`, which annotates the innermost slot — the one being assigned), a
global (`G = 1`), and a plain name. The positional rule above is the same one
here, so a lone annotation over `M.a, M.b = x, y` declares `M.a` only. Where a
`---@field` also declares the member, the two do not compete: the `---@field`
governs the **class surface** (what a value annotated with the class reads
back), and the `---@type` governs the assignment it sits above.

One boundary is deliberate. A `---@type <Class>` over `local X = {}` defers its
conformance to the carrier's *final* accumulated shape, so members assigned
later in the file count — a luabox leniency luals does not have. The assignment
spellings have no such deferral: `---@type <Class>` over `G = {}` reports its
missing members against the literal on the spot (`LB0302`), which is what luals
does for both. For a global table built up over several statements, use the
`---@class` carrier spelling — that one *does* collect the members attached to
it later (#50).

The expected type now also propagates *into* literals and through nested
layers, matching luals (`script/vm/compiler.lua`, which lazily compiles a node
against its expected type):

- **into a table literal** — an expected `---@class` (at a `---@type` local, a
  `---@param` argument, or a `---@return` position) types a function-valued
  field's lambda from the field's declared `fun(...)`, so a bad field read
  inside it is flagged; a nested table-literal field takes its declared class
  type. The field-by-field literal diagnostics against the class are unchanged;
- **`return` position** — a `---@return fun(...)` contextually types the
  returned function literal's parameters the same way `---@type` does, and a
  `---@return <Class>` types a returned table literal's fields; and
- **nested/transitive** — an expected `fun(a: A): fun(b: B)` types both the
  outer and the returned inner lambda's parameters.

Still deferred (follow-ups, not yet done): overload-driven expected types and
generic callback inference (a generic callee is deliberately skipped — its
callback parameters carry unbound placeholders that are never guessed). A layer
with no expected type — e.g. a lambda passed to an *unannotated* parameter — is
never typed: propagation follows the expected-type structure and never invents
one.

## Tooling

### Parser nesting and expression-size limits

**Anything reference Lua accepts, luabox parses.** The parser bounds its own
recursion (nesting limit 220, against reference Lua's ~197 — `LUAI_MAXCCALLS`
is 200) and the height of the trees it builds (512), so pathological input
degrades into a diagnostic instead of a stack overflow. Both limits sit above
what `lua5.1`–`lua5.4` and `luac` accept, so no program a reference
implementation compiles is rejected here for being too deeply nested. Past
them you get one `nesting limit exceeded` or `expression too complex` — never
a crash, and the tree stays lossless.

The one place the two disagree is a *flat* operator chain — `a + a + … + a`,
`"a" .. "a" .. …` — which reference Lua parses iteratively at any length.
luabox builds one tree node per operator, so 511 terms is the longest chain
that parses: at 512 you get `expression too complex`. Machine-generated
sources are the only realistic way to reach that; hand-written Lua does not.

### Human diagnostics window very long source lines

`--format human` prints roughly 200 characters of a source line around each
label, with `...` marking whichever side continues, rather than the whole
line. This only shows up on lines longer than that — minified or generated
sources — where printing the line in full made the *report* larger than the
file it described (10 000 findings on a 377 kB single-line file produced
3.5 GB of output). Column numbers are never windowed: `file:line:col` names
the true column here and in every machine format, and `--format json`,
`sarif`, `github` and `gitlab` carry no source text at all, so nothing is
lost to a tool reading them.

Reporting is otherwise linear in the number of findings. The one cost that
still scales with a *single* diagnostic is its own span: a label covering a
megabyte counts a megabyte of characters to size its caret run, once.

### Dependency management and execution are non-goals, not gaps

luabox neither manages dependencies nor runs Lua. There is no resolver, no
lockfile, no registry client, no publish or sign-in path, and no managed
interpreter — those are **deliberate v1 non-goals**, not gaps waiting to be
filled, and nothing here is planned for a later 0.x. The decision record is
in [DIRECTION.md](../../DIRECTION.md#v1-scope-cut-accepted-2026-07-26)
(flying-dice/luabox#10, #11).

In practice: you materialize a rock tree yourself and luabox reads it.

```sh
luarocks install --tree lua_modules penlight
luabox check
```

What that tree buys you is `require` resolution, bundling **and types**: both
the luarocks layout (`lua_modules/share/lua/<X.Y>/…`, `<X.Y>` from your
`[build] target`, `5.1` for `luajit`) and the flat `lua_modules/<name>/` layout
are on the module path, `lua_modules/` is never walked as project source, and a
luarocks tree's installed sources are read for their LuaCATS surfaces (#30,
[decisions/09](../../decisions/09-rock-tree-type-harvest.md)).

**Cross-package types from a bare tree used to be the sharp edge. It is
gone.** A rock's `---@class`/`---@enum`/`---@alias` declarations and each
module's `require`-export type are harvested from
`lua_modules/share/lua/<X.Y>/**.lua` with **no manifest declaration of any
kind** — no `[dependencies]` entry, no per-package `luabox.toml`, no `[types]
defs`. Surfaces only: a vendored body is never typechecked (a type error
inside a rock produces nothing), and a rock source that does not parse is
skipped in silence, named in the LSP log and nowhere else. The editor and CI
harvest the same tree, so they agree.

What that leaves, stated plainly:

- **A rock with no LuaCATS annotations gives you nothing** and stays `unknown`
  to the typechecker, exactly as before. Deliberate, not pending: an
  un-annotated module's export is a table of `unknown`s whose shape is whatever
  its top-level assignments happen to reveal, so harvesting it could only turn
  dynamic module construction into `undefined-field` noise about code you did
  not write. A source with no `---@` anywhere is skipped before it is parsed.
- **A library whose API is a *global*** rather than a module return — LÖVE,
  Neovim, OpenResty — still wants a `defs/` package. The harvest contributes
  type declarations and export types, not ambient globals.
- **Argument checking at a rock function's call site now happens**, and this
  bound is gone ([#46](https://github.com/flying-dice/luabox/issues/46)).
  `local m = require("rock"); m.f("wrong")` reports `LB0300`, and the wrong
  *number* of arguments reports `LB0301`, exactly as the same call written in
  the same file does — it is one shared signature-checking path, so the
  diagnostics read identically on both sides of the module boundary. This
  applies to every `require`d function, your own project modules included and
  not only rocks: a `return M` module table, a module whose export *is* a
  function, nested tables (`m.util.fmt`), colon-methods and dot-calls on an
  exported class, `---@overload`s, `---@vararg`, and `---@param x? T`
  optionals, which stay omittable across the boundary exactly as they are
  within a file.

  What is *not* checked is unchanged and deliberate: a function carrying **no
  signature annotation** is not argument-checked, because an unannotated
  same-file function is not either. Its parameters are a description of a
  body, not a contract, and checking calls against them would invent arity
  errors about code that claims nothing. A rock with no LuaCATS annotations
  still gives you nothing, per the first bullet above.

  Two edges remain out, each far narrower than the bound it replaces (a
  third — a member declared only as a `---@field` on an exported
  `---@class`, never assigned — closed with #56: the carrier now crosses
  the boundary as the class it carries, and the class's declared surface is
  exactly what `---@field` lines populate, so the declared and attached
  spellings are argument-checked identically):

  - **A dynamic require path** — `require(name)` for a computed `name` —
    resolves to no module, so its result stays `unknown` and nothing about it
    is checked. Static string literals are the resolvable set, the same set
    the bundler accepts.
  - **A function re-exported from a second `require`** — module B does
    `local a = require("a"); return { f = a.f }` and a consumer calls
    `require("b").f(...)`. B's *own* requires are deliberately left
    unresolved when B's export type is computed; that is what keeps the
    cross-file registry acyclic and `require` cycles tolerable, and the cost
    is that a signature does not travel two hops. Re-exporting a function
    defined in B itself works.

  An explicit `---@type` at the call site still restores checking in any of
  these cases, as it always did.

  Two call-site rules that the widening exposed have since been settled, and
  each keeps one deliberately narrow edge:

  - **A trailing parameter that admits `nil` is optional for arity.**
    `---@param b number|nil` and `---@param b? number` say the same thing
    about what may reach `b`, and Lua supplies `nil` for every argument the
    caller left off — so omitting it is exactly the call the annotation
    permits, which is what luals concludes too. `f(1)` against
    `f(a: number, b: number|nil)` is clean, on both sides of the module
    boundary. **Only a trailing run counts.** A nil-admitting parameter
    *followed by a required one* still requires an argument, because a caller
    cannot skip a middle argument in Lua without writing `nil` in its place —
    relaxing that slot would let a genuinely short call through. This bound
    was taken conservatively when luals could not be run here; it has since
    been **measured**: lua-language-server 3.13.5 reports `missing-parameter`
    for exactly that call, so the trailing-only rule matches luals, and the
    `nontrailing_nil_param_omitted` row of the luals parity gate
    (`scripts/tests/luals-differential.sh`, CI job `luals-parity`) keeps it
    measured. Equally narrow:
    "admits `nil`" means the type *says* `nil`. `---@param b any` and
    `---@param b unknown` accept `nil` assignably but only decline to
    constrain the parameter, so they stay required.
  - **A string receiver's `:` methods resolve through the `string` library.**
    Every string shares one metatable whose `__index` is the `string` table,
    so `s:upper()` *is* `string.upper(s)` — it types as `string`, `s:byte()`
    as `integer`, `s:match(p)` as `string|nil`, and the arguments are checked
    with the receiver bound (`s:rep("three")` is `LB0300`). A member the
    library does not declare is `LB0306` in both the `:` and the `.`
    spellings, as it is at runtime. A **nil-admitting receiver** is the edge
    left: `---@type string|nil` must be narrowed before a method call, which
    stays lenient rather than reported — the pre-existing rule for union
    receivers generally, not specific to strings.
- **The flat `lua_modules/<name>/` layout is not harvested.** It keeps its
  existing route: a `[dependencies]`/`[dev-dependencies]` entry naming the
  package, a `luabox.toml` for it at `lua_modules/<name>/luabox.toml` (or at
  the `path` you gave) with `[types] defs`, and the `defs/` directory it names.
- **A name collision between two rocks resolves silently**, first-wins in path
  order. You declared neither side and cannot edit vendored code, so there is
  no warning to act on — unlike a collision between two `[types] defs`
  packages, which still warns (`LB0307`/`LB0310`).

Writing the definitions yourself remains the escape hatch, and it is a real
one: a name your `defs/` package — or your own source — declares wins over a
rock's **outright**, not merged, so a wrong or incomplete annotation upstream
is something you can correct locally. Reading a *rockspec* for definition files
it points at is still not planned for 0.x; nothing needs it now.

Your `*.rockspec` and luarocks own everything else (adding, updating,
publishing), and you run your program with whatever Lua you already have.

### A `---@class` module export: closed, in both spellings (#54 → #56)

This section used to record the one shape that did not close: a module
whose export is a `---@class`. The class's `---@field`s live in the
workspace ambient environment, and the per-file view the editor surfaces
were built on could not reach it. Two things closed it (#56): the export
now crosses the `require` boundary **as the class it carries** — the
workspace-global identity, which is what luals resolves a require to — and
hover/completion resolve class members through the **same merged ambient
environment** the checker enforces, so the editor cannot offer what
`luabox check` rejects or omit what it accepts.

**One measured exception, on suppression rather than resolution.** A
`---@diagnostic disable: class-ancestry-too-deep` / `circle-doc-class` /
`class-ancestry-too-costly` written in the file that DECLARES the class
silences the diagnostic under `luabox check` but not in the editor: the
language server checks one document at a time and cannot read the declaring
file's directives, so it emits what the CLI has been told to drop. The
resolution claim above is unaffected — the same members resolve the same way
in both — but "the editor omits what `luabox check` accepts" does not hold
for cross-file-suppressed ancestry diagnostics. Detail and scope are in the
ancestry-limits section above; no issue of its own yet, same LSP-parity
family as [#70](https://github.com/flying-dice/luabox/issues/70).

The table below is still measured, not assumed — the same fixtures pin the
new behaviour: `crates/luabox-cli/tests/features/lsp/hover-require.feature`
for the editor columns, `crates/luabox-cli/tests/features/frontend/require.feature`
for CI.

Given `---@class Point` / `---@field x number` in `point.lua` and
`local p = require("point")` in the consumer:

| module spells its export as | binding hovers as | `p.x` hover | `p.` completion | `luabox check` |
| --- | --- | --- | --- | --- |
| a class **instance** — `---@type Point` on the returned local | `local p: Point` | `Point.x: number` | offers `x` | enforces: `p.x` is `number`, `p.nope` is `LB0306` |
| a class **carrier** — `---@class Point` over `local P = {}` | `local p: Point` | `Point.x: number` | offers `x` | enforces: `p.x` is `number`, `p.nope` is `LB0306` |
| a **generic** carrier — `---@class Box<T>` over `local B = {}` | the structural table, not the name `Box` | the member's erased type, not `Box.item` | offers `item` | **lenient**: `b.nope` is accepted (below) |

The first two rows are symmetric now; the third is the exception, and it is
an exception in three of the four columns rather than only in the checker's
— the export crosses as the monomorphised template, and a template has no
class name for the editor to render either. Naming the class directly
(`---@param p Point`, `---@type Point`) is equivalent rather than a
workaround — class names are workspace-global, so the editor resolves a
class's members with no `require` in sight, exactly as the checker always
did.

**"Enforces" means the class's *declarations* are the member list**, so two
shapes deserve naming — both measured, both with a spelling that resolves
them.

*A member attached under a computed key.* A carrier populated in a loop —

```lua
---@class Handlers
local H = {}
for _, name in ipairs({ "one", "two" }) do
  H[name] = function() return name end
end
return H
```

— has members at runtime that are declared nowhere, so `h.one` in a consumer
is `LB0306`. This is the one shape the closure genuinely narrows: it was
accepted before the export crossed as the class. It is also **luals parity,
not extra strictness** — lua-language-server 3.13.5 reports `undefined-field`
on exactly the same read, measured as a row of the parity gate
(`dynamic_key_carrier_require` in `scripts/tests/luals-differential/`).
Declare the key space and both tools go clean:

```lua
---@class Handlers
---@field [string] fun(): string
```

*A member reached through an undeclared `__index`.* The same rule in its
other spelling: `local T = setmetatable({}, { __index = Proto })` on a
`---@class` carrier borrows `Proto`'s members at runtime, and `Proto` is a
plain table nothing declares — so `t.hello` through a `require` is `LB0306`.
luals agrees here too (`undefined-field`, row
`metatable_index_carrier_require`). Declare the member on the class, or make
the delegate a class the carrier names as a parent (`---@class Thing : Proto`),
and it resolves — an ordinary inheritance chain is unaffected.

Statically visible attachments need nothing: dotted functions
(`function H.one()`), colon methods, data fields, table-literal carriers and
members assigned from a `require` all resolve as before — measured clean
either side of the change.

*A carrier with an unbound type parameter — the exception to "enforces".* A
class name carries no type arguments, so `---@class Box<T>` crossing a
`require` has no `T` to bind. Its members type as `unknown` — the same thing
a bare `Box` reference means in an annotation — rather than leaking the
parameter name into a consumer that cannot name it. Naming the arguments on
the binding types them:

```lua
---@type Box<number>
local b = require("box")
```

The mechanism has a cost worth stating plainly, because it is the one place
the enforcement claim above does not hold: such a carrier crosses as the
**monomorphised template** rather than as the class name, and a template is
a structural table, which carries no member list to enforce. So `b.nope` on
a generic carrier is **clean**, where the identical read on a plain
`---@class Crate` is `LB0306` — the two differ by `<T>` alone. Both
directions are pinned as fixtures
(`an_undeclared_member_on_a_generic_carrier_stays_lenient` and
`a_plain_carrier_still_crosses_as_the_class_itself` in
`crates/luabox-types/tests/cross_file_require.rs`), so the rule cannot flip
back unnoticed. luals 3.13.5 has no generic-class support at all, so the
generic rows in the parity corpus record where the tools part company and
why (`generic_carrier_require`, `generic_carrier_require_bound`).

**"Unbound" includes a parameter inherited from a parent**, and a parent's
arguments now bind. `---@class Sub : Base<number>` over `---@class Base<U>` /
`---@field item U` used to bind nothing at all — the argument was dropped
when the declaration was lowered, so `Sub` inherited `item: U` and a consumer
was told `found U`, a name it can neither produce nor act on. The argument
binds the parent's parameter where the members merge, at every level of the
chain (`---@class Mid<M> : Slot<M>` passes its own parameter up), so `Sub`
has nothing unbound left: `item` is `number`, and the export keeps the class
identity and its enforcement.

What stays unbound is a generic parent named **without** arguments
(`---@class Sub : Base`) — there is no argument to bind, so `Sub`'s members
fall under the rule above and the export crosses as the template. Both
directions are fixtures (`a_parent_type_argument_binds_the_inherited_member`
and `a_parent_written_bare_leaves_its_parameter_unbound_and_erased`).
A **reference site**'s monomorphisation reaches inherited members too, as of
this change: `---@type Leaf` over `---@class Slot<S>` / `---@class Mid<M> :
Slot<M>` / `---@class Leaf : Mid<number>` types `Leaf.slot` as `number`,
where it previously read `unknown`. The template a reference instantiates is
now the class's merged shape rather than its own `---@field` bodies alone,
so a direct generic reference and a plain one agree.

**What a generic reference still does not carry is the class identity.**
`Ty::Named` has no room for type arguments, so `---@type Box<number>` lowers
to the monomorphised *table* — its declared members are typed correctly, but
an **undeclared** member on it is lenient (`LB0300`, `found unknown`) rather
than `LB0306`. It is the same exception the export seam has, reached by a
different route, and it applies to every generic reference rather than only
to a carrier crossing `require`. Pinned as
`undeclared_members_on_a_generic_reference_stay_lenient_lb0300_not_lb0306`.
Closing it means growing `Ty::Named` an argument list — a type-representation
change touching 125 construction and match sites across 18 files — which is
deliberately not in this change's scope.

### `build --mode love` requires an external zip tool

A `.love` file is a zip archive and luabox carries no zip implementation, so
`[build] mode = "love"` is the one build step that shells out to a tool it
does not ship: `zip`, else `python3 -m zipfile`, else `python -m zipfile` on
Linux/macOS; the System32 `bsdtar`, else PowerShell's `Compress-Archive`, on
Windows. With none of them on `PATH` the build fails loudly, naming every
tool it tried — it never writes a partial archive. This does not weaken the
"never spawns an interpreter" claim: the spawned tool archives the output
luabox just emitted and executes no Lua. `mode = "nvim-plugin"` and every
other command need nothing external. The other two commands that leave the
process are `upgrade` and `doc --open`; see
[DIRECTION.md](../../DIRECTION.md#north-star).

### Editor extensions are not on marketplaces yet (#102)

The editor integrations live in their own repos and ship installable
artifacts from their own releases — the VS Code `.vsix` from
[flying-dice/luabox-vscode](https://github.com/flying-dice/luabox-vscode)
(install via `code --install-extension`), the JetBrains plugin `.zip` from
[flying-dice/luabox-jetbrains](https://github.com/flying-dice/luabox-jetbrains)
(install from disk) — but neither is published to its marketplace yet
(VS Code Marketplace / Open VSX / JetBrains Marketplace). Those uploads are
the only residual steps and are pending publisher accounts/tokens this repo
doesn't hold. Any other editor can point its LSP client at `luabox lsp`.

### Prebuilt binaries (#95 — shipped)

Prebuilt binaries ship as of v0.1.0. Every `v*` tag publishes a
[GitHub release](https://github.com/flying-dice/luabox/releases) with binaries
for Linux x86_64, macOS Apple Silicon, and Windows x86_64 (plus `SHA256SUMS`).
The release is created as a **draft** and stays one until, on all three OSes,
the shipped install script has installed that draft's binary and the full
black-box acceptance + LSP acceptance suites have passed against the
*installed* executable; only then does it become a published, `latest` release
(see [RELEASING.md](../../docs/02-guides/01-releasing.md)). The install scripts
([`scripts/install.sh`](../../scripts/install.sh),
[`scripts/install.ps1`](../../scripts/install.ps1)) download the binary for your
platform from the latest release; they do **not** build from source — if you
want that, use `cargo install --git https://github.com/flying-dice/luabox luabox-cli`.
The remaining gap is only reach: not yet on crates.io, Homebrew, or other
package managers.

## Stability expectation for 0.x

The **annotation surface is stable**: it is stock LuaCATS — the same
`---@`-comment dialect lua-language-server reads — and luabox does not add its
own competing type-file format. Existing annotated code keeps working.

What may still move during 0.x is **luabox's own diagnostic behavior**: the set
of rules, their severities, and the `LBnnnn` diagnostic codes may be tuned as
strictness is refined. Pin a toolchain version if you need byte-for-byte stable
diagnostics in CI.
