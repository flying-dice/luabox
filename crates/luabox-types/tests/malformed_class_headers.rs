//! `LB0320` / `LB0321`: a `---@class` header that cannot become the class it
//! names is reported **at the declaration** (round 6 review M66, #69) — luals'
//! `luadoc-miss-class-name` / `luadoc-miss-class-extends-name`.
//!
//! Before this, a header the harvester could not turn into a class was simply
//! dropped: every consumer of a `---@class` tag guards on
//! `!c.name.is_empty()`, so three of the four malformed shapes below were
//! silent at the declaration, and the fourth (`A : P,`) only ever surfaced
//! through the ordinary undeclared-parent path, which says nothing about the
//! malformation itself. The failure then appeared — if at all — at a *use*
//! site arbitrarily far from the mistake, or nowhere.
//!
//! Every malformed case here is paired with a control that differs from it in
//! exactly one respect (a name where the malformed shape has none, the same
//! parent list without the trailing comma), so a rule that fired on the shared
//! part rather than on the malformation fails its control.

use luabox_diag::{Diagnostic, Severity};
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::{Strictness, check_file};

fn check(source: &str, strictness: Strictness) -> Vec<Diagnostic> {
    let parsed = parse(source, Dialect::Lua54);
    assert_eq!(
        parsed.errors(),
        &[],
        "fixture must parse cleanly:\n{source}"
    );
    check_file(&parsed, "test.lua", strictness, Dialect::Lua54)
}

fn codes(source: &str, strictness: Strictness) -> Vec<String> {
    let mut out: Vec<String> = check(source, strictness)
        .iter()
        .map(|d| d.code.to_string())
        .collect();
    out.sort();
    out
}

/// The diagnostics carrying one code, so a fixture that also raises the
/// ordinary undeclared-name finding (`LB0305`) can still assert this rule's
/// own message and span precisely.
fn only(source: &str, code: &str) -> Vec<Diagnostic> {
    check(source, Strictness::Strict)
        .into_iter()
        .filter(|d| d.code.to_string() == code)
        .collect()
}

/// The 1-based line the diagnostic's primary label starts on — what "at the
/// declaration" means, when the whole point of the rule is *where* it fires.
fn primary_line(source: &str, diag: &Diagnostic) -> usize {
    let start = diag
        .labels
        .first()
        .map_or(0, |label| label.span.range.start);
    source
        .get(..start)
        .map_or(0, |before| before.matches('\n').count() + 1)
}

const MISSING_NAME_COLON_PARENT: &str = "\
---@class Base
local B = {}

---@class : Base
---@field x number
local M = {}
return { B, M }
";

const NON_IDENTIFIER_NAME: &str = "\
---@class 123abc
---@field x number
local M = {}
return M
";

const TRAILING_COMMA_EXTENDS: &str = "\
---@class A : P,
---@field x number
local M = {}
return M
";

const BARE_CLASS: &str = "\
---@class
---@field x number
local M = {}
return M
";

#[test]
fn a_class_header_with_no_name_is_reported_at_the_declaration() {
    let diags = only(MISSING_NAME_COLON_PARENT, "LB0320");
    assert_eq!(diags.len(), 1, "expected exactly one LB0320: {diags:?}");
    assert_eq!(
        primary_line(MISSING_NAME_COLON_PARENT, &diags[0]),
        4,
        "LB0320 must point at the `---@class : Base` line: {diags:?}"
    );
    assert!(
        diags[0].message.contains("class name"),
        "the message must name what is missing: {}",
        diags[0].message
    );
    // A parent list is not an extends-list malformation: `Base` is a perfectly
    // good parent name, it is just in the wrong place. Only LB0320 fires.
    assert_eq!(
        codes(MISSING_NAME_COLON_PARENT, Strictness::Strict),
        vec!["LB0320".to_string()],
    );
}

#[test]
fn a_bare_class_header_is_reported_at_the_declaration() {
    let diags = only(BARE_CLASS, "LB0320");
    assert_eq!(diags.len(), 1, "expected exactly one LB0320: {diags:?}");
    assert_eq!(primary_line(BARE_CLASS, &diags[0]), 1, "{diags:?}");
    // The whole point of #69: the old signal was an LB0305 at a *consumer* of
    // the name the class never got, two lines away or two files away. There is
    // no consumer here at all, and the header is still reported.
    assert_eq!(
        codes(BARE_CLASS, Strictness::Strict),
        vec!["LB0320".to_string()],
    );
}

