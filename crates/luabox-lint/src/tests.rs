// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]
//! Unit tests: each rule (firing / non-firing / type-informed negative),
//! suppression, config precedence, fix idempotence, and a corpus sweep.

use luabox_diag::Severity;
use luabox_syntax::Dialect;

use crate::{Level, LintConfig, LintOutcome, Tier, apply_fixes, lint_source, rules, tier_default};

/// The Lua 5.4 stdlib's known-global names — every test lints against this
/// baseline, exactly as `luabox lint` does via `luabox_types::stdlib_defs`
/// (ticket #103): otherwise every fixture calling `print`/`string.format`/
/// `setmetatable`/... would spuriously trip `undefined-global`.
fn lint(source: &str, config: &LintConfig) -> LintOutcome {
    let known = luabox_types::stdlib_defs(Dialect::Lua54).global_names();
    lint_source("test.lua", source, Dialect::Lua54, config, known)
}

/// Like [`lint`] but with an explicit known-globals set, for
/// `undefined-global`'s own tests (which want to control the baseline
/// directly rather than pull in the whole stdlib).
fn lint_with_globals(
    source: &str,
    config: &LintConfig,
    known_globals: &std::collections::HashSet<String>,
) -> LintOutcome {
    lint_source("test.lua", source, Dialect::Lua54, config, known_globals)
}

/// The set of diagnostic codes emitted, sorted (deduped).
fn codes(source: &str, config: &LintConfig) -> Vec<String> {
    let mut c: Vec<String> = lint(source, config)
        .diagnostics
        .iter()
        .map(|d| d.code.to_string())
        .collect();
    c.sort();
    c.dedup();
    c
}

/// Codes under the default configuration.
fn default_codes(source: &str) -> Vec<String> {
    codes(source, &LintConfig::new())
}

fn has(source: &str, config: &LintConfig, code: &str) -> bool {
    codes(source, config).iter().any(|c| c == code)
}

// --- unused-local (LB0501, style) ------------------------------------------

#[test]
fn unused_local_fires() {
    assert!(default_codes("local x = 1\n").contains(&"LB0501".to_owned()));
}

#[test]
fn read_local_is_clean() {
    assert_eq!(
        default_codes("local x = 1\nreturn x\n"),
        Vec::<String>::new()
    );
}

#[test]
fn underscore_local_is_exempt() {
    assert_eq!(default_codes("local _x = 1\n"), Vec::<String>::new());
}

#[test]
fn local_used_only_in_closure_is_read() {
    let src = "local n = 1\nlocal function f() return n end\nreturn f\n";
    assert_eq!(default_codes(src), Vec::<String>::new());
}

#[test]
fn unused_local_function_fires() {
    // `f` is never called; `g` is exported.
    let src = "local function f() return 1 end\nlocal g = 2\nreturn g\n";
    assert!(default_codes(src).contains(&"LB0501".to_owned()));
}

// --- unused-param (LB0502, pedantic) ---------------------------------------

fn pedantic() -> LintConfig {
    let mut c = LintConfig::new();
    assert!(c.set_tier("pedantic", "warn"));
    c
}

#[test]
fn unused_param_is_off_by_default() {
    assert!(
        !default_codes("local function f(a) return 1 end\nreturn f\n")
            .contains(&"LB0502".to_owned())
    );
}

#[test]
fn unused_param_fires_when_enabled() {
    let src = "local function f(a) return 1 end\nreturn f\n";
    assert!(has(src, &pedantic(), "LB0502"));
}

#[test]
fn self_param_is_exempt() {
    let src = "local t = {}\nfunction t:m() return 1 end\nreturn t\n";
    assert!(!has(src, &pedantic(), "LB0502"));
}

#[test]
fn used_param_is_clean() {
    let src = "local function f(a) return a end\nreturn f\n";
    assert!(!has(src, &pedantic(), "LB0502"));
}

// --- shadowed-local (LB0503, suspicious) -----------------------------------

#[test]
fn shadow_across_scope_fires() {
    let src = "local value = 1\ndo\n  local value = 2\n  print(value)\nend\nprint(value)\n";
    assert!(default_codes(src).contains(&"LB0503".to_owned()));
}

#[test]
fn same_block_relocal_is_allowed() {
    let src = "local x = 1\nlocal x = x + 1\nprint(x)\n";
    assert!(!default_codes(src).contains(&"LB0503".to_owned()));
}

#[test]
fn shadowing_a_parameter_fires() {
    let src = "local function f(v)\n  do\n    local v = 2\n    print(v)\n  end\n  return v\nend\nreturn f\n";
    assert!(has(src, &LintConfig::new(), "LB0503"));
}

