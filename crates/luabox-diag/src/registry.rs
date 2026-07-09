//! The static registry of known diagnostic codes and their explain pages.
//!
//! Each entry carries a short `title` and a Markdown `explain` page surfaced
//! by `luabox explain <code>`. The `LB2xxx` shape codes (SHAPES.md §5) are
//! *reserved* here — the checker that emits them lands in P1 — so their
//! explain text documents the condition and points at the shape spec.

use crate::code::Code;

/// A registry entry: a code, its title, and its Markdown explain page.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    /// The code this entry documents.
    pub code: Code,
    /// A short human title (rustc calls this the message template).
    pub title: &'static str,
    /// The full explain page, in Markdown.
    pub explain: &'static str,
}

/// Every known diagnostic code, in ascending order.
static REGISTRY: &[Entry] = &[
    Entry {
        code: Code::new(1),
        title: "syntax error",
        explain: LB0001,
    },
    Entry {
        code: Code::new(1001),
        title: "unknown edition",
        explain: LB1001,
    },
    Entry {
        code: Code::new(2001),
        title: "missing non-optional field on shape-bound literal",
        explain: LB2001,
    },
    Entry {
        code: Code::new(2002),
        title: "unknown key on sealed shape",
        explain: LB2002,
    },
    Entry {
        code: Code::new(2003),
        title: "incomplete `---@impl`",
        explain: LB2003,
    },
    Entry {
        code: Code::new(2004),
        title: "impl signature mismatch",
        explain: LB2004,
    },
    Entry {
        code: Code::new(2005),
        title: "unresolved `---@use` module",
        explain: LB2005,
    },
    Entry {
        code: Code::new(2006),
        title: "`---@struct` names an undeclared struct",
        explain: LB2006,
    },
    Entry {
        code: Code::new(2007),
        title: "generic bound unsatisfied",
        explain: LB2007,
    },
    Entry {
        code: Code::new(2008),
        title: "supertrait conformance missing",
        explain: LB2008,
    },
    Entry {
        code: Code::new(2010),
        title: "body in `.lb` file",
        explain: LB2010,
    },
];

/// Look up the explain entry for a code, if it is registered.
#[must_use]
pub fn explain(code: &Code) -> Option<&'static Entry> {
    REGISTRY.iter().find(|entry| entry.code == *code)
}

/// Every registered entry, ascending by code. Useful for `explain --list`
/// style commands and completeness checks.
#[must_use]
pub fn all() -> &'static [Entry] {
    REGISTRY
}

const LB0001: &str = "\
# LB0001: syntax error

The parser reached a token that is not valid at this position, or hit the end
of the file while a construct was still open.

Common causes:

- A missing `end`, `)`, `}`, or `then`.
- A stray keyword or operator where an expression or statement was expected.
- Mismatched string or long-bracket / comment delimiters.

luabox parses with a lossless, error-resilient parser: it keeps going past the
error so later diagnostics and editor features still work, but the offending
region is reported here.

Fix the highlighted token and re-run. If the construct is only valid in a Lua
dialect newer than your `edition`, either raise the edition in `luabox.toml`
or rewrite it for the edition you target.
";

const LB1001: &str = "\
# LB1001: unknown edition

`edition` in `luabox.toml` (or the `--edition` flag) names the Lua dialect you
*write*. It must be one of:

- `5.1`
- `5.2`
- `5.3`
- `5.4`
- `luajit`

The value supplied is none of these.

Note: **Luau is intentionally out of scope** (SPEC.md §1). It is a separate
typed paradigm with its own owner and toolchain; luabox's typed story is the
`.lb` shape DSL layered over untyped Lua, so `luau` is not a valid edition.
";

const LB2001: &str = "\
# LB2001: missing non-optional field on shape-bound literal

A table bound to a struct with `---@struct` (or produced by
`setmetatable(literal, Carrier)`) omits a field the struct declares as
non-optional. Structs are *sealed*: every non-`?` field must be present.

Add the missing field, or mark it optional in the `.lb` declaration
(`field: T?`).

