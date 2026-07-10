//! Unit tests: each rule (firing / non-firing / type-informed negative),
//! suppression, config precedence, fix idempotence, and a corpus sweep.

use luabox_diag::Severity;
use luabox_syntax::Dialect;

use crate::{LintConfig, LintOutcome, apply_fixes, lint_source};

fn lint(source: &str, config: &LintConfig) -> LintOutcome {
    lint_source("test.lua", source, Dialect::Lua54, config)
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
        "LB0001", "LB0500", "LB0503", "LB0504", "LB0505", "LB0506", "LB0507", "LB0508",
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
