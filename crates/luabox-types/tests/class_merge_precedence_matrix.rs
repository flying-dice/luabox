// test code — panics document assumptions
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Table-driven pin of every cell in
//! `docs/03-reference/03-class-merge-precedence.md` — the measured matrix
//! backing the "one owner per member kind" refactor of `TypeEnv::absorb_block`
//! (same-file duplicate `---@class`), `TypeEnv::merge_file_types` (cross-file
//! duplicate) and `TypeEnv::collect_class` (consume-site ancestor fold).
//!
//! `CELLS` has one row per line of the matrix's six member-kind tables —
//! `---@field`, carrier method, indexer, `---@operator`, type parameter,
//! visibility — crossed with the seven arrival shapes the doc defines
//! (single / dup-same-file / dup-cross-file / unrelated-parents /
//! diamond-identical / diamond-conflicting / bound-vs-bare). Adding a kind or
//! a seam means adding a row here — the point of a table over one `#[test]`
//! per fixture is that a missing row is visible in the diff, not silently
//! absent from `cargo test`'s output.
//!
//! A cell the matrix marks **N/A** (nothing to disagree about — e.g. a
//! diamond can't bind visibility two ways) or **not separately fixtured**
//! (operator bound-vs-bare, which reuses `collect_operators`'s binding with
//! no dedicated corpus entry) is still a row, with `variants: &[]`: the row
//! exists and its `winner` documents *why* there is nothing to run, so the
//! cell is never just missing from this file.
//!
//! Every fixture's Lua source and expected `(code, message)` diagnostics in
//! [`CELLS`] below ARE the measurement corpus — not a copy of one (round 6
//! review M42: an earlier revision of this comment cited a `merge-matrix.md`
//! "measurement corpus" that was never committed to the repository).
//! `regen_merge_matrix_provenance`, near the bottom of this file, is the
//! actual, re-runnable measurement: it runs every variant below through a
//! real `luabox check` subprocess and writes what it measured to
//! `docs/03-reference/merge-matrix-provenance.md`. The fast `#[test]`s above
//! it re-run the same variants in-process on every `cargo test` and assert
//! they still produce exactly what is written here.
//!
//! A cell with more than one `variant` (labelled `swapped`/`reversed` in the
//! matrix doc) is the same matrix cell confirmed in both directions — e.g.
//! `field-D-unrelated-parents` and its `-swapped` twin both assert the
//! **first**-listed parent wins, with the parents' declaration order
//! reversed, so the winner is provably order-dependent rather than a
//! coincidence of which type happened to be `string`. (Round 6 review M39:
//! this sentence used to say **last**-listed — the one rule this round's
//! `9b32867` changed, and the one place restating it drifted from the cell
//! it introduces. Re-measured for this correction, both in-process
//! (`class_merge_precedence_matrix_matches_the_documented_matrix`, which
//! asserts exactly this row) and through a real binary
//! (`regen_merge_matrix_provenance`): first-listed, both directions.)

mod support;
use support::{check_cross_diags, check_self};

/// One Lua project — `("main.lua", ...)` plus zero or more library files
/// (`a.lua`, `b.lua`, `p1.lua`, ...) merged beneath it in the order listed,
/// matching `merge_file_types`'s file-processing order.
struct Variant {
    /// What differs from the cell's primary fixture — `"base"` when there is
    /// only one variant, otherwise the axis flipped for confirmation
    /// (`"C:P2,P1"`, `"a=string,b=number"`, ...).
    label: &'static str,
    /// The fixture id in `docs/03-reference/03-class-merge-precedence.md`,
    /// so a failure can be cross-checked against the doc by name.
    fixture_id: &'static str,
    files: &'static [(&'static str, &'static str)],
    /// The exact `(code, message)` diagnostics `current` produced when this
    /// corpus was measured — what this refactor must still produce.
    expect: &'static [(&'static str, &'static str)],
}

/// Whether [`Cell::winner`]'s prose claims a checkable "which one wins"
/// ordering rule (round 6 review M43). `winner` used to be pure prose, "read
/// [by a human], not asserted" by anything `cargo test` runs — which is
/// exactly how M39 happened: the module doc at the top of this file said
/// **last**-listed for `field-D-unrelated-parents` while this cell's own
/// `winner` field said, correctly, first-listed, and nothing but a human
/// re-reading both ever compared them. `direction` is that comparison, made
/// permanent: it sits in the same struct literal as `winner`, so editing one
/// without the other is a one-line diff away from a failing test, not a
/// silent drift discovered by the next review round.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Direction {
    /// `winner` claims whichever declaration, parent, or ancestry edge is
    /// FIRST (in source or processing order) wins.
    First,
    /// `winner` claims whichever declaration, parent, or ancestry edge is
    /// LAST wins.
    Last,
    /// `winner` makes no first/last ordering claim this file can check: a
    /// baseline resolve, an agreement case (diamond-identical), a
    /// kind-vs-kind precedence rule (declaration beats attachment), or N/A /
    /// not-separately-fixtured.
    Other,
}

/// One matrix cell: a member kind crossed with an arrival shape.
struct Cell {
    kind: &'static str,
    shape: &'static str,
    /// The matrix doc's own words for who wins. Free prose — for a panic
    /// message that names the rule that broke, not just the code that
    /// changed — but no longer *unchecked* prose: `direction` pins its
    /// ordering claim, and `winner_prose_names_the_same_direction_as_its_direction_flag`
    /// (below) asserts the two agree (M43).
    winner: &'static str,
    /// The ordering claim `winner` makes, cross-checked against it by
    /// `winner_prose_names_the_same_direction_as_its_direction_flag` and,
    /// for a cell with a genuine order-swapped twin, against
    /// [`Variant::expect`] itself by
    /// `order_directional_cells_actually_prove_the_claimed_direction`.
    direction: Direction,
    variants: &'static [Variant],
}