/// The `shadowed-local` findings of `src`, as `(message, secondary label)`
/// pairs in report order.
fn shadow_findings(src: &str) -> Vec<(String, String)> {
    lint(src, &LintConfig::new())
        .diagnostics
        .into_iter()
        .filter(|d| d.code.to_string() == "LB0503")
        .map(|d| {
            let secondary = d
                .labels
                .iter()
                .find(|l| !l.primary)
                .map_or_else(String::new, |l| l.message.clone());
            (d.message, secondary)
        })
        .collect()
}

#[test]
fn shadow_diagnostic_names_both_declarations() {
    let src = "local value = 1\ndo\n  local value = 2\n  print(value)\nend\nprint(value)\n";
    assert_eq!(
        shadow_findings(src),
        vec![(
            "`value` shadows a binding from an enclosing scope".to_owned(),
            "outer `value` declared here".to_owned(),
        )]
    );
}

#[test]
fn shadowing_fires_in_every_kind_of_nested_scope() {
    // if / else, while, repeat-until, numeric for (with a step) and generic
    // for each open a scope the walker must descend into.
    for (label, src) in [
        (
            "if branch",
            "local v = 1\nif c then local v = 2 print(v) end\nprint(v)\n",
        ),
        (
            "else branch",
            "local v = 1\nif c then print(1) else local v = 2 print(v) end\nprint(v)\n",
        ),
        (
            "while body",
            "local v = 1\nwhile c do local v = 2 print(v) end\nprint(v)\n",
        ),
        (
            "repeat body",
            "local v = 1\nrepeat local v = 2 until v > 1\nprint(v)\n",
        ),
        (
            "numeric for var",
            "local v = 1\nfor v = 1, 10, 2 do print(v) end\nprint(v)\n",
        ),
        (
            "generic for var",
            "local v = 1\nfor v in pairs(t) do print(v) end\nprint(v)\n",
        ),
        (
            "method receiver argument",
            "local v = 1\nlocal f = obj:m(function() local v = 2 return v end)\nprint(v, f)\n",
        ),
        (
            "table constructor value",
            "local v = 1\nlocal t = { [k] = function() local v = 2 return v end }\nprint(v, t)\n",
        ),
        (
            "value-truncating parenthesis",
            "local v = 1\nlocal t = (f(function() local v = 2 return v end))\nprint(v, t)\n",
        ),
        (
            "unary and binary operands",
            "local v = 1\nlocal t = -g(function() local v = 2 return v end) + 1\nprint(v, t)\n",
        ),
    ] {
        assert!(
            default_codes(src).contains(&"LB0503".to_owned()),
            "no shadow reported for {label}: {src}"
        );
    }
}

#[test]
fn a_parameter_shadowing_an_outer_local_is_registered_silently() {
    // Parameters are not `local` declarations: they seed the function's frame
    // without a finding of their own — but they do become the outer binding a
    // nested `local` can then shadow (see `shadowing_a_parameter_fires`).
    let src = "local v = 1\nlocal f = function(v) return v end\nprint(v, f)\n";
    assert!(!default_codes(src).contains(&"LB0503".to_owned()));
}

#[test]
fn a_repeat_body_local_is_visible_to_its_until_condition() {
    // One shared frame for body + `until`, so re-declaring in the `until`'s
    // own scope is impossible and the body local shadows nothing new.
    let src = "repeat\n  local v = 1\nuntil v > 0\n";
    assert!(!default_codes(src).contains(&"LB0503".to_owned()));
}

#[test]
fn nameless_error_recovery_bindings_are_skipped_not_shadowed() {
    // Broken sources still reach the rules (only fixes are withheld). The
    // bindings the parser recovers have no name, and a nameless binding can
    // neither shadow nor be shadowed.
    for src in [
        "local = 1\ndo local = 2 end\n",
        "local function f(,) local v = 1 return v end\nreturn f\n",
    ] {
        let out = lint(src, &LintConfig::new());
        assert!(out.had_parse_errors, "{src}");
        assert!(
            out.diagnostics
                .iter()
                .all(|d| d.code.to_string() != "LB0503"),
            "{src}: {:?}",
            out.diagnostics
        );
    }
}

#[test]
fn a_top_level_local_shadows_nothing() {
    // The chunk has exactly one enclosing frame (the chunk's own parameter
    // frame), so nothing above it can be shadowed.
    let src = "local a = 1\nlocal b = 2\nreturn a + b\n";
    assert!(!default_codes(src).contains(&"LB0503".to_owned()));
}