#[test]
fn a_class_name_that_is_not_an_identifier_is_reported_at_the_declaration() {
    let diags = only(NON_IDENTIFIER_NAME, "LB0320");
    assert_eq!(diags.len(), 1, "expected exactly one LB0320: {diags:?}");
    assert_eq!(primary_line(NON_IDENTIFIER_NAME, &diags[0]), 1, "{diags:?}");
    assert!(
        diags[0].message.contains("123abc"),
        "the message must quote the offending name back: {}",
        diags[0].message
    );
}

/// The malformation and the undeclared parent are two findings, and luals
/// reports both: `luadoc-miss-class-extends-name` at the comma *plus*
/// `undefined-doc-class` on `P`. The trailing comma must not swallow the
/// `LB0305` that was already correct, and `A` itself is named fine — no
/// LB0320.
#[test]
fn a_trailing_comma_in_the_extends_list_is_reported_beside_the_unknown_parent() {
    assert_eq!(
        codes(TRAILING_COMMA_EXTENDS, Strictness::Strict),
        vec!["LB0305".to_string(), "LB0321".to_string()],
    );
    let diags = only(TRAILING_COMMA_EXTENDS, "LB0321");
    assert_eq!(diags.len(), 1, "expected exactly one LB0321: {diags:?}");
    assert_eq!(
        primary_line(TRAILING_COMMA_EXTENDS, &diags[0]),
        1,
        "{diags:?}"
    );
}

/// One malformed header is one finding per code, however many bad entries the
/// extends list has — the same attribution luals gives it.
#[test]
fn several_bad_entries_in_one_extends_list_are_one_finding() {
    let src = "\
---@class Base
local B = {}

---@class A : Base,, ,
---@field x number
local M = {}
return { B, M }
";
    assert_eq!(codes(src, Strictness::Strict), vec!["LB0321".to_string()]);
}

/// A header can be wrong in both ways at once — no name AND a bad extends
/// entry — and each mistake has its own fix, so each gets its own diagnostic.
#[test]
fn a_nameless_header_with_a_bad_extends_entry_reports_both() {
    let src = "\
---@class :
---@field x number
local M = {}
return M
";
    assert_eq!(
        codes(src, Strictness::Strict),
        vec!["LB0320".to_string(), "LB0321".to_string()],
    );
}

/// `TypeExprKind::Error` is the type parser's universal recovery node, and a
/// stray separator is only half of what reaches it: a token that is present
/// and simply is not a name lands there too, with a *non-empty* span. LB0321
/// must cover those and its message must stay true on them — "missing a
/// parent class name" would be a lie here, and this is an error under
/// `strict = true`.
#[test]
fn an_extends_entry_that_is_not_a_name_is_reported_without_claiming_it_is_absent() {
    for (src, declaration_line) in [
        (
            "\
---@class A : ?
local M = {}
return M
",
            1,
        ),
        (
            "\
---@class A : %
local M = {}
return M
",
            1,
        ),
        (
            "\
---@class Base
local B = {}

---@class A : Base,, Q
local M = {}
return { B, M }
",
            4,
        ),
    ] {
        let diags = only(src, "LB0321");
        assert_eq!(diags.len(), 1, "expected exactly one LB0321: {diags:?}");
        assert_eq!(primary_line(src, &diags[0]), declaration_line, "{diags:?}");
        assert!(
            !diags[0].message.contains("missing"),
            "a token IS present here — the message must not claim one is absent: {}",
            diags[0].message
        );
    }
}

/// The recovery node is not always at the root of the entry. A postfix or
/// grouping operator is applied to whatever it follows, so an unreadable token
/// under a `?`, a `[]`, a parenthesis or a union arrives wrapped — and a check
/// that matched only the entry's own `kind` reported nothing on any of them,
/// which is the same silence #69 exists to remove, one level down.
#[test]
fn an_unreadable_entry_is_found_however_deeply_the_grammar_wraps_it() {
    for header in [
        "---@class A : Base,, ?", // Optional(Error) — the shape that caught this
        "---@class A : ?[]",      // Array(Error)
        "---@class A : (?)",      // Paren(Error)
        "---@class A : Base|?",   // Union([Named, Error])
        "---@class A : { x: ? }", // Table field
        "---@class A : fun(): ?", // Function return
    ] {
        let src = format!(
            "\
---@class Base
local B = {{}}

{header}
local M = {{}}
return {{ B, M }}
"
        );
        let diags = only(&src, "LB0321");
        assert_eq!(
            diags.len(),
            1,
            "`{header}` must raise exactly one LB0321: {diags:?}"
        );
        assert_eq!(primary_line(&src, &diags[0]), 4, "`{header}`: {diags:?}");
    }
}

