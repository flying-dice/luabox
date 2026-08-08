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
//! Every fixture's Lua source and expected `(code, message)` diagnostics are
//! copied verbatim from the measurement corpus
//! (`merge-matrix.md`'s `fixtures*.json`/`results*.json`, produced by
//! running `luabox check --format json` against this worktree before the
//! refactor) — this file asserts the refactor reproduces exactly what was
//! measured, not a re-derived guess at what it should produce.
//!
//! A cell with more than one `variant` (labelled `swapped`/`reversed` in the
//! matrix doc) is the same matrix cell confirmed in both directions — e.g.
//! `field-D-unrelated-parents` and its `-swapped` twin both assert the
//! **last**-listed parent wins, with the parents' declaration order
//! reversed, so the winner is provably order-dependent rather than a
//! coincidence of which type happened to be `string`.

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
    /// The fixture id in `docs/03-reference/03-class-merge-precedence.md` /
    /// `merge-matrix.md`, so a failure can be cross-checked against the doc
    /// by name.
    fixture_id: &'static str,
    files: &'static [(&'static str, &'static str)],
    /// The exact `(code, message)` diagnostics `current` produced when this
    /// corpus was measured — what this refactor must still produce.
    expect: &'static [(&'static str, &'static str)],
}

/// One matrix cell: a member kind crossed with an arrival shape.
struct Cell {
    kind: &'static str,
    shape: &'static str,
    /// The matrix doc's own words for who wins — read, not asserted; the
    /// assertion is `Variant::expect`. Kept so a failure's panic message
    /// names the rule that broke, not just the code that changed.
    winner: &'static str,
    variants: &'static [Variant],
}

const CELLS: &[Cell] = &[
    Cell {
        kind: "field",
        shape: "single",
        winner: "resolves (baseline)",
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
        kind: "method",
        shape: "diamond-conflicting",
        winner: "N/A (no type param to bind two ways). This fixture is actually an unrelated-parents shape wearing this slot (A's inherited Base attachment vs. B's own field declaration are two DIFFERENT classes' contributions, not one class's own declaration-over-attachment): A, first-listed, wins (A1, luals parity) — B's `m` declaration never even gets visited, matching luals's key-locking (compiler.lua:369-375/424), not the within-class method-G rule",
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
        winner: "first declaration, silently (no LB0311-style warning)",
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
            expect: &[("LB0300", "type mismatch: expected `string`, found `number`")],
        }],
    },
    Cell {
        kind: "indexer",
        shape: "dup-cross-file",
        winner: "first-processed file",
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
        variants: &[],
    },
    Cell {
        kind: "typeparam",
        shape: "dup-same-file-renamed",
        winner: "positional unification: slot 0 is one variable",
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
        variants: &[],
    },
    Cell {
        kind: "visibility",
        shape: "single",
        winner: "enforces (baseline)",
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
    // floor while the real count was 52, which meant an entire `Cell` could
    // be deleted — seven variants' worth — with this suite still green. A
    // merge-gate finder proved it by removing the row that pins carrier
    // methods resolving to the first-listed parent and watching both tests
    // pass. The floor is now the exact count, so a row that disappears is a
    // failure rather than a smaller number nobody reads.
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
/// [`CELLS`], and vice versa.
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
#[test]
fn the_published_matrix_and_the_test_table_name_the_same_fixtures() {
    let doc = include_str!("../../../docs/03-reference/03-class-merge-precedence.md");
    let mut documented: Vec<String> = Vec::new();
    for line in doc.lines() {
        // Fixture ids live in the last column of each table row, in backticks,
        // shaped `<kind>-<letter>-<slug>` (plus optional `-swapped` twins
        // written as a bare `` (+ `-swapped`) `` suffix, which the row's own
        // `variants` cover and which are not separate ids).
        if !line.starts_with('|') {
            continue;
        }
        for token in line.split('`') {
            let looks_like_id = token
                .split_once('-')
                .and_then(|(_, rest)| rest.split_once('-'))
                .is_some_and(|(letter, _)| {
                    letter.len() == 1 && letter.chars().all(|c| c.is_ascii_uppercase())
                });
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

    // Checked in one direction deliberately: every fixture id the published
    // table names must be measured here. That is the direction that catches
    // a deleted row — the doc still advertises the cell while nothing pins
    // it, which the exact-count assertion above cannot see on its own.
    //
    // The reverse is not asserted, because the table writes a swapped twin
    // as a `(+ `-swapped`)` suffix on its sibling's id rather than as its own
    // entry, and names a few finding-specific repro fixtures in prose.
    // Requiring every tested id to appear verbatim would fail on that
    // formatting rather than on drift; the exact count bounds the test side.
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