const CELLS: &[Cell] = &[
    Cell {
        kind: "field",
        shape: "single",
        winner: "resolves (baseline)",
        direction: Direction::Other,
        variants: &[Variant {
            label: "base",
            fixture_id: "field-A-single",
            files: &[(
                "main.lua",
                r"
---@class Foo
---@field x number
---@type Foo
local f

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(f.x)
",
            )],
            expect: &[("LB0300", "type mismatch: expected `string`, found `number`")],
        }],
    },
    Cell {
        kind: "field",
        shape: "dup-same-file",
        winner: "first declaration (+ LB0311 warning)",
        direction: Direction::First,
        variants: &[Variant {
            label: "base",
            fixture_id: "field-B-absorb-block-dup-same-file",
            files: &[(
                "main.lua",
                r"
---@class Foo
---@field x number
---@class Foo
---@field x string
---@type Foo
local f

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(f.x)
",
            )],
            expect: &[
                ("LB0311", "duplicate field `x` on class `Foo`"),
                ("LB0300", "type mismatch: expected `string`, found `number`"),
            ],
        }],
    },
    Cell {
        kind: "field",
        shape: "dup-cross-file",
        winner: "first-processed file",
        direction: Direction::First,
        variants: &[
            Variant {
                label: "a=number,b=string",
                fixture_id: "field-C-merge-file-types-dup-cross-file",
                files: &[
                    (
                        "a.lua",
                        r"
---@class Foo
---@field x number
",
                    ),
                    (
                        "b.lua",
                        r"
---@class Foo
---@field x string
",
                    ),
                    (
                        "main.lua",
                        r"
---@type Foo
local f

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(f.x)
",
                    ),
                ],
                expect: &[("LB0300", "type mismatch: expected `string`, found `number`")],
            },
            Variant {
                label: "a=string,b=number",
                fixture_id: "field-C-merge-file-types-dup-cross-file-reversed",
                files: &[
                    (
                        "a.lua",
                        r"
---@class Foo
---@field x string
",
                    ),
                    (
                        "b.lua",
                        r"
---@class Foo
---@field x number
",
                    ),
                    (
                        "main.lua",
                        r"
---@type Foo
local f

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(f.x)
",
                    ),
                ],
                expect: &[],
            },
        ],
    },
    Cell {
        kind: "field",
        shape: "unrelated-parents",
        winner: "first-listed parent (luals parity — see production-readiness-assessment-9natxz A1: luals's own compiler.lua:369-375/424 resolves the first-listed parent, confirmed by directly measuring the pinned binary; luabox previously resolved last-listed, reasoned from its own code comments rather than from luals, and was backwards)",
        direction: Direction::First,
        variants: &[
            Variant {
                label: "C:P1,P2",
                fixture_id: "field-D-unrelated-parents",
                files: &[(
                    "main.lua",
                    r"
---@class P1
---@field x number
---@class P2
---@field x string
---@class C : P1, P2
---@type C
local c

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(c.x)
",
                )],
                expect: &[("LB0300", "type mismatch: expected `string`, found `number`")],
            },
            Variant {
                label: "C:P2,P1",
                fixture_id: "field-D-unrelated-parents-swapped",
                files: &[(
                    "main.lua",
                    r"
---@class P1
---@field x number
---@class P2
---@field x string
---@class C : P2, P1
---@type C
local c

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(c.x)
",
                )],
                expect: &[],
            },
        ],
    },
    Cell {
        kind: "field",
        shape: "diamond-identical",
        winner: "resolves to the agreed value",
        direction: Direction::Other,
        variants: &[Variant {
            label: "base",
            fixture_id: "field-E-diamond-identical-binding",
            files: &[(
                "main.lua",
                r"
---@class Base<V>
---@field item V
---@class A : Base<number>
---@class B : Base<number>
---@class C : A, B
---@type C
local c

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(c.item)
",
            )],
            expect: &[("LB0300", "type mismatch: expected `string`, found `number`")],
        }],
    },
    Cell {
        kind: "field",
        shape: "diamond-conflicting",
        winner: "last-visited ancestry edge",
        direction: Direction::Last,
        variants: &[
            Variant {
                label: "A=number,B=string",
                fixture_id: "field-F-diamond-conflicting-binding",
                files: &[(
                    "main.lua",
                    r"
---@class Base<V>
---@field item V
---@class A : Base<number>
---@class B : Base<string>
---@class C : A, B
---@type C
local c

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(c.item)
",
                )],
                expect: &[],
            },
            Variant {
                label: "A=string,B=number",
                fixture_id: "field-F-diamond-conflicting-binding-swapped",
                files: &[(
                    "main.lua",
                    r"
---@class Base<V>
---@field item V
---@class A : Base<string>
---@class B : Base<number>
---@class C : A, B
---@type C
local c

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(c.item)
",
                )],
                expect: &[("LB0300", "type mismatch: expected `string`, found `number`")],
            },
        ],
    },
    Cell {
        kind: "field",
        shape: "bound-vs-bare",
        winner: "bound: substitutes; bare: reads `unknown` (production readiness review finding 5)",
        direction: Direction::Other,
        variants: &[Variant {
            label: "base",
            fixture_id: "field-G-generic-bound-vs-bare",
            files: &[(
                "main.lua",
                r"
---@class Base<T>
---@field item T
---@class SubBare : Base
---@class SubBound : Base<number>
---@type SubBare
local sb
---@type SubBound
local sd

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(sd.item)
want_string(sb.item)
",
            )],
            expect: &[
                ("LB0300", "type mismatch: expected `string`, found `number`"),
                (
                    "LB0300",
                    "type mismatch: expected `string`, found `unknown`",
                ),
            ],
        }],
    },
    Cell {
        kind: "method",
        shape: "single",
        winner: "resolves (baseline)",
        direction: Direction::Other,
        variants: &[Variant {
            label: "base",
            fixture_id: "method-A-single",
            files: &[(
                "main.lua",
                r#"
---@class Foo
local F = {}
function F:m()
  return "s"
end
---@type Foo
local f

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_number(f:m())
"#,
            )],
            expect: &[("LB0300", "type mismatch: expected `number`, found `\"s\"`")],
        }],
    },
    Cell {
        kind: "method",
        shape: "dup-same-file",
        winner: "first-declared carrier, in statement order",
        direction: Direction::First,
        variants: &[Variant {
            label: "base",
            fixture_id: "method-B-two-carriers-same-file",
            files: &[(
                "main.lua",
                r#"
---@class Foo
local F1 = {}
function F1:m()
  return 1
end

---@class Foo
local F2 = {}
function F2:m()
  return "s"
end

---@type Foo
local f

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(f:m())
"#,
            )],
            expect: &[("LB0300", "type mismatch: expected `string`, found `1`")],
        }],
    },
    Cell {
        kind: "method",
        shape: "dup-cross-file",
        winner: "first-processed FILE wins (finding 2 fixed: was unobservable)",
        direction: Direction::First,
        variants: &[
            Variant {
                label: "a=1,b=s",
                fixture_id: "method-C-merge-file-types-cross-file",
                files: &[
                    (
                        "a.lua",
                        r"
---@class Foo
local F = {}
function F:m()
  return 1
end
",
                    ),
                    (
                        "b.lua",
                        r#"
---@class Foo
local F = {}
function F:m()
  return "s"
end
"#,
                    ),
                    (
                        "main.lua",
                        r"
---@type Foo
local f

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(f:m())
",
                    ),
                ],
                expect: &[("LB0300", "type mismatch: expected `string`, found `1`")],
            },
            Variant {
                label: "a=s,b=1",
                fixture_id: "method-C-merge-file-types-cross-file-reversed",
                files: &[
                    (
                        "a.lua",
                        r#"
---@class Foo
local F = {}
function F:m()
  return "s"
end
"#,
                    ),
                    (
                        "b.lua",
                        r"
---@class Foo
local F = {}
function F:m()
  return 1
end
",
                    ),
                    (
                        "main.lua",
                        r"
---@type Foo
local f

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(f:m())
",
                    ),
                ],
                // `a.lua` (first-processed) declares `m` returning `"s"` —
                // `want_string(f:m())` passes cleanly, confirming the winner
                // is `a.lua`'s own value (a real type, not `unknown`) rather
                // than merely "the first entry happens to already be a
                // string": swap the two files' bodies (the `a=1,b=s`
                // variant above) and the winner flips to `a.lua`'s new
                // value, `1`, not `"s"` again.
                expect: &[],
            },
        ],
    },
    Cell {
        kind: "method",
        shape: "unrelated-parents",
        winner: "first-listed parent wins, matching field-D (luals parity — production-readiness-assessment-9natxz A1: the carrier/bindSource lookup runs inside the same searchClass recursion the extends walk drives, so it is subject to the identical first-ancestor-wins gate as `---@field`)",
        // Round 6 review M43: this flag said `Last` while the prose above
        // said first-listed, and nothing compared them. Re-measured against
        // the built binary: `MC : MP1, MP2` with `MP1:m(): number` and
        // `MP2:m(): string` types `c:m()` as `number` (clean against a
        // `number` param); swapping to `MC : MP2, MP1` types it `string`
        // (`LB0300 expected number, found string`). Order-dependent,
        // first-listed. The prose was right and the flag was stale.
        direction: Direction::First,
        variants: &[
            Variant {
                label: "C:P1,P2",
                fixture_id: "method-D-unrelated-parents",
                files: &[(
                    "main.lua",
                    r#"
---@class P1
local T1 = {}
function T1:m()
  return 1
end

---@class P2
local T2 = {}
function T2:m()
  return "s"
end

---@class C : P1, P2

---@type C
local c

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(c:m())
"#,
                )],
                // P1, the first-listed parent, wins — `m` returns `1`, so
                // `want_string(c:m())` mismatches.
                expect: &[("LB0300", "type mismatch: expected `string`, found `1`")],
            },
            Variant {
                label: "C:P2,P1",
                fixture_id: "method-D-unrelated-parents-swapped",
                files: &[(
                    "main.lua",
                    r#"
---@class P1
local T1 = {}
function T1:m()
  return 1
end

---@class P2
local T2 = {}
function T2:m()
  return "s"
end

---@class C : P2, P1

---@type C
local c

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(c:m())
"#,
                )],
                // Parents reversed: P2, now first-listed, wins — `m` returns
                // `"s"`, so `want_string(c:m())` passes cleanly. Confirms the
                // winner is genuinely order-dependent, not a coincidence of
                // which parent happened to return a string.
                expect: &[],
            },
        ],
    },
    Cell {
        kind: "method",
        shape: "diamond-identical",
        winner: "resolves to the agreed value (finding 1 fixed: was N/A)",
        direction: Direction::Other,
        variants: &[Variant {
            label: "base",
            fixture_id: "method-E-diamond-identical",
            files: &[(
                "main.lua",
                r#"
---@class Base
local T = {}
function T:m()
  return "s"
end

---@class A : Base
---@class B : Base
---@class C : A, B

---@type C
local c

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(c:m())
"#,
            )],
            // Both edges (A and B) reach `Base`'s identical `m`, so it
            // resolves cleanly to `"s"` — `want_string(c:m())` passes.
            expect: &[],
        }],
    },
    Cell {
        // Round 6 review M44: this row used to be `shape: "diamond-conflicting"`
        // with a `winner` that opened "N/A (no type param to bind two ways)"
        // and then, in the same string, described a live variant with a real
        // `LB0300` assertion — a genuinely N/A cell and a genuinely live one,
        // sharing one `(kind, shape)` key. `every_cell_is_named_exactly_once`
        // only catches a *duplicate* `(kind, shape)` pair; it has no way to
        // notice that a single row's own prose contradicts its own
        // `variants`. Split honestly below: this is the true N/A row (methods
        // carry no type parameter, so there is nothing for a diamond to bind
        // two ways), and the live fixture moves to its own row under a shape
        // name that says what it actually is.
        kind: "method",
        shape: "diamond-conflicting",
        winner: "N/A: methods carry no type parameter to bind two ways",
        direction: Direction::Other,
        variants: &[],
    },
    Cell {
        // The substitute this slot used to carry (declaration overriding an
        // inherited attachment) was retired: measured, it is not a
        // diamond-conflict at all — A's inherited `Base` attachment and B's
        // own `---@field` declaration are two DIFFERENT classes' contributions
        // to `C : A, B`, i.e. an unrelated-parents shape (finding 6 governs
        // it), not one class's own declaration beating its own attachment
        // (that is method-G, a different, single-class shape). Kept as its
        // own row — not folded back into `unrelated-parents` above — because
        // its assertion exercises the field/method key-locking boundary
        // (`m` resolves via A's subtree before B is ever visited, matching
        // luals's `searchClass`, compiler.lua:369-375/424) that the plain
        // unrelated-parents fixture does not.
        kind: "method",
        shape: "unrelated-ancestor-declaration-vs-attachment (finding 6 repro)",
        winner: "A, first-listed, wins (A1, luals parity) — B's `m` declaration never even gets visited, matching luals's key-locking, not the within-class method-G rule",
        direction: Direction::First,
        variants: &[Variant {
            label: "base",
            fixture_id: "method-F-diamond-conflicting-unrelated-ancestor-declaration",
            files: &[(
                "main.lua",
                r"
---@class Base
local T = {}
function T:m()
  return 1
end

---@class A : Base

---@class B : Base
---@field m fun(self: B): string

---@class C : A, B

---@type C
local c

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(c.m(c))
",
            )],
            // A, first-listed, wins: `m` resolves to Base's inherited
            // attachment (returns `1`), not B's own `---@field` declaration
            // — B's declaration is never visited at all, because luals's
            // (and now luabox's) key-locking resolves "m" via A's subtree
            // before B is ever looked at. `want_string(c.m(c))` mismatches.
            expect: &[("LB0300", "type mismatch: expected `string`, found `1`")],
        }],
    },
    Cell {
        kind: "method",
        shape: "declaration-vs-attachment",
        winner: "the ---@field declaration's type wins",
        direction: Direction::Other,
        variants: &[Variant {
            label: "base",
            fixture_id: "method-G-field-declaration-beats-attachment",
            files: &[(
                "main.lua",
                r"
---@class Foo
---@field m fun(): string
local F = {}
function F.m()
  return 1
end

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(F.m())
",
            )],
            expect: &[],
        }],
    },
    Cell {
        kind: "method",
        shape: "same-file-inheritance-gap (finding 1 repro)",
        winner: "resolves, matching the cross-file control (finding 1 fixed: was LB0306)",
        direction: Direction::Other,
        variants: &[Variant {
            label: "base",
            fixture_id: "method-inheritance-gap-same-file",
            files: &[(
                "main.lua",
                r"
---@class P1
local T1 = {}
function T1:m()
  return 1
end

---@class C : P1

---@type C
local c
local y = c.m
",
            )],
            expect: &[],
        }],
    },
    Cell {
        kind: "method",
        shape: "cross-file-inheritance-control (finding 1 repro)",
        winner: "resolves regardless of which file P1 lives in",
        direction: Direction::Other,
        variants: &[Variant {
            label: "base",
            fixture_id: "method-inheritance-cross-file-control",
            files: &[
                (
                    "p1.lua",
                    r"
---@class P1
local T1 = {}
function T1:m()
  return 1
end
",
                ),
                (
                    "main.lua",
                    r"
---@class C : P1

---@type C
local c
local y = c.m
",
                ),
            ],
            expect: &[],
        }],
    },
    Cell {
        kind: "method",
        shape: "cross-file-signature-gap (finding 2 repro)",
        winner: "resolves to the inferred return type (finding 2 fixed: was `unknown`)",
        direction: Direction::Other,
        variants: &[Variant {
            label: "base",
            fixture_id: "method-cross-file-signature-gap",
            files: &[
                (
                    "p1.lua",
                    r#"
---@class Foo
local F = {}
function F:m()
  return "s"
end
"#,
                ),
                (
                    "main.lua",
                    r"
---@type Foo
local f

---@param n number
local function want_number(n) end

want_number(f:m())
",
                ),
            ],
            // Zero duplication — `Foo` is declared in exactly one file —
            // so there is no precedence decision to make, only whether the
            // carrier method's own (unannotated, body-inferred) return type
            // crosses the file boundary at all. Before the fix it did not:
            // `f:m()` typed `unknown` and this call passed silently, no
            // matter how badly it disagreed with `want_number`'s parameter.
            expect: &[("LB0300", "type mismatch: expected `number`, found `\"s\"`")],
        }],
    },
    Cell {
        kind: "method",
        shape: "cross-file-signature-gap-field-control (finding 2 repro)",
        winner: "resolves — the one-variable control: a `---@field` in the identical shape already worked",
        direction: Direction::Other,
        variants: &[Variant {
            label: "base",
            fixture_id: "method-cross-file-signature-gap-field-control",
            files: &[
                (
                    "p1.lua",
                    r"
---@class Foo
---@field m fun(): string
",
                ),
                (
                    "main.lua",
                    r"
---@type Foo
local f

---@param n number
local function want_number(n) end

want_number(f.m())
",
                ),
            ],
            // Identical cross-file shape, but `m` is an annotated
            // `---@field` rather than a carrier attachment: this already
            // resolved before *and* after the finding-2 fix, isolating the
            // gap to carrier-attached methods specifically, not cross-file
            // member access in general.
            expect: &[("LB0300", "type mismatch: expected `number`, found `string`")],
        }],
    },
    Cell {
        kind: "indexer",
        shape: "single",
        winner: "resolves (baseline)",
        direction: Direction::Other,
        variants: &[Variant {
            label: "base",
            fixture_id: "indexer-A-single",
            files: &[(
                "main.lua",
                r#"
---@class Foo
---@field [string] number
---@type Foo
local f

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(f["k"])
"#,
            )],
            expect: &[("LB0300", "type mismatch: expected `string`, found `number`")],
        }],
    },
    Cell {
        kind: "indexer",
        shape: "dup-same-file",
        // Round 6 review M11: this used to read "first declaration,
        // *silently* (no LB0311-style warning)" — the asymmetry M11 names.
        // The resolution was already the named field's (first wins); only
        // the diagnostic was missing, so a conflicting indexer resolved with
        // no signal while the identical conflict on a named field warned.
        // Measured against the oracle before changing it: lua-language-server
        // 3.13.5 on this exact fixture reports `duplicate-doc-field
        // Duplicate defined fields `[string]`.`, the same as it does for a
        // repeated named field across two blocks of one class.
        winner: "first declaration (+ LB0311 warning), matching field-B",
        direction: Direction::First,
        variants: &[Variant {
            label: "base",
            fixture_id: "indexer-B-absorb-block-dup-same-file",
            files: &[(
                "main.lua",
                r#"
---@class Foo
---@field [string] number
---@class Foo
---@field [string] string
---@type Foo
local f

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(f["k"])
"#,
            )],
            expect: &[
                ("LB0311", "duplicate field `[string]` on class `Foo`"),
                ("LB0300", "type mismatch: expected `string`, found `number`"),
            ],
        }],
    },
    Cell {
        kind: "indexer",
        shape: "dup-cross-file",
        winner: "first-processed file",
        direction: Direction::First,
        variants: &[Variant {
            label: "base",
            fixture_id: "indexer-C-merge-file-types-dup-cross-file",
            files: &[
                (
                    "a.lua",
                    r"
---@class Foo
---@field [string] number
",
                ),
                (
                    "b.lua",
                    r"
---@class Foo
---@field [string] string
",
                ),
                (
                    "main.lua",
                    r#"
---@type Foo
local f

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(f["k"])
"#,
                ),
            ],
            expect: &[("LB0300", "type mismatch: expected `string`, found `number`")],
        }],
    },
    Cell {
        kind: "indexer",
        shape: "unrelated-parents",
        winner: "first-listed parent",
        direction: Direction::First,
        variants: &[
            Variant {
                label: "C:P1,P2",
                fixture_id: "indexer-D-unrelated-parents",
                files: &[(
                    "main.lua",
                    r#"
---@class P1
---@field [string] number
---@class P2
---@field [string] string
---@class C : P1, P2
---@type C
local c

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(c["k"])
"#,
                )],
                expect: &[("LB0300", "type mismatch: expected `string`, found `number`")],
            },
            Variant {
                label: "C:P2,P1",
                fixture_id: "indexer-D-unrelated-parents-swapped",
                files: &[(
                    "main.lua",
                    r#"
---@class P1
---@field [string] number
---@class P2
---@field [string] string
---@class C : P2, P1
---@type C
local c

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(c["k"])
"#,
                )],
                expect: &[],
            },
        ],
    },
    Cell {
        kind: "indexer",
        shape: "diamond-identical",
        winner: "resolves to the agreed value",
        direction: Direction::Other,
        variants: &[Variant {
            label: "base",
            fixture_id: "indexer-E-diamond-identical-binding",
            files: &[(
                "main.lua",
                r#"
---@class Base<V>
---@field [string] V
---@class A : Base<number>
---@class B : Base<number>
---@class C : A, B
---@type C
local c

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(c["k"])
"#,
            )],
            expect: &[("LB0300", "type mismatch: expected `string`, found `number`")],
        }],
    },
    Cell {
        kind: "indexer",
        shape: "diamond-conflicting",
        winner: "last-visited ancestry edge",
        direction: Direction::Last,
        variants: &[
            Variant {
                label: "A=number,B=string",
                fixture_id: "indexer-F-diamond-conflicting-binding",
                files: &[(
                    "main.lua",
                    r#"
---@class Base<V>
---@field [string] V
---@class A : Base<number>
---@class B : Base<string>
---@class C : A, B
---@type C
local c

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(c["k"])
"#,
                )],
                expect: &[],
            },
            Variant {
                label: "A=string,B=number",
                fixture_id: "indexer-F-diamond-conflicting-binding-swapped",
                files: &[(
                    "main.lua",
                    r#"
---@class Base<V>
---@field [string] V
---@class A : Base<string>
---@class B : Base<number>
---@class C : A, B
---@type C
local c

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(c["k"])
"#,
                )],
                expect: &[("LB0300", "type mismatch: expected `string`, found `number`")],
            },
        ],
    },
    Cell {
        kind: "indexer",
        shape: "bound-vs-bare",
        winner: "bound: substitutes; bare: reads `unknown` (production readiness review finding 5)",
        direction: Direction::Other,
        variants: &[Variant {
            label: "base",
            fixture_id: "indexer-G-generic-bound-vs-bare",
            files: &[(
                "main.lua",
                r#"
---@class Base<T>
---@field [string] T
---@class SubBare : Base
---@class SubBound : Base<number>
---@type SubBare
local sb
---@type SubBound
local sd

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(sd["k"])
want_string(sb["k"])
"#,
            )],
            expect: &[
                ("LB0300", "type mismatch: expected `string`, found `number`"),
                (
                    "LB0300",
                    "type mismatch: expected `string`, found `unknown`",
                ),
            ],
        }],
    },
    Cell {
        kind: "operator",
        shape: "single",
        winner: "resolves (baseline)",
        direction: Direction::Other,
        variants: &[Variant {
            label: "base",
            fixture_id: "operator-A-single",
            files: &[(
                "main.lua",
                r"
---@class Foo
---@operator add(Foo): number
local F = {}
---@type Foo
local a
---@type Foo
local b

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(a + b)
",
            )],
            expect: &[("LB0300", "type mismatch: expected `string`, found `number`")],
        }],
    },
    Cell {
        kind: "operator",
        shape: "dup-same-file",
        winner: "first overload matched by the accepts-input scan (#114)",
        direction: Direction::First,
        variants: &[Variant {
            label: "base",
            fixture_id: "operator-B-absorb-block-dup-same-file",
            files: &[(
                "main.lua",
                r"
---@class Foo
---@operator add(Foo): number
---@class Foo
---@operator add(Foo): string
---@type Foo
local a
---@type Foo
local b

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(a + b)
",
            )],
            expect: &[("LB0300", "type mismatch: expected `string`, found `number`")],
        }],
    },
    Cell {
        kind: "operator",
        shape: "dup-cross-file",
        winner: "first-processed file's overload, same scan mechanism",
        direction: Direction::First,
        variants: &[Variant {
            label: "base",
            fixture_id: "operator-C-merge-file-types-dup-cross-file",
            files: &[
                (
                    "a.lua",
                    r"
---@class Foo
---@operator add(Foo): number
",
                ),
                (
                    "b.lua",
                    r"
---@class Foo
---@operator add(Foo): string
",
                ),
                (
                    "main.lua",
                    r"
---@type Foo
local a
---@type Foo
local b

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(a + b)
",
                ),
            ],
            expect: &[("LB0300", "type mismatch: expected `string`, found `number`")],
        }],
    },
    Cell {
        kind: "operator",
        shape: "unrelated-parents",
        winner: "first-listed parent",
        direction: Direction::First,
        variants: &[
            Variant {
                label: "C:P1,P2",
                fixture_id: "operator-D-unrelated-parents",
                files: &[(
                    "main.lua",
                    r"
---@class P1
---@operator add(P1): number
---@class P2
---@operator add(P2): string
---@class C : P1, P2
---@type C
local a
---@type C
local b

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(a + b)
want_number(a + b)
",
                )],
                expect: &[("LB0300", "type mismatch: expected `string`, found `number`")],
            },
            Variant {
                label: "C:P2,P1",
                fixture_id: "operator-D-unrelated-parents-swapped",
                files: &[(
                    "main.lua",
                    r"
---@class P1
---@operator add(P1): number
---@class P2
---@operator add(P2): string
---@class C : P2, P1
---@type C
local a
---@type C
local b

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(a + b)
want_number(a + b)
",
                )],
                expect: &[("LB0300", "type mismatch: expected `number`, found `string`")],
            },
        ],
    },
    Cell {
        kind: "operator",
        shape: "diamond-identical",
        winner: "resolves to the agreed value",
        direction: Direction::Other,
        variants: &[Variant {
            label: "base",
            fixture_id: "operator-E-diamond-identical-binding",
            files: &[(
                "main.lua",
                r"
---@class Base<V>
---@operator add(Base): V
---@class A : Base<number>
---@class B : Base<number>
---@class C : A, B
---@type C
local a
---@type C
local b

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(a + b)
",
            )],
            expect: &[("LB0300", "type mismatch: expected `string`, found `number`")],
        }],
    },
    Cell {
        kind: "operator",
        shape: "diamond-conflicting",
        winner: "last-visited edge (now matches field/indexer, finding 3 fixed)",
        direction: Direction::Last,
        variants: &[
            Variant {
                label: "A=number,B=string",
                fixture_id: "operator-F-diamond-conflicting-binding",
                files: &[(
                    "main.lua",
                    r"
---@class Base<V>
---@operator add(Base): V
---@class A : Base<number>
---@class B : Base<string>
---@class C : A, B
---@type C
local a
---@type C
local b

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(a + b)
want_number(a + b)
",
                )],
                expect: &[("LB0300", "type mismatch: expected `number`, found `string`")],
            },
            Variant {
                label: "A=string,B=number",
                fixture_id: "operator-F-diamond-conflicting-binding-swapped",
                files: &[(
                    "main.lua",
                    r"
---@class Base<V>
---@operator add(Base): V
---@class A : Base<string>
---@class B : Base<number>
---@class C : A, B
---@type C
local a
---@type C
local b
---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end
want_string(a + b)
want_number(a + b)
",
                )],
                expect: &[("LB0300", "type mismatch: expected `string`, found `number`")],
            },
        ],
    },
    Cell {
        kind: "operator",
        shape: "bound-vs-bare",
        winner: "not separately fixtured (see collect_operators binding)",
        direction: Direction::Other,
        variants: &[],
    },
    Cell {
        kind: "typeparam",
        shape: "dup-same-file-renamed",
        winner: "positional unification: slot 0 is one variable",
        direction: Direction::Other,
        variants: &[Variant {
            label: "base",
            fixture_id: "typeparam-B-absorb-block-renamed-same-file",
            files: &[(
                "main.lua",
                r"
---@class Boxed<T>
---@field value T
---@class Boxed<U>
---@field other U
---@type Boxed<string>
local b

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(b.value)
want_string(b.other)
",
            )],
            expect: &[],
        }],
    },
    Cell {
        kind: "typeparam",
        shape: "dup-same-file-second-empty",
        winner: "first (non-empty) declaration's list is canonical",
        direction: Direction::First,
        variants: &[Variant {
            label: "base",
            fixture_id: "typeparam-B-absorb-block-second-empty-same-file",
            files: &[(
                "main.lua",
                r"
---@class Boxed<T>
---@field value T
---@class Boxed
---@field other number
---@type Boxed<string>
local b

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(b.value)
",
            )],
            expect: &[],
        }],
    },
    Cell {
        kind: "typeparam",
        shape: "dup-cross-file-renamed",
        winner: "same positional unification, across files",
        direction: Direction::Other,
        variants: &[Variant {
            label: "base",
            fixture_id: "typeparam-C-merge-file-types-renamed-cross-file",
            files: &[
                (
                    "a.lua",
                    r"
---@class Boxed<T>
---@field value T
",
                ),
                (
                    "b.lua",
                    r"
---@class Boxed<U>
---@field other U
",
                ),
                (
                    "main.lua",
                    r"
---@type Boxed<string>
local b

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(b.value)
want_string(b.other)
",
                ),
            ],
            expect: &[],
        }],
    },
    Cell {
        kind: "typeparam",
        shape: "bound-vs-bare (parent reference)",
        winner: "`: Base<number>` binds the parent's parameter",
        direction: Direction::Other,
        variants: &[Variant {
            label: "base",
            fixture_id: "typeparam-G-parent-bound-vs-bare",
            files: &[(
                "main.lua",
                r"
---@class Base<T>
---@field item T
---@class SubBare : Base
---@class SubBound : Base<number>
---@type SubBound
local sd

---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end

want_string(sd.item)
",
            )],
            expect: &[("LB0300", "type mismatch: expected `string`, found `number`")],
        }],
    },
    Cell {
        kind: "typeparam",
        shape: "unrelated-parents / diamond",
        winner: "N/A: a class's own parameter list has no analogue across parents",
        direction: Direction::Other,
        variants: &[],
    },
    Cell {
        kind: "visibility",
        shape: "single",
        winner: "enforces (baseline)",
        direction: Direction::Other,
        variants: &[Variant {
            label: "base",
            fixture_id: "visibility-A-single-private",
            files: &[(
                "main.lua",
                r"
---@class Foo
---@field private x number
---@type Foo
local f
local y = f.x
",
            )],
            expect: &[("LB0312", "cannot access private member `x` of `Foo` here")],
        }],
    },
    Cell {
        kind: "visibility",
        shape: "dup-same-file",
        winner: "first wins atomically with the field itself",
        direction: Direction::First,
        variants: &[Variant {
            label: "base",
            fixture_id: "visibility-B-absorb-block-conflicting-scopes",
            files: &[(
                "main.lua",
                r"
---@class Foo
---@field private x number
---@class Foo
---@field protected x number
---@type Foo
local f
local y = f.x
",
            )],
            expect: &[
                ("LB0311", "duplicate field `x` on class `Foo`"),
                ("LB0312", "cannot access private member `x` of `Foo` here"),
            ],
        }],
    },
    Cell {
        kind: "visibility",
        shape: "dup-cross-file",
        winner: "first-processed file",
        direction: Direction::First,
        variants: &[
            Variant {
                label: "private,protected",
                fixture_id: "visibility-C-merge-file-types-conflicting-scopes",
                files: &[
                    (
                        "a.lua",
                        r"
---@class Foo
---@field private x number
",
                    ),
                    (
                        "b.lua",
                        r"
---@class Foo
---@field protected x number
",
                    ),
                    (
                        "main.lua",
                        r"
---@type Foo
local f
local y = f.x
",
                    ),
                ],
                expect: &[("LB0312", "cannot access private member `x` of `Foo` here")],
            },
            Variant {
                label: "protected,private",
                fixture_id: "visibility-C-merge-file-types-conflicting-scopes-reversed",
                files: &[
                    (
                        "a.lua",
                        r"
---@class Foo
---@field protected x number
",
                    ),
                    (
                        "b.lua",
                        r"
---@class Foo
---@field private x number
",
                    ),
                    (
                        "main.lua",
                        r"
---@type Foo
local f
local y = f.x
",
                    ),
                ],
                expect: &[("LB0312", "cannot access protected member `x` of `Foo` here")],
            },
        ],
    },
    Cell {
        kind: "visibility",
        shape: "unrelated-parents",
        winner: "first-listed parent (now matches indexer/operator, finding 4 fixed)",
        direction: Direction::First,
        variants: &[
            Variant {
                label: "C:P1,P2",
                fixture_id: "visibility-D-unrelated-parents-conflicting-scope",
                files: &[(
                    "main.lua",
                    r"
---@class P1
---@field private x number
---@class P2
---@field protected x number
---@class C : P1, P2
---@type C
local c
local y = c.x
",
                )],
                expect: &[("LB0312", "cannot access private member `x` of `P1` here")],
            },
            Variant {
                label: "C:P2,P1",
                fixture_id: "visibility-D-unrelated-parents-conflicting-scope-swapped",
                files: &[(
                    "main.lua",
                    r"
---@class P1
---@field private x number
---@class P2
---@field protected x number
---@class C : P2, P1
---@type C
local c
local y = c.x
",
                )],
                expect: &[("LB0312", "cannot access protected member `x` of `P2` here")],
            },
        ],
    },
    Cell {
        kind: "visibility",
        shape: "diamond (identical owner)",
        winner: "unambiguous: both edges reach the same declaration",
        direction: Direction::Other,
        variants: &[Variant {
            label: "base",
            fixture_id: "visibility-E-diamond-identical-owner",
            files: &[(
                "main.lua",
                r"
---@class Base
---@field private x number
---@class A : Base
---@class B : Base
---@class C : A, B
---@type C
local c
local y = c.x
",
            )],
            expect: &[("LB0312", "cannot access private member `x` of `Base` here")],
        }],
    },
    Cell {
        kind: "visibility",
        shape: "diamond-conflicting",
        winner: "N/A: visibility is not parameterized by generic arguments",
        direction: Direction::Other,
        variants: &[],
    },
];