#[test]
fn shadowing_is_suppressible_like_any_other_finding() {
    let src = "local v = 1\ndo\n  ---@luabox-ignore shadowed-local intentional inner scope\n  local v = 2\n  print(v)\nend\nprint(v)\n";
    assert!(!default_codes(src).contains(&"LB0503".to_owned()));
}

#[test]
fn control_flow_statements_are_walked_without_incident() {
    // `break`, `goto` and labels carry no bindings; the walker must step over
    // them and still find the shadow that follows.
    let src = "\
local v = 1
while c do
  if a then break end
  goto continue
  ::continue::
  do local v = 2 print(v) end
end
print(v)
";
    assert!(default_codes(src).contains(&"LB0503".to_owned()), "{src}");
}

// --- global-write (LB0504, suspicious) -------------------------------------

#[test]
fn global_write_fires() {
    assert!(default_codes("counter = 0\n").contains(&"LB0504".to_owned()));
}

#[test]
fn local_assignment_is_clean() {
    assert_eq!(
        default_codes("local counter = 0\ncounter = 1\nreturn counter\n"),
        Vec::<String>::new()
    );
}

#[test]
fn field_write_is_not_a_global_write() {
    assert!(!default_codes("local t = {}\nt.x = 1\nreturn t\n").contains(&"LB0504".to_owned()));
}

#[test]
fn allowlisted_global_is_silenced() {
    let mut c = LintConfig::new();
    c.allow_global("vim");
    assert!(!has("vim = 1\n", &c, "LB0504"));
}

// --- undefined-global (LB0509, suspicious, ticket #103) --------------------

#[test]
fn typo_read_fires() {
    // `prnit` is the exact repro from ticket #103.
    assert!(default_codes("prnit(1)\n").contains(&"LB0509".to_owned()));
}

#[test]
fn stdlib_read_is_clean() {
    assert_eq!(
        default_codes("print(\"hi\")\nreturn math.floor(1.5)\n"),
        Vec::<String>::new()
    );
}

#[test]
fn allowlisted_global_read_is_clean() {
    let mut c = LintConfig::new();
    c.allow_global("vim");
    assert!(!has("vim.notify(\"hi\")\n", &c, "LB0509"));
}

#[test]
fn file_assigned_global_read_is_clean() {
    // `foo = 1` makes the file self-defining; the later read is fine (the
    // assignment itself is `global-write`'s business, LB0504).
    let src = "foo = 1\nprint(foo)\n";
    assert!(!has(src, &LintConfig::new(), "LB0509"));
    assert!(has(src, &LintConfig::new(), "LB0504"));
}

#[test]
fn assignment_target_itself_is_not_flagged_as_a_read() {
    // The bare-name target of `counter = 0` is a write, not a read — only
    // `global-write` (LB0504) fires here, never LB0509.
    assert!(!default_codes("counter = 0\n").contains(&"LB0509".to_owned()));
}

#[test]
fn field_write_base_is_still_a_read() {
    // `t.x = 1` reads `t` (it must already exist to be indexed into) even
    // though the write itself isn't a bare-name target `global-write` flags.
    assert!(default_codes("t.x = 1\n").contains(&"LB0509".to_owned()));
}

#[test]
fn meta_file_is_exempt() {
    let src = "---@meta\nlove = {}\nfunction love.graphics.line() end\n";
    assert!(!default_codes(src).contains(&"LB0509".to_owned()));
}

#[test]
fn luabox_ignore_suppresses() {
    let src = "---@luabox-ignore undefined-global vendor global\nprnit(1)\n";
    assert!(!default_codes(src).contains(&"LB0509".to_owned()));
}

#[test]
fn diagnostic_disable_suppresses() {
    let src = "---@diagnostic disable: undefined-global\nprnit(1)\n";
    assert!(!default_codes(src).contains(&"LB0509".to_owned()));
}

#[test]
fn diagnostic_disable_next_line_suppresses_only_that_line() {
    let src = "---@diagnostic disable-next-line: undefined-global\nprnit(1)\nprnit(2)\n";
    let out = lint(src, &LintConfig::new());
    let lb0509_lines: Vec<usize> = out
        .diagnostics
        .iter()
        .filter(|d| d.code.to_string() == "LB0509")
        .filter_map(|d| d.primary_label().map(|l| l.span.range.start))
        .collect();
    // Only the second `prnit` call (unsuppressed) is reported.
    assert_eq!(lb0509_lines.len(), 1, "{:?}", out.diagnostics);
    assert!(src[lb0509_lines[0]..].starts_with("prnit(2)"));
}

#[test]
fn diagnostic_disable_does_not_touch_unrelated_names() {
    // `disable: undefined-field` isn't a rule we map — `prnit` still fires.
    let src = "---@diagnostic disable: undefined-field\nprnit(1)\n";
    assert!(default_codes(src).contains(&"LB0509".to_owned()));
}