/// A parent that is a legitimate non-name type expression is not this finding:
/// the extends list accepts what the type grammar accepts, and only an entry
/// the parser could not read at all is malformed.
#[test]
fn a_parent_the_type_grammar_accepts_is_not_reported() {
    for src in [
        "\
---@class P
local P = {}

---@class A : (P)
local M = {}
return { P, M }
",
        "\
---@class A : { x: number }
local M = {}
return M
",
        // A generic parent that reads cleanly end to end — clean before and
        // after the stop-at-a-name rule below, which is why that rule needs
        // the `P<?>` case in
        // `an_unreadable_type_argument_does_not_make_the_parent_unreadable`
        // rather than this one.
        "\
---@class P
local P = {}

---@class A : P<number>
local M = {}
return { P, M }
",
    ] {
        assert!(
            !codes(src, Strictness::Strict).contains(&"LB0321".to_string()),
            "a parseable parent must not raise LB0321:\n{src}"
        );
    }
}

/// The counterpart to the wrapping shapes above, and the arm they do not
/// cover: an unreadable *type argument* of a parent whose name reads fine.
///
/// The three fixtures differ in one respect each. `: Base<?>` must behave
/// like the well-formed `: Base` — the parent is resolved and its members are
/// inherited, so LB0300 fires on the missing member and LB0321 must not fire
/// at all. It must *not* behave like `: ?`, where the entry really is
/// discarded and `b` is never inherited. Reporting the middle case as "an
/// entry that is not a class name" told the user, at `Severity::Error`, to
/// delete a parent that works (review of !2, `e751bb0`).
#[test]
fn an_unreadable_type_argument_does_not_make_the_parent_unreadable() {
    fn fixture(parents: &str) -> String {
        format!(
            "\
---@class Base
---@field b number
local B = {{}}

---@class A : {parents}
local M = {{}}
return {{ B, M }}
"
        )
    }

    let generic_argument = fixture("Base<?>");
    assert_eq!(
        codes(&generic_argument, Strictness::Strict),
        codes(&fixture("Base"), Strictness::Strict),
        "`Base<?>` must be diagnosed exactly like the well-formed `Base`:\n{generic_argument}"
    );
    assert_eq!(
        codes(&generic_argument, Strictness::Strict),
        vec!["LB0300".to_string()],
        "the parent must still be wired — ancestry is enforced, so the missing \
         member is the only finding: {:?}",
        check(&generic_argument, Strictness::Strict),
    );

    // The control on the other side, and the one this shape is NOT: an entry
    // the parser could not read at all really is discarded, so `b` is never
    // inherited — no LB0300 about the missing member, an LB0306 at the read
    // of it instead, and this rule fires. `an_extends_entry_that_is_not_a_...`
    // pins that finding's own message and span; what matters here is that the
    // two shapes measure differently at all.
    assert_ne!(
        codes(&generic_argument, Strictness::Strict),
        codes(&fixture("?"), Strictness::Strict),
        "`Base<?>` must not be diagnosed like an entry that is genuinely \
         unreadable — the parent is wired in one and dropped in the other"
    );
}

/// The shape that decides which rule LB0321 is actually following: a parent
/// name that reads fine, an unreadable type argument, and a *wrapper* around
/// the pair — `Base<?>?`, `Base<?>[]`, `(Base<?>)`.
///
/// The stop-at-a-name rule was first written to look through `?`, `[]` and
/// parens on the premise that a name under a wrapper is still the parent. It
/// is not. The controls below measure that directly: `: Base` inherits `b`
/// (LB0300, ancestry enforced), while `: Base?`, `: Base[]`, `: (Base)` and
/// `: Base|Base` each leave `A` with no members at all — LB0306 at the read,
/// byte-identical to a header with no extends list. A wrapped entry
/// contributes no parent, so the note ("the entry is ignored") holds of it,
/// and an unreadable token inside one is this finding. Looking through the
/// wrapper silenced all three shapes, which `1b544a5` reported (review of !2,
/// `6f03acd`).
#[test]
fn a_wrapper_around_a_name_is_not_a_parent_so_an_error_under_one_is_reported() {
    /// The fixture with a *read* of the inherited member: LB0300 means the
    /// parent was wired and ancestry is enforced, LB0306 means it was dropped.
    /// Without the read, the two are indistinguishable at the header.
    fn fixture(extends: &str) -> String {
        format!(
            "\
---@class Base
---@field b number
local B = {{}}

---@class A{extends}
local M = {{}}

---@type A
local a
print(a.b)
return {{ B, M, a }}
"
        )
    }

    let dropped = codes(&fixture(""), Strictness::Strict);
    assert_eq!(
        dropped,
        vec!["LB0306".to_string()],
        "no extends list: nothing is inherited, so the read of `b` is undefined",
    );
    assert_eq!(
        codes(&fixture(" : Base"), Strictness::Strict),
        vec!["LB0300".to_string()],
        "the well-formed control: the parent is wired and `b` is inherited",
    );

    // One variable per row: the same readable name, one wrapper applied to it.
    // Every one of them measures like *no extends list*, not like `: Base`.
    for wrapped in [" : Base?", " : Base[]", " : (Base)", " : Base|Base"] {
        assert_eq!(
            codes(&fixture(wrapped), Strictness::Strict),
            dropped,
            "`{wrapped}` must measure like no extends list — a wrapper \
             contributes no parent, so the name inside it is not one",
        );
    }

    // Therefore: wrapper + unreadable type argument is an entry that names no
    // parent *and* contains a token the parser could not read — exactly what
    // LB0321 is for. `Base|?`, which the `_` arm already reports, is the
    // control they must match.
    let union_control = codes(&fixture(" : Base|?"), Strictness::Strict);
    assert_eq!(
        union_control,
        vec!["LB0306".to_string(), "LB0321".to_string()],
    );
    for wrapped in [" : Base<?>?", " : Base<?>[]", " : (Base<?>)"] {
        assert_eq!(
            codes(&fixture(wrapped), Strictness::Strict),
            union_control,
            "`{wrapped}` drops its parent and carries an unreadable token — \
             it must be reported, like the union shape",
        );
    }

    // And the rule it must not undo: no wrapper, so the name at the root IS
    // the parent, and the unreadable argument is a separate axis.
    assert_eq!(
        codes(&fixture(" : Base<?>"), Strictness::Strict),
        vec!["LB0300".to_string()],
        "`Base<?>` heads with a name at the root: parent wired, no LB0321",
    );
}

