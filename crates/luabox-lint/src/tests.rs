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

use crate::{
    Level, LintConfig, LintLevel, LintOutcome, LintTier, Tier, UnknownRuleId, apply_fixes,
    lint_source, rule_ids, rules, tier_default,
};

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
    c.set_tier(Tier::Pedantic, Level::Warn);
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

// --- metatable-without-index (LB0510, suspicious) --------------------------
//
// The checker resolves `c:value()` through the carrier with no `__index`
// (luals parity, #33, LIMITATIONS-recorded). Every reference Lua crashes on
// that program, so the runtime gap lives here instead (Shockwave round 2).

/// The reviewer's repro, verbatim in shape: `lua5.4` says
/// `attempt to call a nil value (method 'value')`.
const COUNTER_REPRO: &str = "\
---@class Counter
---@field n integer
local Counter = {}

function Counter:value()
  return self.n
end

local c = setmetatable({ n = 1 }, Counter)
return c:value()
";

#[test]
fn setmetatable_with_an_unwired_carrier_fires() {
    assert!(has(COUNTER_REPRO, &LintConfig::new(), "LB0510"));
}

#[test]
fn the_finding_names_the_carrier_and_the_one_line_fix() {
    let out = lint(COUNTER_REPRO, &LintConfig::new());
    let diag = out
        .diagnostics
        .iter()
        .find(|d| d.code.to_string() == "LB0510")
        .expect("LB0510");
    assert!(diag.message.contains("Counter"), "{}", diag.message);
    assert!(diag.message.contains("__index"), "{}", diag.message);
    assert!(
        diag.notes
            .iter()
            .any(|n| n.contains("Counter.__index = Counter")),
        "{:?}",
        diag.notes
    );
}

#[test]
fn an_index_assigned_before_the_setmetatable_is_silent() {
    let src = "\
---@class Counter
local Counter = {}
Counter.__index = Counter
function Counter:value() return 1 end
local c = setmetatable({}, Counter)
return c:value()
";
    assert!(!has(src, &LintConfig::new(), "LB0510"));
}

/// Order does not matter to Lua: the field is set before any lookup runs.
#[test]
fn an_index_assigned_after_the_setmetatable_is_silent() {
    let src = "\
---@class Counter
local Counter = {}
function Counter:value() return 1 end
local c = setmetatable({}, Counter)
Counter.__index = Counter
return c:value()
";
    assert!(!has(src, &LintConfig::new(), "LB0510"));
}

#[test]
fn an_index_assigned_to_something_other_than_the_carrier_is_silent() {
    let src = "\
---@class Base
local Base = {}
Base.__index = Base
---@class Counter
local Counter = {}
Counter.__index = Base
local c = setmetatable({}, Counter)
return c
";
    assert!(!has(src, &LintConfig::new(), "LB0510"));
}