#[test]
fn did_you_mean_hint_present_for_close_typo() {
    let out = lint("prnit(1)\n", &LintConfig::new());
    let finding = out
        .diagnostics
        .iter()
        .find(|d| d.code.to_string() == "LB0509")
        .expect("LB0509 finding");
    assert!(
        finding
            .notes
            .iter()
            .any(|n| n.contains("did you mean `print`")),
        "{:?}",
        finding.notes
    );
}

#[test]
fn project_defs_global_is_known() {
    // Without `love` in the known-globals set, the read fires...
    let src = "love.graphics.line()\n";
    assert!(has(src, &LintConfig::new(), "LB0509"));
    // ...but a project `[types] defs` package naming it (mirrored here by
    // an explicit known-globals set, the way `lint_cmd` builds one from
    // `defs/love2d.d.lua`) clears it.
    let mut known = std::collections::HashSet::new();
    known.insert("love".to_owned());
    let out = lint_with_globals(src, &LintConfig::new(), &known);
    assert!(
        out.diagnostics
            .iter()
            .all(|d| d.code.to_string() != "LB0509")
    );
}

// --- `---@meta` definition-file policy (LB0504 + LB0501, ticket #76) -------

#[test]
fn meta_file_global_write_lints_clean() {
    let src = "---@meta\nlove = {}\nlove.graphics = {}\n";
    assert_eq!(default_codes(src), Vec::<String>::new());
}

#[test]
fn same_content_without_meta_tag_still_fires_global_write() {
    let src = "love = {}\nlove.graphics = {}\n";
    assert!(default_codes(src).contains(&"LB0504".to_owned()));
}

#[test]
fn mid_file_meta_comment_does_not_exempt_the_file() {
    // The `---@meta` tag appears after a statement, so it does not count —
    // the tag must precede every statement in the file.
    let src = "local x = 1\nreturn x\n---@meta\ncounter = 0\n";
    assert!(default_codes(src).contains(&"LB0504".to_owned()));
}

#[test]
fn meta_file_unused_local_lints_clean() {
    // `scaffold` is never read — plain unused-local, exempted only because
    // the file is a `---@meta` defs module.
    let src = "---@meta\nlocal scaffold = {}\nlove = {}\n";
    assert!(!default_codes(src).contains(&"LB0501".to_owned()));
}

#[test]
fn meta_file_named_variant_is_still_recognised() {
    // `---@meta <name>` (a named module) is the same tag, just with an
    // optional module-name argument — still exempts the file.
    let src = "---@meta love2d\nlove = {}\n";
    assert_eq!(default_codes(src), Vec::<String>::new());
}

// --- explicit-nil-compare-truthiness (LB0505, style, type-informed) --------

#[test]
fn nil_compare_fires_on_known_non_boolean() {
    let src = "---@param s string\nlocal function f(s)\n  if s ~= nil then return s end\n  return \"\"\nend\nreturn f\n";
    assert!(has(src, &LintConfig::new(), "LB0505"));
}

#[test]
fn nil_compare_skips_boolean_type() {
    let src = "---@param b boolean\nlocal function f(b)\n  if b ~= nil then return b end\n  return false\nend\nreturn f\n";
    assert!(!has(src, &LintConfig::new(), "LB0505"));
}

#[test]
fn nil_compare_skips_unknown_type() {
    let src = "local function f(x)\n  if x ~= nil then return x end\nend\nreturn f\n";
    assert!(!has(src, &LintConfig::new(), "LB0505"));
}

#[test]
fn nil_compare_fix_both_directions() {
    let ne = "---@param s string\nlocal function f(s)\n  if s ~= nil then return s end\n  return \"\"\nend\nreturn f\n";
    let fixed = apply_fixes(ne, &lint(ne, &LintConfig::new()).fixes);
    assert!(fixed.contains("if s then"), "{fixed}");

    let eq = "---@param s string\nlocal function f(s)\n  if s == nil then return \"\" end\n  return s\nend\nreturn f\n";
    let fixed = apply_fixes(eq, &lint(eq, &LintConfig::new()).fixes);
    assert!(fixed.contains("if not s then"), "{fixed}");
}

#[test]
fn nil_compare_reads_either_operand_order() {
    let src = "---@param s string\nlocal function f(s)\n  if nil ~= s then return s end\n  return \"\"\nend\nreturn f\n";
    assert!(has(src, &LintConfig::new(), "LB0505"));
    let fixed = apply_fixes(src, &lint(src, &LintConfig::new()).fixes);
    assert!(fixed.contains("if s then"), "{fixed}");
}