/// Run one fixture project: `main.lua` is always the checked consumer; every
/// other named file is a library merged beneath it, in listed order
/// (`check_cross_diags`), or — when `main.lua` is the only file — a
/// self-inclusive standalone check (`check_self`, folding `main.lua`'s own
/// surface beneath its own ambient — the CLI batch path always does this,
/// self included, `docs/03-reference/03-class-merge-precedence.md`'s finding
/// 1), matching whichever of `absorb_block`'s same-file path or
/// `merge_file_types`'s cross-file path the fixture exercises.
fn run(files: &[(&str, &str)]) -> Vec<(String, String)> {
    let consumer = files
        .iter()
        .find(|(name, _)| *name == "main.lua")
        .map(|(_, src)| *src)
        .expect("every fixture has a main.lua consumer");
    let libs: Vec<&str> = files
        .iter()
        .filter(|(name, _)| *name != "main.lua")
        .map(|(_, src)| *src)
        .collect();
    let diags = if libs.is_empty() {
        check_self(consumer)
    } else {
        check_cross_diags(&libs, consumer)
    };
    diags
        .into_iter()
        .map(|d| (d.code.to_string(), d.message))
        .collect()
}

#[test]
fn class_merge_precedence_matrix_matches_the_documented_matrix() {
    let mut failures = Vec::new();
    let mut checked = 0usize;
    for cell in CELLS {
        if cell.variants.is_empty() {
            // N/A or not-separately-fixtured: the row itself, with its
            // `winner` note non-empty, is the whole assertion — see the
            // module doc comment for why this is still a row, not an
            // omission.
            assert!(
                !cell.winner.is_empty(),
                "{}/{} must document why it has no fixture",
                cell.kind,
                cell.shape
            );
            continue;
        }
        for variant in cell.variants {
            checked += 1;
            let got = run(variant.files);
            let want: Vec<(String, String)> = variant
                .expect
                .iter()
                .map(|(c, m)| ((*c).to_string(), (*m).to_string()))
                .collect();
            if got != want {
                failures.push(format!(
                    "{}/{} [{}] (fixture `{}`, doc says: {})
  got:  {got:?}
  want: {want:?}",
                    cell.kind, cell.shape, variant.label, variant.fixture_id, cell.winner
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of the matrix's fixtured cells produced a different verdict than measured:\n{}",
        failures.len(),
        failures.join("\n")
    );
    // Every fixtured variant in the corpus actually ran. This was a `>= 45`
    // floor while the real count was 51 (round 6 review M65: this comment
    // used to say 52 — measured against `EXPECTED_FIXTURED_VARIANTS` and the
    // commit that introduced this guard, `619517d`, whose own message says
    // 51; the "52" here was a transcription slip in the comment text, not in
    // the code), which meant up to six variants could vanish and the floor
    // would stay satisfied. A merge-gate finder proved it concretely by
    // removing the entire two-variant row that pins carrier methods
    // resolving to the first-listed parent (`method-D-unrelated-parents` and
    // its `-swapped` twin, dropping the count from 51 to 49) and watching
    // both tests pass. The floor is now the exact count, so a row that
    // disappears is a failure rather than a smaller number nobody reads.
    assert_eq!(
        checked, EXPECTED_FIXTURED_VARIANTS,
        "the matrix ran {checked} fixtured variants, expected          {EXPECTED_FIXTURED_VARIANTS} — if you added or removed a cell,          update this count and the published table together"
    );
}

/// The number of fixtured variants across every row of [`CELLS`]. Exact, not
/// a floor: see the assertion above for why.
const EXPECTED_FIXTURED_VARIANTS: usize = 51;

/// Every fixture id named by the published matrix
/// (`docs/03-reference/03-class-merge-precedence.md`) exists as a variant in
/// [`CELLS`], and vice versa — in both directions, for every variant,
/// including `-swapped`/`-reversed` twins and the finding-specific repro
/// fixtures.
///
/// This is the check that makes "adding a member kind or an arrival shape
/// forces a row" true rather than aspirational. Without it the table and the
/// tests drift independently: the doc can describe a cell nothing measures,
/// or a cell can be dropped from the tests while the doc still claims it is
/// pinned. Both have happened on this page already.
///
/// The doc is the source of truth for *which* cells exist; this file is the
/// source of truth for what each one resolves to. Cells the doc marks N/A or
/// unobservable name no fixture id, so they are absent from both sides and
/// stay that way.
///
/// (Round 6 review M62: this used to check one direction only — every
/// documented id has a test — and the doc wrote a `-swapped`/`-reversed`
/// twin as a `(+ `-swapped`)` suffix on its sibling's id, plus a couple of
/// repro fixtures named only in prose ("zero-dup repro: ... (+ field
/// control)"), never as their own literal backtick token. Neither shape was
/// ever a `documented` entry, so the ~10 twin variants and the
/// prose-only repro variants had no doc-side protection at all: deleting
/// `method-D-unrelated-parents-swapped` and its `EXPECTED_FIXTURED_VARIANTS`
/// decrement left every test in this file green. The doc above now spells
/// out every twin and every repro fixture as its own literal backtick id —
/// measured, this closes the gap exactly: the set of `field`/`method`/
/// `indexer`/`operator`/`typeparam`/`visibility`-prefixed backtick tokens in
/// the doc and the set of `fixture_id`s in [`CELLS`] are now equal, 51 or
/// 51, zero missing either direction. The parser below is widened to match
/// (prefix-based, not "single uppercase letter" — that heuristic is what
/// excluded the prose-named repro ids in the first place) and the assertion
/// is now bidirectional, so either side losing an entry independently is a
/// failure, not a silent shrink.)
#[test]
fn the_published_matrix_and_the_test_table_name_the_same_fixtures() {
    const KIND_PREFIXES: &[&str] = &[
        "field-",
        "method-",
        "indexer-",
        "operator-",
        "typeparam-",
        "visibility-",
    ];
    let doc = include_str!("../../../docs/03-reference/03-class-merge-precedence.md");
    let mut documented: Vec<String> = Vec::new();
    for line in doc.lines() {
        // Fixture ids live in table rows, in backticks. Every one — base id,
        // `-swapped`/`-reversed` twin, and finding-specific repro fixture —
        // is spelled out in full as its own backtick token (see the doc
        // comment above); no shorthand suffix notation is left to miss.
        if !line.starts_with('|') {
            continue;
        }
        for token in line.split('`') {
            let looks_like_id = KIND_PREFIXES.iter().any(|p| token.starts_with(p));
            if looks_like_id && !documented.contains(&token.to_string()) {
                documented.push(token.to_string());
            }
        }
    }
    assert!(
        documented.len() >= 30,
        "parsed only {} fixture ids from the published matrix — the table's          shape changed and this parser no longer reads it, which would make          this test vacuous",
        documented.len()
    );

    let tested: Vec<&str> = CELLS
        .iter()
        .flat_map(|cell| cell.variants.iter().map(|v| v.fixture_id))
        .collect();

    // Both directions, now that every variant — including twins and repro
    // fixtures — is spelled out literally in the doc (M62): a documented id
    // with no test is a claim nothing checks; a tested id with no doc
    // mention is a fixture the published matrix doesn't actually advertise.
    let missing_from_tests: Vec<&String> = documented
        .iter()
        .filter(|id| !tested.iter().any(|t| t == &id.as_str()))
        .collect();
    assert!(
        missing_from_tests.is_empty(),
        "the published matrix names {} fixture(s) this table does not \
         measure: {missing_from_tests:?} — a documented cell with no test is \
         a claim nothing checks",
        missing_from_tests.len()
    );

    let missing_from_doc: Vec<&&str> = tested
        .iter()
        .filter(|id| !documented.iter().any(|d| d == *id))
        .collect();
    assert!(
        missing_from_doc.is_empty(),
        "this table measures {} fixture(s) the published matrix does not \
         name: {missing_from_doc:?} — a tested variant with no doc mention \
         can be deleted, with its EXPECTED_FIXTURED_VARIANTS decrement, and \
         nothing here would notice",
        missing_from_doc.len()
    );
}

/// Every `CELLS` row's `(kind, shape)` pair is unique — a duplicate row
/// would mean two entries silently splitting one cell's assertions rather
/// than one owner deciding it, exactly the failure mode this file exists to
/// catch in the production code.
#[test]
fn every_cell_is_named_exactly_once() {
    let mut seen = std::collections::HashSet::new();
    for cell in CELLS {
        assert!(
            seen.insert((cell.kind, cell.shape)),
            "duplicate matrix row: {}/{}",
            cell.kind,
            cell.shape
        );
    }
}

/// Every `CELLS` row with `variants: &[]` — an N/A or not-separately-fixtured
/// cell — is named by a `| — | — |` row in the published matrix, under the
/// matching section header, and vice versa.
///
/// (Round 6 review M63: the module doc at the top of this file claims a cell
/// marked N/A "is never just missing from this file" — but nothing checked
/// that. Deleting either zero-variant `Cell` (`typeparam` /
/// `unrelated-parents / diamond`, or `visibility` / `diamond-conflicting`)
/// with no other edit left every other test in this file green:
/// `class_merge_precedence_matrix_matches_the_documented_matrix` only walks
/// non-empty `variants`, `EXPECTED_FIXTURED_VARIANTS` only counts fixtured
/// variants, `every_cell_is_named_exactly_once` only catches a *duplicate*
/// key, and the fixture-id matcher above only sees rows with a real
/// backtick-quoted id — an N/A row's fixture-id column is `—`, not a
/// fixture. This test closes that: it cross-checks `(kind, shape)` for every
/// zero-variant row on both sides, so a placeholder disappearing from either
/// the doc or the code is a failure, making the module doc's claim true
/// rather than aspirational.)
#[test]
fn every_na_cell_is_documented_and_every_documented_na_row_is_a_cell() {
    const SECTION_KINDS: &[(&str, &str)] = &[
        ("### `---@field`", "field"),
        ("### Method", "method"),
        ("### Indexer", "indexer"),
        ("### Operator", "operator"),
        ("### Type parameter", "typeparam"),
        ("### Visibility", "visibility"),
    ];
    let doc = include_str!("../../../docs/03-reference/03-class-merge-precedence.md");
    let mut kind: Option<&str> = None;
    let mut documented: Vec<(String, String)> = Vec::new();
    for line in doc.lines() {
        if line.starts_with("### ") {
            kind = SECTION_KINDS
                .iter()
                .find(|(prefix, _)| line.starts_with(prefix))
                .map(|(_, k)| *k);
            continue;
        }
        if !line.starts_with('|') || !line.trim_end().ends_with("| — | — |") {
            continue;
        }
        let Some(k) = kind else { continue };
        let shape = line
            .trim_start_matches('|')
            .split('|')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        documented.push((k.to_string(), shape));
    }
    assert!(
        documented.len() >= 3,
        "parsed only {} zero-variant `| — | — |` rows from the published \
         matrix — the table's shape or section headers changed and this \
         parser no longer reads it, which would make this test vacuous",
        documented.len()
    );

    let coded: Vec<(&str, &str)> = CELLS
        .iter()
        .filter(|c| c.variants.is_empty())
        .map(|c| (c.kind, c.shape))
        .collect();

    let missing_from_code: Vec<&(String, String)> = documented
        .iter()
        .filter(|(k, s)| !coded.iter().any(|(ck, cs)| ck == k && *cs == s.as_str()))
        .collect();
    assert!(
        missing_from_code.is_empty(),
        "the published matrix documents {} zero-variant row(s) with no \
         matching zero-variant CELLS entry: {missing_from_code:?}",
        missing_from_code.len()
    );

    let missing_from_doc: Vec<&(&str, &str)> = coded
        .iter()
        .filter(|(k, s)| !documented.iter().any(|(dk, ds)| dk == k && ds == *s))
        .collect();
    assert!(
        missing_from_doc.is_empty(),
        "CELLS has {} zero-variant row(s) the published matrix's \
         `| — | — |` convention does not document: {missing_from_doc:?} — \
         a zero-variant cell can otherwise be deleted from this file with no \
         other edit and every test still green (M63)",
        missing_from_doc.len()
    );
}

/// [`Cell::winner`]'s prose and its [`Cell::direction`] flag must name the
/// same rule (round 6 review M43). They live in the same struct literal, a
/// few lines apart, so this cannot be defeated the way M39 was — one field
/// changed (or added new) without the other being touched.
///
/// A row whose prose mentions BOTH "first" and "last" (contrasting this
/// cell's rule against a sibling's, or narrating a history where the rule
/// changed) is deliberately skipped rather than guessed at: a bare substring
/// scan cannot safely tell "this cell's own rule is first, unlike a sibling
/// which is last" apart from an actual contradiction, and guessing wrong
/// would make this test itself the next source of drift.
/// The ordering claim a "who wins" prose string makes, read from its
/// **headline** — everything before the first `(`.
///
/// Both this file's `Cell::winner` fields and the published matrix's Winner
/// column state the rule first and then qualify it in parentheses ("first
/// declaration (+ `LB0311` warning)", "**first**-listed parent (finding 6,
/// fixed: was last-listed)"). Reading the whole string means a cell that
/// merely *narrates* the rule it used to have reads as claiming both
/// directions at once and gets skipped — which is how the most interesting
/// rows, the ones whose rule changed, ended up unchecked. The headline is
/// the claim; the parenthetical is history.
///
/// `None` means the headline names neither direction (a baseline resolve, an
/// agreement case, a kind-vs-kind rule) or both (a genuinely ambiguous
/// sentence this file will not guess at) — nothing checkable either way.
fn claimed_direction(prose: &str) -> Option<Direction> {
    let headline = prose.split('(').next().unwrap_or(prose).to_lowercase();
    match (headline.contains("first"), headline.contains("last")) {
        (true, false) => Some(Direction::First),
        (false, true) => Some(Direction::Last),
        _ => None,
    }
}

#[test]
fn winner_prose_names_the_same_direction_as_its_direction_flag() {
    let mut failures = Vec::new();
    for cell in CELLS {
        if let Some(claimed) = claimed_direction(cell.winner)
            && claimed != cell.direction
        {
            failures.push(format!(
                "{}/{}: winner text says {claimed:?} but Cell::direction is {:?} — one of \
                 them is wrong:\n  winner: {}",
                cell.kind, cell.shape, cell.direction, cell.winner
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} cell(s) whose winner prose disagrees with their own direction flag:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The **published matrix's own Winner column** must claim the same ordering
/// rule as the [`Cell`] it names — the last unchecked half of the doc↔code
/// contract (production readiness review).
///
/// [`the_published_matrix_and_the_test_table_name_the_same_fixtures`] already
/// cross-checks the *fixture id* column both directions, and
/// [`winner_prose_names_the_same_direction_as_its_direction_flag`] pins each
/// cell's own prose against its own flag — but between them, nothing ever
/// compared the doc's Winner column to `CELLS`. Editing a published cell from
/// "first-listed" to "last-listed" changed the rule this page tells users
/// luabox follows and left every test in this file green: the fixture ids
/// still matched, and the cell's private `winner`/`direction` pair still
/// agreed with each other. The page is the artifact users read; an unchecked
/// column in it is exactly the drift `Cell::direction` was introduced to stop
/// one level down.
///
/// A row is matched to cells by the fixture ids in its last column, not by
/// its shape text — several rows deliberately group a primary fixture with
/// repro fixtures owned by other (non-directional) cells, so the assertion is
/// "the direction this row claims is the direction of at least one cell it
/// names", not "of every cell it names". The converse half then closes the
/// gap that would leave: every cell with a real ordering rule must be claimed
/// by some row, so deleting a Winner column's direction word is a failure too,
/// not a silent downgrade to unchecked.
#[test]
fn the_published_matrix_winner_column_agrees_with_each_cells_direction() {
    const KIND_PREFIXES: &[&str] = &[
        "field-",
        "method-",
        "indexer-",
        "operator-",
        "typeparam-",
        "visibility-",
    ];
    let doc = include_str!("../../../docs/03-reference/03-class-merge-precedence.md");

    let mut by_id: std::collections::HashMap<&str, &Cell> = std::collections::HashMap::new();
    for cell in CELLS {
        for variant in cell.variants {
            by_id.insert(variant.fixture_id, cell);
        }
    }

    let mut failures = Vec::new();
    let mut claimed_rows = 0usize;
    let mut covered: std::collections::HashSet<(&str, &str)> = std::collections::HashSet::new();

    for line in doc.lines() {
        if !line.starts_with('|') {
            continue;
        }
        let cols: Vec<&str> = line
            .trim_end()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        // Every matrix table is `| Arrival shape | Winner | Same in develop? |
        // Fixture id |`. Anything else — the arrival-shape glossary at the top
        // of the page, a separator, a header — is not a cell row.
        if cols.len() != 4 {
            continue;
        }
        let ids: Vec<&str> = cols[3]
            .split('`')
            .filter(|token| KIND_PREFIXES.iter().any(|p| token.starts_with(p)))
            .collect();
        if ids.is_empty() {
            continue;
        }
        let Some(claimed) = claimed_direction(cols[1]) else {
            continue;
        };
        claimed_rows += 1;

        let named: Vec<&Cell> = ids.iter().filter_map(|id| by_id.get(id).copied()).collect();
        let agreeing: Vec<&&Cell> = named.iter().filter(|c| c.direction == claimed).collect();
        if agreeing.is_empty() {
            failures.push(format!(
                "published row `{}` claims {claimed:?} in its Winner column, but no cell it \
                 names resolves that way — {:?}\n  Winner column: {}",
                cols[0],
                named
                    .iter()
                    .map(|c| format!("{}/{} is {:?}", c.kind, c.shape, c.direction))
                    .collect::<Vec<_>>(),
                cols[1]
            ));
        }
        for cell in agreeing {
            covered.insert((cell.kind, cell.shape));
        }
    }

    assert!(
        claimed_rows >= 15,
        "read only {claimed_rows} directional Winner column(s) out of the published matrix — \
         the table's column layout changed and this parser no longer reads it, which would \
         make this test vacuous"
    );

    for cell in CELLS
        .iter()
        .filter(|c| c.direction != Direction::Other && !c.variants.is_empty())
    {
        if !covered.contains(&(cell.kind, cell.shape)) {
            failures.push(format!(
                "{}/{} resolves {:?}, but no published matrix row naming its fixtures says so \
                 in its Winner column — the page no longer states the rule this table measures",
                cell.kind, cell.shape, cell.direction
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} disagreement(s) between the published matrix's Winner column and CELLS:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// A [`Cell`] with a genuine order-swapped twin (`variants.len() >= 2`) and
/// a [`Direction`] other than `Other` must actually SHOW the claimed
/// order-dependence, not merely assert it: if every variant after the first
/// produced the identical diagnostics, reversing the declaration/parent/edge
/// order changed nothing observable, and "first-listed wins" or
/// "last-visited wins" is an unfalsified claim, not a measured one — the gap
/// M43 exists to close. Round 6 review M39's drift was in the prose alone;
/// this test's sibling above catches that. This test catches the shape one
/// level deeper: a `direction` flag with no evidence behind it at all.
#[test]
fn order_directional_cells_actually_prove_the_claimed_direction() {
    let mut failures = Vec::new();
    for cell in CELLS {
        if cell.direction == Direction::Other || cell.variants.len() < 2 {
            continue;
        }
        let baseline = cell.variants[0].expect;
        for other in &cell.variants[1..] {
            if other.expect == baseline {
                failures.push(format!(
                    "{}/{} [{}]: Cell::direction claims {:?}, but this variant produced the \
                     SAME diagnostics as `{}` — that is not order-dependence, it is a \
                     coincidence with a direction claim stapled to it: {:?}",
                    cell.kind,
                    cell.shape,
                    other.label,
                    cell.direction,
                    cell.variants[0].label,
                    other.expect
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} order-directional cell(s) with no actual measured order-dependence:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// === Provenance regeneration (round 6 review M42) =========================
//
// `docs/03-reference/03-class-merge-precedence.md` used to cite a
// `merge-matrix.md` "scratch report this page was built from" that was never
// committed — `find . -name 'merge-matrix*'` returns nothing, so neither
// that 396-line doc nor this 1883-line file could be re-derived or audited
// by anyone who did not already trust the person who wrote them. This is
// the real regeneration mechanism the doc now points at instead: it runs
// every fixture in [`CELLS`] — the same fixtures the fast, in-process
// `class_merge_precedence_matrix_matches_the_documented_matrix` test above
// checks against this crate's library code directly — through an actual
// `luabox` release binary as a subprocess, exactly the way a user's project
// would be checked, and writes what it measured to
// `docs/03-reference/merge-matrix-provenance.md`. There is exactly one
// fixture corpus in this repository now, not a corpus plus a doc plus a
// scratch report that can drift from it three separate ways.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// A directory under the OS temp dir, unique per call, built from only
/// `std` — this crate's `[dev-dependencies]` do not include `tempfile`
/// (only `proptest`), and adding one is outside a documentation-provenance
/// fix's file ownership. `std::process::id()` plus a per-process atomic
/// counter is enough uniqueness for a single test process's own fixtures.
fn fresh_temp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "luabox-merge-matrix-provenance-{}-{label}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(dir.join("src")).expect("create fixture project dir");
    dir
}

/// Materialize one fixture's `files` under `dir/src/` with a minimal strict
/// manifest (edition 5.4 — every fixture in this file is parsed with
/// [`luabox_syntax::lua::Dialect::Lua54`]) and run `bin check` against it,
/// returning the `(code, message)` pairs parsed from its human-readable
/// output (`error[LB0300]: message` / `warning[LB0300]: message` — no JSON
/// parser is pulled in for this alone, for the same dependency-ownership
/// reason [`fresh_temp_dir`] avoids `tempfile`).
fn run_via_binary(bin: &Path, label: &str, files: &[(&str, &str)]) -> Vec<(String, String)> {
    let dir = fresh_temp_dir(label);
    std::fs::write(
        dir.join("luabox.toml"),
        "[package]\nname=\"regen\"\nversion=\"0.1.0\"\nedition=\"5.4\"\n\n[types]\nstrict=true\n",
    )
    .expect("write manifest");
    for (name, src) in files {
        std::fs::write(dir.join("src").join(name), src).expect("write fixture source");
    }
    let output = Command::new(bin)
        .arg("check")
        .current_dir(&dir)
        .output()
        .unwrap_or_else(|e| panic!("failed to run {}: {e}", bin.display()));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut diags = Vec::new();
    for line in stdout.lines() {
        for prefix in ["error[", "warning["] {
            if let Some(rest) = line.strip_prefix(prefix)
                && let Some((code, tail)) = rest.split_once(']')
                && let Some(message) = tail.strip_prefix(": ")
            {
                diags.push((code.to_string(), message.to_string()));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    diags
}

/// Regenerates `docs/03-reference/merge-matrix-provenance.md` by measuring
/// every fixture in [`CELLS`] against a real `luabox check`, and — when
/// `LUABOX_DEVELOP_BIN` names a second binary — a second real `luabox check`
/// from that binary too, so the doc's "Same in develop?" column has an
/// actual, reproducible answer behind it instead of a claim nobody can
/// re-run.
///
/// `#[ignore]`d: this shells out to a prebuilt release binary rather than
/// exercising this crate's own code in-process, so it does not belong in the
/// default `cargo test` run. To regenerate:
///
/// ```text
/// cargo build --release -p luabox-cli
/// LUABOX_BIN=$PWD/target/release/luabox \
/// LUABOX_DEVELOP_BIN=/path/to/a/release/luabox/built/at/git-merge-base/HEAD/origin-develop \
///   cargo test -p luabox-types --test class_merge_precedence_matrix \
///   -- --ignored regen_merge_matrix_provenance --nocapture
/// ```
///
/// `LUABOX_DEVELOP_BIN` is optional; without it the report says plainly that
/// the develop column was not measured this run, rather than reprinting a
/// stale answer.
#[test]
#[ignore = "shells out to a prebuilt release binary — see this test's doc comment for how to run it"]
fn regen_merge_matrix_provenance() {
    let current_bin = std::env::var("LUABOX_BIN").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/release/luabox").to_string()
    });
    let current_bin = PathBuf::from(current_bin);
    assert!(
        current_bin.is_file(),
        "no luabox binary at {} — build one (`cargo build --release -p luabox-cli`) or set \
         LUABOX_BIN",
        current_bin.display()
    );
    let develop_bin = std::env::var("LUABOX_DEVELOP_BIN").ok().map(PathBuf::from);
    if let Some(bin) = &develop_bin {
        assert!(
            bin.is_file(),
            "LUABOX_DEVELOP_BIN={} is not a file",
            bin.display()
        );
    }

    let mut report = String::new();
    report.push_str("<!-- GENERATED by `regen_merge_matrix_provenance`");
    report.push_str(
        " (crates/luabox-types/tests/class_merge_precedence_matrix.rs) — do not hand-edit. \
         Regenerate per that test's doc comment. -->\n\n# Class-merge precedence matrix — \
         provenance\n\nBacks `docs/03-reference/03-class-merge-precedence.md`. Every row below \
         is one `CELLS` variant from `class_merge_precedence_matrix.rs`, measured against a \
         real `luabox check` subprocess (not this crate's in-process helpers) at the time this \
         file was regenerated.\n\n",
    );
    if develop_bin.is_none() {
        report.push_str(
            "**`LUABOX_DEVELOP_BIN` was not set for this run — the develop column below is \
             `(not measured this run)`, not a claim.**\n\n",
        );
    }
    report.push_str(
        "| Fixture id | Measured (current) | Matches doc `expect`? | Measured (develop) |\n\
         |---|---|---|---|\n",
    );

    let mut mismatches = Vec::new();
    for cell in CELLS {
        for variant in cell.variants {
            let got_current = run_via_binary(&current_bin, "current", variant.files);
            let want: Vec<(String, String)> = variant
                .expect
                .iter()
                .map(|(c, m)| ((*c).to_string(), (*m).to_string()))
                .collect();
            let matches_doc = got_current == want;
            if !matches_doc {
                mismatches.push(format!(
                    "{}/{} [{}] (fixture `{}`): binary measured {got_current:?}, doc/test \
                     `expect` says {want:?}",
                    cell.kind, cell.shape, variant.label, variant.fixture_id
                ));
            }
            let got_develop = develop_bin
                .as_ref()
                .map(|bin| run_via_binary(bin, "develop", variant.files));
            let develop_cell = match &got_develop {
                Some(d) => format!("{d:?}"),
                None => "(not measured this run)".to_string(),
            };
            let _ = writeln!(
                report,
                "| `{}` | {got_current:?} | {} | {develop_cell} |",
                variant.fixture_id,
                if matches_doc { "yes" } else { "**NO**" },
            );
        }
    }

    let doc_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/03-reference/merge-matrix-provenance.md");
    std::fs::write(&doc_path, &report)
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", doc_path.display()));

    assert!(
        mismatches.is_empty(),
        "{} fixture(s) measured through the real binary disagree with this file's own \
         `expect`, i.e. the in-process helpers and the shipped binary disagree — investigate \
         before trusting the regenerated report:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

// --- malformed `---@class` header axis (round 6 review M66, #69) -------
//
// Everything above this line pins *which parent wins* once a class header
// parses. `M66` is a different axis entirely: does a **malformed** header —
// one `luacats::harvest` cannot turn into a well-formed class at all — get a
// diagnostic pointing at the malformation, or is it silently dropped?
//
// It used to be dropped. Every consumer of a `---@class` tag in this crate
// guards on `!c.name.is_empty()`, so a header with no usable name never
// reached the type env and there was nothing to hang a diagnostic on; two of
// the four shapes below were silent everywhere, and a third only ever
// surfaced at a *consumer* of the name the class never got, arbitrarily far
// from the mistake. This test pinned that measured silence, `#[ignore]`d, so
// the day it changed would be visible.
//
// #69 is that day. `check::malformed_class_headers` now reports the header
// itself — `LB0320` for a name that cannot name a class (missing, or not an
// identifier), `LB0321` for an extends-list slot with no name in it — and
// the assertions below are the new, non-silent measurement. The `#[ignore]`
// is gone with the bug: this now covers a rule the repo claims to enforce,
// like every other test in this file, and runs in the default suite.
//
// What each row pins, against lua-language-server 3.13.5 as the oracle:
//
//   shape                    luabox (this head)          luals 3.13.5
//   ---@class : Base         LB0320 at the declaration   `luadoc-miss-class-name`
//                                                         at the declaration, plus
//                                                         `doc-field-no-class` on the
//                                                         `---@field` beneath it
//   ---@class 123abc         LB0320 at the declaration,  identical to the above —
//                            quoting the name back        luals treats an unlexable
//                                                         name token as a missing one
//   ---@class A : P,         LB0321 at the declaration   `luadoc-miss-class-extends-name`
//   (P undeclared)           PLUS the pre-existing        at the comma, PLUS an ordinary
//                            LB0305 on `P`                `undefined-doc-class` on `P`
//   bare ---@class           LB0320 at the declaration,  `luadoc-miss-class-name` +
//   (no name at all)         and the consumer's LB0305    `doc-field-no-class` at the
//                            two lines later still fires  declaration
//
// Both tools now report at the declaration for all four shapes. luabox
// reports the header once and leaves the orphaned `---@field` lines alone
// where luals adds a `doc-field-no-class` per field — one mistake, one
// diagnostic — a message-count difference, not a silence.
//
// The `A : P,` row is the one that did NOT change: it was never silent, and
// its `LB0305` fires on the ordinary undeclared-parent path, independent of
// the trailing comma. What #69 added there is the `LB0321` beside it, so the
// malformed *syntax* is now reported as well as the undeclared *name* —
// exactly the pair luals reports. Its control (`---@class A : P`, no comma)
// lives in `tests/malformed_class_headers.rs` and stays LB0305-only, so a
// rule that fired on the shared undeclared parent rather than on the comma
// fails there.
#[test]
fn malformed_class_headers_m66() {
    type MalformedCase = (
        &'static str,
        &'static str,
        &'static [(&'static str, &'static str)],
    );
    let cases: &[MalformedCase] = &[
        (
            "no name, colon parent (`---@class : Base`)",
            r"
---@class : Base
---@field x number
local M = {}
return M
",
            &[("LB0320", "`---@class` is missing a class name")],
        ),
        (
            "non-identifier name (`---@class 123abc`)",
            r"
---@class 123abc
---@field x number
local M = {}
return M
",
            &[("LB0320", "`123abc` is not a valid class name")],
        ),
        (
            "trailing comma in the extends list (`---@class A : P,`)",
            r"
---@class A : P,
---@field x number
local M = {}
return M
",
            &[
                (
                    "LB0321",
                    "`---@class` extends list has an entry that is not a class name",
                ),
                ("LB0305", "unknown type name `P` in annotation"),
            ],
        ),
        (
            "bare, no name at all (`---@class`), consumer two lines later",
            r"
---@class
---@field x number
local M = {}
---@type A
local a
print(a.x)
return M
",
            &[
                ("LB0320", "`---@class` is missing a class name"),
                ("LB0305", "unknown type name `A` in annotation"),
            ],
        ),
    ];
    let mut failures = Vec::new();
    for (label, src, expect) in cases {
        let got: Vec<(String, String)> = support::check_self(src)
            .into_iter()
            .map(|d| (d.code.to_string(), d.message))
            .collect();
        let want: Vec<(String, String)> = expect
            .iter()
            .map(|(c, m)| ((*c).to_string(), (*m).to_string()))
            .collect();
        if got != want {
            failures.push(format!("{label}\n  got:  {got:?}\n  want: {want:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} malformed-header shape(s) no longer report at the declaration — #69 regressed, \
         and a malformed `---@class` is silently dropped again:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