Reserved code — the shape checker that emits it lands in P1. See SHAPES.md §5.
";

const LB2002: &str = "\
# LB2002: unknown key on sealed shape

A read or write used a key the struct does not declare. Sealed structs reject
unknown keys. To allow extra keys, open the struct with `..` in its `.lb`
declaration (extras then type as `unknown`).

Reserved code — the shape checker that emits it lands in P1. See SHAPES.md §5.
";

const LB2003: &str = "\
# LB2003: incomplete `---@impl`

An `---@impl Trait for Struct` carrier does not define every function the trait
requires. The diagnostic lists the missing functions; a fix-it can generate
stubs for them.

Reserved code — the shape checker that emits it lands in P1. See SHAPES.md §5.
";

const LB2004: &str = "\
# LB2004: impl signature mismatch

A function on an `---@impl` carrier does not match the trait signature:
parameters are contravariant, returns covariant, and a `:` vs `.` receiver
must agree with `self`. Both the trait and the implementation spans are shown.

Reserved code — the shape checker that emits it lands in P1. See SHAPES.md §5.
";

const LB2005: &str = "\
# LB2005: unresolved `---@use` module

A `---@use <module>` (or a `use` inside a `.lb` file) names a shape module that
could not be resolved. Resolution tries, in order: a sibling `<name>.lb`, the
`[types] shape-paths` directories, then dependency-exported shapes (SHAPES.md
§6). Same-tier ambiguity is also an error.

Reserved code — the shape checker that emits it lands in P1. See SHAPES.md §5.
";

const LB2006: &str = "\
# LB2006: `---@struct` names an undeclared struct

A `---@struct <Name>` annotation refers to a struct that no in-scope shape
module declares. Check the name and that the module is imported with `---@use`.

Reserved code — the shape checker that emits it lands in P1. See SHAPES.md §5.
";

const LB2007: &str = "\
# LB2007: generic bound unsatisfied

A generic shape was instantiated with a type argument that does not satisfy the
declared bound. Generics are monomorphised per use site and bound violations
are reported at the call, rustc-style.

Reserved code — the shape checker that emits it lands in P1. See SHAPES.md §5.
";

const LB2008: &str = "\
# LB2008: supertrait conformance missing

An `impl` for a trait with supertraits (`trait Drawable: Shape`) requires the
carrier to also conform to every supertrait. Add the missing `---@impl` for the
supertrait on the same carrier.

Reserved code — the shape checker that emits it lands in P1. See SHAPES.md §5.
";

const LB2010: &str = "\
# LB2010: body in `.lb` file

A `.lb` file contains a body or expression. Shape files are declaration-only:
no bodies, no expressions. Implementations live in `.lua` and bind with
`---@impl`.

Reserved code — the shape parser that emits it lands in P1. See SHAPES.md §5.
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_codes_are_all_present() {
        for raw in [
            "LB0001", "LB1001", "LB2001", "LB2002", "LB2003", "LB2004", "LB2005", "LB2006",
            "LB2007", "LB2008", "LB2010",
        ] {
            let code: Code = raw.parse().unwrap();
            assert!(explain(&code).is_some(), "{raw} missing from registry");
        }
    }

    #[test]
    fn every_entry_has_nonempty_title_and_explain() {
        for entry in all() {
            assert!(!entry.title.is_empty(), "{} has empty title", entry.code);
            assert!(
                entry.explain.trim().len() > 20,
                "{} has a suspiciously short explain page",
                entry.code
            );
            // Explain pages are Markdown headed by the code itself.
            assert!(
                entry.explain.contains(&entry.code.to_string()),
                "{} explain page does not mention its own code",
                entry.code
            );
        }
    }

    #[test]
    fn registry_is_sorted_and_unique() {
        for pair in all().windows(2) {
            assert!(
                pair[0].code < pair[1].code,
                "registry not strictly ascending at {} / {}",
                pair[0].code,
                pair[1].code
            );
        }
    }

    #[test]
    fn unknown_code_is_none() {
        let code: Code = "LB9999".parse().unwrap();
        assert!(explain(&code).is_none());
    }
}