#[test]
fn nil_compare_needs_a_plain_name_on_the_other_side() {
    // `t.field ~= nil` and `f() ~= nil` are not name expressions, so there is
    // no binding to consult and the rule stays silent.
    for src in [
        "---@param t table\nlocal function g(t)\n  if t.field ~= nil then return 1 end\n  return 0\nend\nreturn g\n",
        "---@param t table\nlocal function g(t)\n  if t.f() ~= nil then return 1 end\n  return 0\nend\nreturn g\n",
    ] {
        assert!(!has(src, &LintConfig::new(), "LB0505"), "{src}");
    }
}

#[test]
fn nil_compare_follows_the_declared_type_through_its_grammar() {
    // Optional, union and literal shapes all resolve to "cannot be `false`".
    for annotation in [
        "string?",
        "\"yes\"|\"no\"",
        "true",
        "(string)",
        "42",
        "fun():number",
    ] {
        let src = format!(
            "---@param s {annotation}\nlocal function f(s)\n  if s ~= nil then return 1 end\n  return 0\nend\nreturn f\n"
        );
        assert!(has(&src, &LintConfig::new(), "LB0505"), "{annotation}");
    }
    // ...and these can be `false`, so the guard is not redundant.
    for annotation in [
        "boolean",
        "any",
        "unknown",
        "false",
        "boolean|string",
        "`T`",
    ] {
        let src = format!(
            "---@param s {annotation}\nlocal function f(s)\n  if s ~= nil then return 1 end\n  return 0\nend\nreturn f\n"
        );
        assert!(!has(&src, &LintConfig::new(), "LB0505"), "{annotation}");
    }
}

#[test]
fn a_type_annotation_on_a_local_is_harvested_too() {
    // `---@type` binds positionally to the locals the statement declares.
    let src = "---@type string, boolean\nlocal name, flag = get()\nif name ~= nil then print(name) end\nif flag ~= nil then print(flag) end\n";
    let out = lint(src, &LintConfig::new());
    let hits: Vec<_> = out
        .diagnostics
        .iter()
        .filter(|d| d.code.to_string() == "LB0505")
        .collect();
    // Only `name` (string) is redundant-guard material; `flag` is a boolean.
    assert_eq!(hits.len(), 1, "{:?}", out.diagnostics);
    assert!(hits[0].message.contains("`name`"), "{}", hits[0].message);
}

#[test]
fn an_annotation_block_attached_to_nothing_is_ignored() {
    // The trailing block has no statement to bind to; harvesting must skip it
    // instead of mis-attaching the type to an earlier declaration.
    let src =
        "---@param s string\nlocal function f(s)\n  return s\nend\nreturn f\n---@param s boolean\n";
    assert_eq!(default_codes(src), Vec::<String>::new());
}

// --- concat-in-loop (LB0506, perf) -----------------------------------------

#[test]
fn concat_in_loop_fires() {
    let src = "local function join(parts)\n  local s = \"\"\n  for i = 1, #parts do\n    s = s .. parts[i]\n  end\n  return s\nend\nreturn join\n";
    assert!(has(src, &LintConfig::new(), "LB0506"));
}

#[test]
fn concat_of_loop_local_is_clean() {
    let src = "local function f(n)\n  for i = 1, n do\n    local s = \"\"\n    s = s .. \"x\"\n    print(s)\n  end\nend\nreturn f\n";
    assert!(!has(src, &LintConfig::new(), "LB0506"));
}

#[test]
fn concat_outside_any_loop_is_clean() {
    // No loop in the body at all...
    let src = "local s = \"\"\ns = s .. \"x\"\nprint(s)\n";
    assert!(!has(src, &LintConfig::new(), "LB0506"));
    // ...and a body that *does* have a loop, with the concat outside it.
    let src = "local s = \"\"\nfor i = 1, 3 do print(i) end\ns = s .. \"x\"\nprint(s)\n";
    assert!(!has(src, &LintConfig::new(), "LB0506"));
}

#[test]
fn concat_in_loop_needs_a_single_self_referential_assignment() {
    for (label, src) in [
        (
            "multi-target assignment",
            "local s, t = \"\", \"\"\nfor i = 1, 3 do s, t = s .. \"x\", t end\nprint(s, t)\n",
        ),
        (
            "not self-referential",
            "local s = \"\"\nlocal a, b = \"a\", \"b\"\nfor i = 1, 3 do s = a .. b end\nprint(s)\n",
        ),
        (
            "global accumulator",
            "for i = 1, 3 do total = total .. \"x\" end\nprint(total)\n",
        ),
    ] {
        assert!(!has(src, &LintConfig::new(), "LB0506"), "{label}: {src}");
    }
}