#[test]
fn a_bracket_spelled_index_is_silent() {
    let src = "\
---@class Counter
local Counter = {}
Counter[\"__index\"] = Counter
local c = setmetatable({}, Counter)
return c
";
    assert!(!has(src, &LintConfig::new(), "LB0510"));
}

#[test]
fn an_index_key_in_the_carriers_own_constructor_is_silent() {
    let src = "\
---@class Counter
local Counter = { __index = nil }
local c = setmetatable({}, Counter)
return c
";
    assert!(!has(src, &LintConfig::new(), "LB0510"));
}

/// A computed key might be `__index`; guessing would be a false positive.
#[test]
fn a_dynamic_field_write_on_the_carrier_is_silent() {
    let src = "\
---@class Counter
local Counter = {}
local k = \"__index\"
Counter[k] = Counter
local c = setmetatable({}, Counter)
return c
";
    assert!(!has(src, &LintConfig::new(), "LB0510"));
}

#[test]
fn a_rawset_on_the_carrier_is_silent() {
    let src = "\
---@class Counter
local Counter = {}
rawset(Counter, \"__index\", Counter)
local c = setmetatable({}, Counter)
return c
";
    assert!(!has(src, &LintConfig::new(), "LB0510"));
}

#[test]
fn a_dynamic_metatable_is_silent() {
    let src = "local mt = require(\"other\")\nlocal c = setmetatable({}, mt)\nreturn c\n";
    assert!(!has(src, &LintConfig::new(), "LB0510"));
}

#[test]
fn a_metatable_that_is_not_a_declared_carrier_is_silent() {
    let src = "local mt = {}\nlocal c = setmetatable({}, mt)\nreturn c\n";
    assert!(!has(src, &LintConfig::new(), "LB0510"));
}

#[test]
fn a_table_literal_metatable_is_silent() {
    let src = "local c = setmetatable({}, { __index = {} })\nreturn c\n";
    assert!(!has(src, &LintConfig::new(), "LB0510"));
}

/// A local `setmetatable` is not the stdlib one.
#[test]
fn a_shadowed_setmetatable_is_silent() {
    let src = "\
---@class Counter
local Counter = {}
local setmetatable = function(t, _) return t end
local c = setmetatable({}, Counter)
return c
";
    assert!(!has(src, &LintConfig::new(), "LB0510"));
}

#[test]
fn a_meta_definition_file_is_exempt() {
    let src = "\
---@meta
---@class Counter
local Counter = {}
local c = setmetatable({}, Counter)
return c
";
    assert!(!has(src, &LintConfig::new(), "LB0510"));
}

#[test]
fn the_rule_is_suppressible_like_any_other() {
    let src = "\
---@class Counter
local Counter = {}
---@luabox-ignore metatable-without-index wired up by the caller
local c = setmetatable({}, Counter)
return c
";
    assert!(!has(src, &LintConfig::new(), "LB0510"));
}

#[test]
fn every_setmetatable_on_an_unwired_carrier_is_reported() {
    let src = "\
---@class Counter
local Counter = {}
local a = setmetatable({}, Counter)
local b = setmetatable({}, Counter)
return a, b
";
    let found = lint(src, &LintConfig::new())
        .diagnostics
        .iter()
        .filter(|d| d.code.to_string() == "LB0510")
        .count();
    assert_eq!(found, 2);
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

/// Line numbers now come from a precomputed line table rather than a newline
/// count per finding (the O(findings x file size) fix). The inputs where a
/// line table and a raw newline count could disagree are CRLF endings, a
/// missing final newline, and offsets far from byte 0 — pin all three
/// through the behaviour that depends on them.
#[test]
fn suppression_lines_survive_crlf_and_a_missing_final_newline() {
    let late = format!(
        "{}---@luabox-ignore unused-local late\r\nlocal x = 1",
        "print(1)\r\n".repeat(200)
    );
    for src in [
        "---@luabox-ignore unused-local crlf\r\nlocal x = 1\r\n",
        "local x = 1 ---@luabox-ignore unused-local crlf trailing\r\n",
        "---@luabox-ignore unused-local no final newline\nlocal x = 1",
        "local x = 1 ---@luabox-ignore unused-local no final newline",
        &late,
    ] {
        assert_eq!(default_codes(src), Vec::<String>::new(), "{src:?}");
    }
}

/// The mirror of the test above: an ignore comment that is *not* adjacent to
/// the finding must still not suppress it, whatever the line endings.
#[test]
fn a_distant_ignore_comment_does_not_suppress() {
    // Not file-level: a statement precedes the comment, so it only covers
    // its own line and the one below — line 5's finding stays.
    let src = "print(1)\r\nprint(2)\r\n---@luabox-ignore unused-local too late\r\nprint(3)\r\nlocal y = 2\r\n";
    let c = default_codes(src);
    assert!(c.contains(&"LB0501".to_owned()), "{c:?}");
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
    c.set_rule("unused-local", Level::Allow);
    assert!(!has("local x = 1\n", &c, "LB0501"));
}

#[test]
fn tier_toggle_silences() {
    let mut c = LintConfig::new();
    c.set_tier(Tier::Style, Level::Allow);
    assert!(!has("local x = 1\n", &c, "LB0501"));
}

#[test]
fn rule_override_beats_tier() {
    let mut c = LintConfig::new();
    c.set_tier(Tier::Style, Level::Deny);
    c.set_rule("unused-local", Level::Allow);
    assert!(!has("local x = 1\n", &c, "LB0501"));
}

#[test]
fn deny_tier_raises_error_severity() {
    let mut c = LintConfig::new();
    c.set_rule("unused-local", Level::Deny);
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
    assert_eq!(registry.len(), 10, "the SPEC §9 rule set");
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
        assert!(!codes.contains(&code), "duplicate code {code} on `{id}`");
        // The tier keyword round-trips, so a `[lint]` toggle can name it.
        assert_eq!(Tier::parse(rule.tier().name()), Some(rule.tier()));
        ids.push(id);
        codes.push(code);
    }
}

/// The load-bearing half of the lint-band contract
/// ([`luabox_diag::Code::is_lint`]).
///
/// The language server decides a finding's `source` — and therefore whether
/// its quick-fix matcher will look at it — from that predicate. Nothing used
/// to assert that a rule's code satisfies it: the LSP open-coded
/// `code.number() / 100 == 5` and every rule happened to comply
/// (Shockwave round 2). A rule registered at, say, `LB0700` would have been
/// published under the toolchain source, silently losing its quick fixes.
///
/// `luabox-diag` cannot assert this direction — it sits below this crate and
/// cannot see the rule registry — so this is where it lives.
#[test]
fn every_rule_code_is_in_the_lint_band() {
    let registry = rules();
    assert!(!registry.is_empty(), "the registry is empty");
    for rule in &registry {
        let code = rule.code();
        // `is_lint_rule` is `is_lint` minus `LB0500`, the crate's own
        // malformed-`---@luabox-ignore` diagnostic: in the band (so the LSP
        // tags it with the lint source) but not a rule.
        assert!(
            code.is_lint_rule(),
            "rule `{}` has code {code}, which is not a lint rule code",
            rule.id()
        );
    }
    // The suppression-syntax diagnostic is in the band and is not a rule —
    // both halves, so neither predicate can quietly collapse into the other.
    assert!(luabox_diag::Code::new(500).is_lint());
    assert!(!luabox_diag::Code::new(500).is_lint_rule());
}

/// The second unwritten invariant behind the editor's quick-fix matcher:
/// **only rules carry fixes**.
///
/// `crate::lint_source` mirrors a rule's machine-applicable fix into both the
/// `fixes` list and the diagnostic's suggestions, and the language server
/// pairs them back up by span + replacement before offering a code action.
/// A fix arriving on a diagnostic outside the rule band would be matched to
/// a diagnostic the editor tagged with the toolchain source, and the action
/// would reference a diagnostic the client never saw.
#[test]
fn only_lint_rules_carry_fixes() {
    // A file with a fixable finding (`pairs` over an array literal), a
    // non-rule lint-crate finding (`LB0500`, a bare ignore tag), and a
    // control-flow legality error (`LB0022`) — all three travel out of the
    // same engine.
    let src = "\
---@luabox-ignore
local function each()
  for _, v in pairs({ 1, 2, 3 }) do print(v) end
end
break
return each
";
    let out = lint(src, &LintConfig::new());
    let codes: Vec<String> = out.diagnostics.iter().map(|d| d.code.to_string()).collect();
    assert!(codes.contains(&"LB0500".to_owned()), "{codes:?}");
    assert!(codes.contains(&"LB0022".to_owned()), "{codes:?}");
    assert!(codes.contains(&"LB0507".to_owned()), "{codes:?}");
    assert!(!out.fixes.is_empty(), "expected at least one fix");

    for diag in &out.diagnostics {
        if diag.suggestions.is_empty() {
            continue;
        }
        assert!(
            diag.code.is_lint_rule(),
            "{} carries a fix but is not a lint rule code",
            diag.code
        );
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
fn an_unknown_rule_id_is_inert_but_no_longer_silent() {
    // Rule ids are open — they live with the rules, and a `[lint]` entry for
    // one this build does not have does not fail the lint. What it must not do
    // any more is vanish: the override is inert *and reported* (CC-M8).
    let mut c = LintConfig::new();
    c.set_rule("no-such-rule", Level::Deny);
    assert!(has("local x = 1\n", &c, "LB0501"));

    let unknown = c.unknown_rule_ids();
    assert_eq!(unknown.len(), 1, "{unknown:?}");
    assert_eq!(unknown[0].id(), "no-such-rule");
    assert_eq!(
        unknown[0].message(),
        "unknown lint rule id `no-such-rule` in `[lint]`"
    );
}

// --- unknown `[lint]` rule ids (CC-M8) --------------------------------------

#[test]
fn every_known_rule_id_is_accepted_silently() {
    let mut c = LintConfig::new();
    for id in rule_ids() {
        c.set_rule(id, Level::Warn);
    }
    assert!(
        c.unknown_rule_ids().is_empty(),
        "{:?}",
        c.unknown_rule_ids()
    );
}

#[test]
fn a_typod_rule_id_is_reported_with_the_rule_it_meant() {
    let mut c = LintConfig::new();
    c.set_rule("unused-locl", Level::Allow);
    let unknown = c.unknown_rule_ids();
    assert_eq!(unknown.len(), 1, "{unknown:?}");
    assert_eq!(unknown[0].suggestion(), Some("unused-local"));
    assert_eq!(
        unknown[0].notes(),
        vec![
            "did you mean `unused-local`?".to_owned(),
            "this `[lint]` entry has no effect".to_owned(),
        ]
    );
}

#[test]
fn a_typod_tier_name_is_reported_with_the_tier_it_meant() {
    // A mistyped *tier* is indistinguishable from a rule-id override by the
    // time it reaches the config — `[lint] tiers` is typed, so `pedantics`
    // lands in `rules`. The nudge therefore searches both vocabularies.
    for (typo, meant) in [
        ("pedantics", "pedantic"),
        ("styl", "style"),
        ("correctnes", "correctness"),
    ] {
        let mut c = LintConfig::new();
        c.set_rule(typo, Level::Warn);
        let unknown = c.unknown_rule_ids();
        assert_eq!(unknown.len(), 1, "{typo}: {unknown:?}");
        assert_eq!(unknown[0].suggestion(), Some(meant), "{typo}");
    }
}

#[test]
fn an_unrecognisable_rule_id_is_reported_without_a_suggestion() {
    let mut c = LintConfig::new();
    c.set_rule("totally-made-up-thing", Level::Deny);
    let unknown = c.unknown_rule_ids();
    assert_eq!(unknown.len(), 1, "{unknown:?}");
    assert_eq!(unknown[0].suggestion(), None);
    assert_eq!(
        unknown[0].notes(),
        vec!["this `[lint]` entry has no effect"]
    );
}

#[test]
fn unknown_rule_ids_are_reported_in_a_stable_order() {
    let mut c = LintConfig::new();
    for id in ["zebra-rule", "alpha-rule", "middle-rule"] {
        c.set_rule(id, Level::Warn);
    }
    let unknown = c.unknown_rule_ids();
    let ids: Vec<&str> = unknown.iter().map(UnknownRuleId::id).collect();
    assert_eq!(ids, ["alpha-rule", "middle-rule", "zebra-rule"]);
}

#[test]
fn from_manifest_builds_the_config_and_hands_back_the_unknown_ids() {
    let manifest = luabox_manifest::model::Manifest::parse(
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n\
         [lint]\nglobals = [\"acme\"]\nstyle = \"deny\"\nunused-local = \"allow\"\n\
         unused-parm = \"allow\"\n",
    )
    .expect("manifest parses");
    let (config, unknown) = LintConfig::from_manifest(&manifest.lint);

    // The known halves land: the allow-list, the tier toggle, the rule
    // override (`unused-local` silenced despite style now denying).
    assert!(config.is_allowed_global("acme"));
    assert!(!has("local x = 1\n", &config, "LB0501"));

    // ...and the typo is the only thing reported.
    assert_eq!(unknown.len(), 1, "{unknown:?}");
    assert_eq!(unknown[0].id(), "unused-parm");
    assert_eq!(unknown[0].suggestion(), Some("unused-param"));
}

#[test]
fn every_tier_is_in_tier_all() {
    // `Tier::ALL` feeds the did-you-mean candidate set; a tier missing from it
    // is a tier a typo can never be nudged towards.
    assert_eq!(Tier::ALL.len(), 5);
    for tier in Tier::ALL {
        assert_eq!(Tier::parse(tier.name()), Some(tier));
    }
    for name in ["correctness", "suspicious", "perf", "style", "pedantic"] {
        assert!(
            Tier::ALL.iter().any(|t| t.name() == name),
            "`{name}` missing from Tier::ALL"
        );
    }
}

#[test]
fn the_manifest_lint_vocabulary_maps_onto_this_crate_s_own() {
    for (manifest_tier, tier) in [
        (LintTier::Correctness, Tier::Correctness),
        (LintTier::Suspicious, Tier::Suspicious),
        (LintTier::Perf, Tier::Perf),
        (LintTier::Style, Tier::Style),
        (LintTier::Pedantic, Tier::Pedantic),
    ] {
        assert_eq!(Tier::from(manifest_tier), tier);
        // Both vocabularies spell a tier the same way in `luabox.toml`.
        assert_eq!(manifest_tier.as_str(), tier.name());
    }
    for (manifest_level, level) in [
        (LintLevel::Allow, Level::Allow),
        (LintLevel::Warn, Level::Warn),
        (LintLevel::Deny, Level::Deny),
    ] {
        assert_eq!(Level::from(manifest_level), level);
        assert_eq!(Level::parse(manifest_level.as_str()), Some(level));
    }
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