/// One variable per control: the same header, the same undeclared parent, no
/// trailing comma. `LB0305` stays; `LB0321` must not appear.
#[test]
fn an_extends_list_without_the_trailing_comma_raises_only_the_unknown_parent() {
    let control = "\
---@class A : P
---@field x number
local M = {}
return M
";
    assert_eq!(
        codes(control, Strictness::Strict),
        vec!["LB0305".to_string()],
    );
}

/// One variable per control: every malformed shape above, given a name that
/// *is* an identifier, is clean.
#[test]
fn well_formed_class_headers_stay_clean() {
    for source in [
        "\
---@class Base
local B = {}

---@class Derived : Base
---@field x number
local M = {}
return { B, M }
",
        "\
---@class abc123
---@field x number
local M = {}
return M
",
        "\
---@class geometry.Point
---@field x number
local M = {}
return M
",
        "\
---@class (exact) Boxed<T>
---@field value T
local M = {}
return M
",
        "\
---@class _Private
---@field x number
local M = {}
return M
",
        // Non-ASCII is a name character in luacats' own grammar, first
        // position included — this rule must not quietly become ASCII-only.
        "\
---@class Größe
---@field x number
local M = {}
return M
",
    ] {
        assert_eq!(
            codes(source, Strictness::Strict),
            Vec::<String>::new(),
            "well-formed header must be clean:\n{source}"
        );
    }
}

/// The strictness ladder, like every other `LB03xx`: an error under
/// `strict = true`, a warning below it, and nothing at all under `None`.
#[test]
fn severity_follows_the_strictness_ladder() {
    for source in [BARE_CLASS, TRAILING_COMMA_EXTENDS] {
        let strict = check(source, Strictness::Strict);
        let malformed: Vec<&Diagnostic> = strict
            .iter()
            .filter(|d| matches!(d.code.to_string().as_str(), "LB0320" | "LB0321"))
            .collect();
        assert_eq!(malformed.len(), 1, "{strict:?}");
        assert_eq!(malformed[0].severity, Severity::Error);

        let warn = check(source, Strictness::Warn);
        let malformed: Vec<&Diagnostic> = warn
            .iter()
            .filter(|d| matches!(d.code.to_string().as_str(), "LB0320" | "LB0321"))
            .collect();
        assert_eq!(malformed.len(), 1, "{warn:?}");
        assert_eq!(malformed[0].severity, Severity::Warning);

        assert_eq!(codes(source, Strictness::None), Vec::<String>::new());
    }
}

/// luals' own rule names, so a `---@diagnostic` comment written for luals
/// silences the luabox finding unchanged.
#[test]
fn each_code_is_suppressed_by_the_luals_rule_name_it_borrows() {
    for (rule, fixture) in [
        ("luadoc-miss-class-name", BARE_CLASS),
        ("luadoc-miss-class-extends-name", TRAILING_COMMA_EXTENDS),
    ] {
        let suppressed = format!("---@diagnostic disable: {rule}\n{fixture}");
        let got = codes(&suppressed, Strictness::Strict);
        assert!(
            !got.iter().any(|c| c == "LB0320" || c == "LB0321"),
            "`{rule}` did not silence its own finding: {got:?}"
        );
    }
}