// --- pairs-on-array (LB0507, perf) -----------------------------------------

#[test]
fn pairs_on_declared_array_fires() {
    let src = "---@param xs number[]\nlocal function total(xs)\n  local n = 0\n  for _, x in pairs(xs) do n = n + x end\n  return n\nend\nreturn total\n";
    assert!(has(src, &LintConfig::new(), "LB0507"));
}

#[test]
fn pairs_on_map_is_clean() {
    let src = "---@param m table<string, number>\nlocal function f(m)\n  for k, v in pairs(m) do print(k, v) end\n  return m\nend\nreturn f\n";
    assert!(!has(src, &LintConfig::new(), "LB0507"));
}

#[test]
fn pairs_on_array_literal_fires_and_fixes() {
    let src = "for _, x in pairs({ 1, 2, 3 }) do print(x) end\n";
    assert!(has(src, &LintConfig::new(), "LB0507"));
    let fixed = apply_fixes(src, &lint(src, &LintConfig::new()).fixes);
    assert!(fixed.contains("ipairs({ 1, 2, 3 })"), "{fixed}");
}

#[test]
fn pairs_over_an_integer_keyed_table_type_fires() {
    let src = "---@param xs table<integer, string>\nlocal function f(xs)\n  for _, x in pairs(xs) do print(x) end\n  return xs\nend\nreturn f\n";
    assert!(has(src, &LintConfig::new(), "LB0507"));
}

#[test]
fn pairs_over_a_parenthesised_array_type_fires() {
    let src = "---@param xs (number[])\nlocal function f(xs)\n  for _, x in pairs(xs) do print(x) end\n  return xs\nend\nreturn f\n";
    assert!(has(src, &LintConfig::new(), "LB0507"));
}

#[test]
fn pairs_is_only_flagged_on_the_real_global_with_one_array_argument() {
    for (label, src) in [
        (
            "shadowed pairs",
            "local pairs = myiter\nfor _, x in pairs({ 1, 2 }) do print(x) end\n",
        ),
        (
            "extra argument",
            "for _, x in pairs({ 1, 2 }, extra) do print(x) end\n",
        ),
        (
            "empty table literal",
            "for _, x in pairs({}) do print(x) end\n",
        ),
        (
            "keyed table literal",
            "for k, v in pairs({ a = 1 }) do print(k, v) end\n",
        ),
        (
            "not a name or table literal",
            "local t = {}\nfor k, v in pairs(t.inner) do print(k, v) end\n",
        ),
        (
            "non-array declared type",
            "---@param xs fun():number\nlocal function f(xs)\n  for k in pairs(xs) do print(k) end\n  return xs\nend\nreturn f\n",
        ),
    ] {
        assert!(!has(src, &LintConfig::new(), "LB0507"), "{label}: {src}");
    }
}

// --- empty-then (LB0508, suspicious) ---------------------------------------

#[test]
fn empty_then_fires() {
    let src = "local x = true\nif x then end\n";
    assert!(has(src, &LintConfig::new(), "LB0508"));
}

#[test]
fn commented_then_is_clean() {
    let src = "local x = true\nif x then\n  -- handled elsewhere\nend\n";
    assert!(!has(src, &LintConfig::new(), "LB0508"));
}

#[test]
fn nonempty_then_is_clean() {
    let src = "local x = true\nif x then print(x) end\n";
    assert!(!has(src, &LintConfig::new(), "LB0508"));
}

#[test]
fn empty_elseif_fires() {
    let src = "local x = 1\nif x == 1 then print(x) elseif x == 2 then end\n";
    assert!(has(src, &LintConfig::new(), "LB0508"));
}

// --- suppression / malformed-ignore (LB0500) -------------------------------

#[test]
fn ignore_with_reason_suppresses() {
    let src = "---@luabox-ignore unused-local leftover from refactor\nlocal x = 1\n";
    assert_eq!(default_codes(src), Vec::<String>::new());
}

#[test]
fn same_line_ignore_suppresses() {
    let src = "local x = 1 ---@luabox-ignore unused-local intentional\n";
    assert_eq!(default_codes(src), Vec::<String>::new());
}

#[test]
fn ignore_without_reason_is_diagnosed() {
    let src = "---@luabox-ignore unused-local\nlocal x = 1\n";
    let c = default_codes(src);
    assert!(c.contains(&"LB0500".to_owned()), "{c:?}");
    // Malformed => not actually suppressed, so the finding still shows.
    assert!(c.contains(&"LB0501".to_owned()), "{c:?}");
}

#[test]
fn ignore_without_rule_id_is_diagnosed() {
    let src = "---@luabox-ignore\nlocal x = 1\n";
    assert!(default_codes(src).contains(&"LB0500".to_owned()));
}

#[test]
fn file_level_ignore_suppresses_all() {
    let src = "---@luabox-ignore unused-local project-wide policy\nlocal x = 1\nlocal y = 2\n";
    assert_eq!(default_codes(src), Vec::<String>::new());
}

#[test]
fn ignore_targets_only_the_named_rule() {
    // Suppress unused-local; the global-write finding survives.
    let src = "---@luabox-ignore unused-local ok\ncounter = 0\n";
    let c = default_codes(src);
    assert!(c.contains(&"LB0504".to_owned()), "{c:?}");
}

#[test]
fn a_diagnostic_directive_without_a_colon_is_ignored() {
    let src = "---@diagnostic disable\nprnit(1)\n";
    assert!(default_codes(src).contains(&"LB0509".to_owned()));
}

#[test]
fn an_unrecognised_diagnostic_action_is_ignored() {
    let src = "---@diagnostic enable: undefined-global\nprnit(1)\n";
    assert!(default_codes(src).contains(&"LB0509".to_owned()));
}

#[test]
fn a_diagnostic_directive_naming_several_rules_still_maps() {
    let src = "---@diagnostic disable: undefined-field, undefined-global\nprnit(1)\n";
    assert!(!default_codes(src).contains(&"LB0509".to_owned()));
}

#[test]
fn an_underscore_parameter_is_exempt_from_unused_param() {
    let src = "local function f(_ignored, used) return used end\nreturn f\n";
    assert!(!has(src, &pedantic(), "LB0502"));
}

// --- config precedence -----------------------------------------------------

#[test]
fn rule_allow_silences() {
    let mut c = LintConfig::new();
    assert!(c.set_rule("unused-local", "allow"));
    assert!(!has("local x = 1\n", &c, "LB0501"));
}

#[test]
fn tier_toggle_silences() {
    let mut c = LintConfig::new();
    assert!(c.set_tier("style", "allow"));
    assert!(!has("local x = 1\n", &c, "LB0501"));
}

#[test]
fn rule_override_beats_tier() {
    let mut c = LintConfig::new();
    assert!(c.set_tier("style", "deny"));
    assert!(c.set_rule("unused-local", "allow"));
    assert!(!has("local x = 1\n", &c, "LB0501"));
}

#[test]
fn deny_tier_raises_error_severity() {
    let mut c = LintConfig::new();
    assert!(c.set_rule("unused-local", "deny"));
    let out = lint("local x = 1\n", &c);
    assert!(out.error_count >= 1);
    assert!(
        out.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    );
}

#[test]
fn default_style_finding_does_not_fail() {
    let out = lint("local x = 1\n", &LintConfig::new());
    assert_eq!(out.error_count, 0);
    assert!(
        out.diagnostics
            .iter()
            .all(|d| d.severity == Severity::Warning)
    );
}

// --- fixes -----------------------------------------------------------------

#[test]
fn unused_local_fix_renames_and_reconverges() {
    let src = "local x = 1\n";
    let out = lint(src, &LintConfig::new());
    let fixed = apply_fixes(src, &out.fixes);
    assert_eq!(fixed, "local _x = 1\n");
    // Second pass is clean.
    assert_eq!(default_codes(&fixed), Vec::<String>::new());
}

#[test]
fn no_fixes_on_files_with_parse_errors() {
    let src = "local = 5\n";
    let out = lint(src, &LintConfig::new());
    assert!(out.had_parse_errors);
    assert!(out.fixes.is_empty());
    assert!(
        out.diagnostics
            .iter()
            .any(|d| d.code.to_string() == "LB0001")
    );
}

#[test]
fn apply_fixes_is_stable_on_second_run() {
    let src = "local x = 1\nfor _, y in pairs({ 1, 2 }) do print(y) end\n";
    let first = apply_fixes(src, &lint(src, &LintConfig::new()).fixes);
    let second = apply_fixes(&first, &lint(&first, &LintConfig::new()).fixes);
    assert_eq!(first, second, "fixes should converge");
}

// --- registry / tier / level vocabulary -------------------------------------

#[test]
fn every_registered_rule_is_uniquely_identified_and_described() {
    let registry = rules();
    assert_eq!(registry.len(), 9, "the SPEC §9 rule set");
    let mut ids: Vec<&str> = Vec::new();
    let mut codes: Vec<String> = Vec::new();
    for rule in &registry {
        let id = rule.id();
        assert!(
            id.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
            "`{id}` is not kebab-case"
        );
        assert!(!rule.description().is_empty(), "`{id}` has no description");
        assert!(
            !rule.description().ends_with('.'),
            "`{id}` description is a phrase, not a sentence"
        );
        assert!(!ids.contains(&id), "duplicate rule id `{id}`");
        let code = rule.code().to_string();
        assert!(
            code.starts_with("LB05"),
            "`{id}` code {code} is outside LB05xx"
        );
        assert!(!codes.contains(&code), "duplicate code {code} on `{id}`");
        // The tier keyword round-trips, so a `[lint]` toggle can name it.
        assert_eq!(Tier::parse(rule.tier().name()), Some(rule.tier()));
        ids.push(id);
        codes.push(code);
    }
}

#[test]
fn every_tier_keyword_round_trips_and_has_a_default_level() {
    for (tier, keyword, default) in [
        (Tier::Correctness, "correctness", Level::Deny),
        (Tier::Suspicious, "suspicious", Level::Warn),
        (Tier::Perf, "perf", Level::Warn),
        (Tier::Style, "style", Level::Warn),
        (Tier::Pedantic, "pedantic", Level::Allow),
    ] {
        assert_eq!(tier.name(), keyword);
        assert_eq!(Tier::parse(keyword), Some(tier));
        assert_eq!(tier_default(tier), default);
    }
    assert_eq!(Tier::parse("correctnes"), None);
    assert_eq!(Tier::parse(""), None);
}

#[test]
fn levels_map_to_severities_and_reject_unknown_keywords() {
    assert_eq!(Level::parse("allow"), Some(Level::Allow));
    assert_eq!(Level::parse("warn"), Some(Level::Warn));
    assert_eq!(Level::parse("deny"), Some(Level::Deny));
    assert_eq!(Level::parse("forbid"), None);
    assert_eq!(Level::Allow.severity(), None);
    assert_eq!(Level::Warn.severity(), Some(Severity::Warning));
    assert_eq!(Level::Deny.severity(), Some(Severity::Error));
}

#[test]
fn unrecognised_config_keywords_are_rejected_without_changing_anything() {
    let mut c = LintConfig::new();
    assert!(!c.set_tier("nonsense", "warn"), "unknown tier name");
    assert!(!c.set_tier("style", "forbid"), "unknown level keyword");
    assert!(
        !c.set_rule("unused-local", "forbid"),
        "unknown level keyword"
    );
    // None of the rejected calls took effect: the style default still fires.
    assert!(has("local x = 1\n", &c, "LB0501"));
    // An unknown *rule id* is accepted (ids are not validated) but inert.
    assert!(c.set_rule("no-such-rule", "deny"));
    assert!(has("local x = 1\n", &c, "LB0501"));
}

// --- corpus sweep ----------------------------------------------------------

/// A module mirroring `tools/gen-corpus` output shapes (function, config
/// table, OOP metatable class, filter loop). Idiomatic Lua must produce no
/// correctness/suspicious/perf findings — only style (`unused-local`) is
/// acceptable — and must never panic.
const CORPUS: &str = "\
local M = {}

---Compute the value derived from `a` and `b`.
---@param a number
---@param b number
---@return number
local function compute(a, b)
    local result = a + b
    if result < 0 then
        result = -result
    end
    return result
end

---@class Widget
---@field kind string
---@field value number
local Widget = {}
Widget.__index = Widget

---@param value number
---@return Widget
function Widget.new(value)
    local self = setmetatable({}, Widget)
    self.kind = \"item\"
    self.value = value
    return self
end

---@return string
function Widget:describe()
    return string.format(\"%s=%d\", self.kind, self.value)
end

---Collect the entries that meet a threshold.
---@param items number[]
---@param threshold number
---@return number[]
local function filter(items, threshold)
    local out = {}
    for i = 1, #items do
        local v = items[i]
        if v >= threshold then
            out[#out + 1] = v
        end
    end
    return out
end

M.compute = compute
M.Widget = Widget
M.filter = filter

return M
";

#[test]
fn corpus_has_no_serious_findings_and_no_panic() {
    let out = lint(CORPUS, &LintConfig::new());
    let serious = [
        "LB0001", "LB0500", "LB0503", "LB0504", "LB0505", "LB0506", "LB0507", "LB0508", "LB0509",
    ];
    for diag in &out.diagnostics {
        let code = diag.code.to_string();
        assert!(
            !serious.contains(&code.as_str()),
            "unexpected serious finding {code}: {}",
            diag.message
        );
    }
    // Everything that exports its locals is read; no unused-local either.
    assert_eq!(out.error_count, 0);
}
