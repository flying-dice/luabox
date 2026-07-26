// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]
//! Exact-output tests for the lowering matrix (SPEC.md §2.1): every rule's
//! rewrite is asserted byte-for-byte, plus tree-shaking, diagnostics, and
//! idempotence invariants. Semantics arguments live in each rule module's
//! doc comment; these tests pin the mechanical output they argue about.

use std::collections::BTreeSet;

use luabox_lower::{Helper, Severity, lower, lower_bare, rt_prelude};
use luabox_syntax::Dialect;

/// Lower and expect success, returning the output text.
fn text(source: &str, from: Dialect, to: Dialect) -> String {
    match lower(source, from, to) {
        Ok(lowered) => lowered.text,
        Err(diags) => panic!("expected lowering to succeed, got {diags:#?}"),
    }
}

/// Lower and expect failure, returning the error codes.
fn error_codes(source: &str, from: Dialect, to: Dialect) -> Vec<&'static str> {
    match lower(source, from, to) {
        Ok(lowered) => panic!("expected lowering to fail, got:\n{}", lowered.text),
        Err(diags) => diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.code)
            .collect(),
    }
}

fn warning_codes(source: &str, from: Dialect, to: Dialect) -> Vec<&'static str> {
    match lower(source, from, to) {
        Ok(lowered) => lowered.warnings.iter().map(|d| d.code).collect(),
        Err(diags) => panic!("expected lowering to succeed, got {diags:#?}"),
    }
}

// === identity / idempotence ===============================================

const CORPUS_51: &str = "local x = 1\nprint(x)\n";

#[test]
fn same_dialect_is_byte_identity() {
    for dialect in Dialect::ALL {
        let lowered = lower(CORPUS_51, dialect, dialect).expect("identity");
        assert_eq!(lowered.text, CORPUS_51);
        assert!(lowered.polyfills.is_empty());
        assert!(lowered.warnings.is_empty());
    }
}

#[test]
fn nothing_to_lower_means_no_prelude_and_no_change() {
    let source = "local t = { a = 1 }\nfor k, v in pairs(t) do print(k, v) end\n";
    let lowered = lower(source, Dialect::Lua54, Dialect::Lua51).expect("lower");
    assert_eq!(lowered.text, source, "untouched input must round-trip");
    assert!(lowered.polyfills.is_empty(), "zero-cost when unused");
}

// === floor division ========================================================

#[test]
fn floor_div_lowers_to_math_floor() {
    assert_eq!(
        text("x = a // b\n", Dialect::Lua53, Dialect::Lua51),
        "x = math.floor(a / b)\n"
    );
}

#[test]
fn floor_div_warns_lb0606_once_per_file() {
    let warnings = warning_codes("x = a // b\ny = c // d\n", Dialect::Lua53, Dialect::Lua51);
    assert_eq!(warnings, vec!["LB0606"]);
}

#[test]
fn floor_div_precedence_same_level_needs_no_parens() {
    // `/` and `//` share precedence and associativity; the call wrapper is
    // primary, so operand text carries over verbatim.
    assert_eq!(
        text("x = a + b // c\n", Dialect::Lua53, Dialect::Lua51),
        "x = a + math.floor(b / c)\n"
    );
    assert_eq!(
        text("x = a // b * c\n", Dialect::Lua53, Dialect::Lua51),
        "x = math.floor(a / b) * c\n"
    );
    assert_eq!(
        text("x = -a // b\n", Dialect::Lua53, Dialect::Lua51),
        "x = math.floor(-a / b)\n"
    );
    assert_eq!(
        text("x = 2 ^ n // d\n", Dialect::Lua53, Dialect::Lua51),
        "x = math.floor(2 ^ n / d)\n"
    );
}

#[test]
fn floor_div_chains_lower_innermost_first() {
    assert_eq!(
        text("x = a // b // c\n", Dialect::Lua54, Dialect::Lua52),
        "x = math.floor(math.floor(a / b) / c)\n"
    );
}

#[test]
fn floor_div_keeps_parenthesized_operands() {
    assert_eq!(
        text("x = (a + b) // c\n", Dialect::Lua53, Dialect::Lua51),
        "x = math.floor((a + b) / c)\n"
    );
}

#[test]
fn floor_div_not_lowered_when_target_has_it() {
    let source = "x = a // b\n";
    assert_eq!(text(source, Dialect::Lua54, Dialect::Lua53), source);
}

// === bitwise operators =====================================================

#[test]
fn bitops_full_output_on_52_uses_bit32_backend() {
    assert_eq!(
        text("x = a & b\n", Dialect::Lua53, Dialect::Lua52),
        "local __luabox_rt = (function()\n\
         \x20 local M = {}\n\
         \x20 M.band = bit32.band\n\
         \x20 return M\n\
         end)()\n\
         \n\
         x = __luabox_rt.band(a, b)\n"
    );
}

#[test]
fn bitops_nested_expression_association_preserved() {
    let out = text("x = a & b | ~c\n", Dialect::Lua53, Dialect::Lua52);
    assert!(out.ends_with("x = __luabox_rt.bor(__luabox_rt.band(a, b), __luabox_rt.bnot(c))\n"));
}

#[test]
fn bitops_polyfill_is_tree_shaken_to_used_ops() {
    let lowered = lower("x = a << 1 | b\n", Dialect::Lua53, Dialect::Lua52).expect("lower");
    assert_eq!(lowered.polyfills, vec!["bor", "shl"]);
    assert!(!lowered.text.contains("M.band"));
    assert!(!lowered.text.contains("M.shr"));
}

#[test]
fn unary_bnot_vs_binary_bxor() {
    let out = text("x = a ~ b\ny = ~a\n", Dialect::Lua53, Dialect::Lua52);
    assert!(out.contains("x = __luabox_rt.bxor(a, b)"));
    assert!(out.contains("y = __luabox_rt.bnot(a)"));
}

#[test]
fn tilde_eq_is_not_a_bitop() {
    let source = "if a ~= b then print(1) end\n";
    let lowered = lower(source, Dialect::Lua53, Dialect::Lua51).expect("lower");
    assert_eq!(lowered.text, source);
    assert!(lowered.polyfills.is_empty());
}

#[test]
fn pure_51_backend_uses_bitpair_core() {
    let lowered = lower("x = a & b\n", Dialect::Lua53, Dialect::Lua51).expect("lower");
    assert!(lowered.text.contains("local function bitpair"));
    assert!(lowered.text.contains("function M.band(a, b)"));
    assert_eq!(lowered.polyfills, vec!["band"]);
}

#[test]
fn luajit_target_backend_wraps_bit_library() {
    let lowered = lower("x = a & b\n", Dialect::Lua53, Dialect::LuaJit).expect("lower");
    assert!(lowered.text.contains("return bit.band(a, b) % 4294967296"));
}

#[test]
fn bitop_inside_floor_div_operand_renders_recursively() {
    assert_eq!(
        text("x = (a & b) // c\n", Dialect::Lua53, Dialect::Lua52),
        "local __luabox_rt = (function()\n\
         \x20 local M = {}\n\
         \x20 M.band = bit32.band\n\
         \x20 return M\n\
         end)()\n\
         \n\
         x = math.floor((__luabox_rt.band(a, b)) / c)\n"
    );
}

// === goto restructuring ====================================================

#[test]
fn backward_goto_as_repeat_until() {
    let source = "\
local i = 0
::top::
i = i + 1
if i < 3 then goto top end
print(i)
";
    let expected = "\
local i = 0
repeat
i = i + 1
until not (i < 3)
print(i)
";
    assert_eq!(text(source, Dialect::Lua54, Dialect::Lua51), expected);
}

#[test]
fn backward_goto_condition_is_rendered_through_expression_rules() {
    let source = "\
::top::
step()
if n // 2 > 0 then goto top end
";
    let out = text(source, Dialect::Lua54, Dialect::Lua51);
    assert!(out.contains("until not (math.floor(n / 2) > 0)"), "{out}");
}

#[test]
fn unconditional_backward_goto_as_while_true() {
    let source = "\
::top::
work()
goto top
";
    let expected = "\
while true do
work()
end
";
    assert_eq!(text(source, Dialect::Lua52, Dialect::Lua51), expected);
}

#[test]
fn forward_goto_as_skip_flag() {
    let source = "\
local i = 1
while i < 10 do
  if i % 2 == 0 then goto continue end
  print(i)
  ::continue::
  i = i + 1
end
";
    let expected = "\
local i = 1
while i < 10 do
  local __luabox_skip_1 = false
  if i % 2 == 0 then __luabox_skip_1 = true end
  if not __luabox_skip_1 then
  print(i)
  end
  i = i + 1
end
";
    assert_eq!(text(source, Dialect::Lua54, Dialect::Lua51), expected);
}

#[test]
fn two_forward_gotos_to_one_label_nest_their_wrappers() {
    let source = "\
while true do
  if a then goto continue end
  mid()
  if b then goto continue end
  work()
  ::continue::
end
";
    let expected = "\
while true do
  local __luabox_skip_1 = false
  if a then __luabox_skip_1 = true end
  if not __luabox_skip_1 then
  mid()
  local __luabox_skip_2 = false
  if b then __luabox_skip_2 = true end
  if not __luabox_skip_2 then
  work()
  end
  end
end
";
    assert_eq!(text(source, Dialect::Lua54, Dialect::Lua51), expected);
}

#[test]
fn direct_unconditional_forward_goto_skips_region() {
    let source = "\
goto done
print(\"dead\")
::done::
print(\"alive\")
";
    let expected = "\
local __luabox_skip_1 = true
if not __luabox_skip_1 then
print(\"dead\")
end
print(\"alive\")
";
    assert_eq!(text(source, Dialect::Lua52, Dialect::Lua51), expected);
}

#[test]
fn unreferenced_label_is_deleted() {
    let source = "::unused::\nprint(1)\n";
    assert_eq!(text(source, Dialect::Lua52, Dialect::Lua51), "\nprint(1)\n");
}

#[test]
fn goto_out_of_a_loop_is_irreducible() {
    let source = "\
while true do
  goto out
end
::out::
";
    assert_eq!(
        error_codes(source, Dialect::Lua54, Dialect::Lua51),
        vec!["LB0601"]
    );
}

#[test]
fn two_backward_gotos_to_one_label_are_irreducible() {
    let source = "\
::top::
if a then goto top end
if b then goto top end
";
    let codes = error_codes(source, Dialect::Lua52, Dialect::Lua51);
    assert!(codes.iter().all(|c| *c == "LB0601"), "{codes:?}");
}

#[test]
fn backward_goto_with_else_clause_is_irreducible() {
    let source = "\
::top::
if a then goto top else print(1) end
";
    assert_eq!(
        error_codes(source, Dialect::Lua52, Dialect::Lua51),
        vec!["LB0601"]
    );
}

#[test]
fn backward_region_with_naked_break_is_irreducible() {
    let source = "\
while outer do
  ::top::
  if x then break end
  if a then goto top end
end
";
    assert_eq!(
        error_codes(source, Dialect::Lua52, Dialect::Lua51),
        vec!["LB0601"]
    );
}

#[test]
fn backward_region_break_inside_inner_loop_is_fine() {
    let source = "\
::top::
while inner do
  break
end
if a then goto top end
";
    let out = text(source, Dialect::Lua52, Dialect::Lua51);
    assert!(out.starts_with("repeat"), "{out}");
}

#[test]
fn unconditional_backward_goto_not_last_is_irreducible() {
    let source = "\
::top::
work()
goto top
print(\"unreachable\")
";
    assert_eq!(
        error_codes(source, Dialect::Lua52, Dialect::Lua51),
        vec!["LB0601"]
    );
}

#[test]
fn deeply_nested_goto_is_irreducible() {
    let source = "\
if a then
  if b then goto out end
end
::out::
";
    assert_eq!(
        error_codes(source, Dialect::Lua52, Dialect::Lua51),
        vec!["LB0601"]
    );
}

#[test]
fn goto_with_no_matching_label_anywhere_is_irreducible() {
    assert_eq!(
        error_codes("goto nowhere\n", Dialect::Lua52, Dialect::Lua51),
        vec!["LB0601"]
    );
}

#[test]
fn goto_cannot_see_a_label_outside_its_function() {
    // Label visibility stops at the enclosing function, exactly as in Lua.
    let source = "local function f()\n  goto out\nend\n::out::\nf()\n";
    assert_eq!(
        error_codes(source, Dialect::Lua52, Dialect::Lua51),
        vec!["LB0601"]
    );
}

#[test]
fn a_goto_that_is_not_its_branchs_last_statement_is_irreducible() {
    let source = "\
if a then
  goto out
  print(\"after\")
end
::out::
";
    assert_eq!(
        error_codes(source, Dialect::Lua52, Dialect::Lua51),
        vec!["LB0601"]
    );
}

#[test]
fn a_backward_goto_sharing_its_branch_is_irreducible() {
    let source = "\
::top::
if a then
  step()
  goto top
end
";
    assert_eq!(
        error_codes(source, Dialect::Lua52, Dialect::Lua51),
        vec!["LB0601"]
    );
}

#[test]
fn interleaved_forward_regions_are_irreducible() {
    // Region A spans [0, 3], region B spans [1, 4]: they overlap without
    // nesting, so their skip wrappers could not be emitted as a valid tree.
    let source = "\
if a then goto one end
if b then goto two end
work()
::one::
::two::
";
    assert_eq!(
        error_codes(source, Dialect::Lua52, Dialect::Lua51),
        vec!["LB0601"]
    );
}

#[test]
fn a_forward_region_strictly_nested_in_a_backward_one_is_fine() {
    // Backward region [0, 4] strictly contains forward region [1, 3].
    let source = "\
::top::
if a then goto skip end
work()
::skip::
if b then goto top end
";
    let out = text(source, Dialect::Lua52, Dialect::Lua51);
    assert!(out.starts_with("repeat"), "{out}");
    assert!(out.contains("until not (b)"), "{out}");
    assert!(out.contains("local __luabox_skip_1 = false"), "{out}");
    assert!(!out.contains("goto"), "{out}");
}

#[test]
fn a_break_in_a_loop_nested_in_the_backward_region_is_fine() {
    // The `break` sits inside a `while` that is itself inside an `if` in the
    // region: it binds to that `while`, not to the inserted `repeat`.
    let source = "\
::top::
if c then
  while d do break end
end
if a then goto top end
";
    let out = text(source, Dialect::Lua52, Dialect::Lua51);
    assert!(out.starts_with("repeat"), "{out}");
    assert!(out.contains("until not (a)"), "{out}");
}

#[test]
fn a_break_in_a_function_inside_the_backward_region_is_fine() {
    // Function bodies are their own boundary; the `break` there belongs to a
    // loop the rewrite never sees.
    let source = "\
::top::
local f = function() while d do break end end
if a then goto top end
";
    let out = text(source, Dialect::Lua52, Dialect::Lua51);
    assert!(out.starts_with("repeat"), "{out}");
}

#[test]
fn goto_untouched_when_target_has_goto() {
    let source = "::top::\ni = i + 1\nif i < 3 then goto top end\n";
    assert_eq!(text(source, Dialect::Lua54, Dialect::Lua52), source);
}

#[test]
fn goto_in_elseif_branch_forward_skips() {
    let source = "\
do
  if a then
    p()
  elseif b then
    goto out
  end
  q()
  ::out::
end
";
    let expected = "\
do
  local __luabox_skip_1 = false
  if a then
    p()
  elseif b then
    __luabox_skip_1 = true
  end
  if not __luabox_skip_1 then
  q()
  end
end
";
    assert_eq!(text(source, Dialect::Lua54, Dialect::Lua51), expected);
}

// === <const> ===============================================================

#[test]
fn const_attribute_is_dropped() {
    assert_eq!(
        text(
            "local x <const> = 1\nprint(x)\n",
            Dialect::Lua54,
            Dialect::Lua53
        ),
        "local x = 1\nprint(x)\n"
    );
}

#[test]
fn const_reassignment_is_lb0602() {
    assert_eq!(
        error_codes(
            "local x <const> = 1\nx = 2\n",
            Dialect::Lua54,
            Dialect::Lua51
        ),
        vec!["LB0602"]
    );
}

#[test]
fn const_reassignment_inside_closure_is_lb0602() {
    assert_eq!(
        error_codes(
            "local x <const> = 1\nlocal f = function() x = 2 end\n",
            Dialect::Lua54,
            Dialect::Lua53
        ),
        vec!["LB0602"]
    );
}

#[test]
fn const_function_decl_sugar_is_lb0602() {
    assert_eq!(
        error_codes(
            "local x <const> = 1\nfunction x() end\n",
            Dialect::Lua54,
            Dialect::Lua53
        ),
        vec!["LB0602"]
    );
}

#[test]
fn const_shadowed_then_assigned_is_fine() {
    assert_eq!(
        text(
            "local x <const> = 1\nlocal x = 2\nx = 3\n",
            Dialect::Lua54,
            Dialect::Lua53
        ),
        "local x = 1\nlocal x = 2\nx = 3\n"
    );
}

#[test]
fn const_shadowed_by_param_is_fine() {
    let source = "local x <const> = 1\nlocal f = function(x) x = 2 end\nf(x)\n";
    assert_eq!(
        text(source, Dialect::Lua54, Dialect::Lua53),
        "local x = 1\nlocal f = function(x) x = 2 end\nf(x)\n"
    );
}

#[test]
fn const_shadowed_by_for_var_is_fine() {
    let source = "local x <const> = 1\nfor x = 1, 3 do x = x + 1 end\n";
    assert_eq!(
        text(source, Dialect::Lua54, Dialect::Lua53),
        "local x = 1\nfor x = 1, 3 do x = x + 1 end\n"
    );
}

#[test]
fn assignment_to_other_names_is_fine() {
    let source = "local x <const> = 1\ny = x\nt.x = 2\n";
    assert_eq!(
        text(source, Dialect::Lua54, Dialect::Lua53),
        "local x = 1\ny = x\nt.x = 2\n"
    );
}

// === <close> ===============================================================

#[test]
fn close_rewrite_shape_and_warning() {
    let source = "\
do
  local h <close> = open()
  use(h)
end
";
    let lowered = lower(source, Dialect::Lua54, Dialect::Lua51).expect("lower");
    let expected_tail = "\
do
  local h = open()
  __luabox_rt.close_scope(h, function()
  use(h)
  end)
end
";
    assert!(lowered.text.ends_with(expected_tail), "{}", lowered.text);
    assert!(lowered.text.contains("function M.close_scope"));
    assert_eq!(lowered.polyfills, vec!["close_scope"]);
    assert_eq!(
        lowered.warnings.iter().map(|w| w.code).collect::<Vec<_>>(),
        vec!["LB0603"]
    );
}

#[test]
fn close_warning_suppressed_by_allow_annotation() {
    let source = "\
do
  ---@luabox-allow lossy-lowering
  local h <close> = open()
  use(h)
end
";
    let lowered = lower(source, Dialect::Lua54, Dialect::Lua51).expect("lower");
    assert!(
        lowered.warnings.iter().all(|w| w.code != "LB0603"),
        "{:?}",
        lowered.warnings
    );
    assert!(
        lowered
            .text
            .contains("__luabox_rt.close_scope(h, function()")
    );
}

#[test]
fn two_closes_in_one_block_nest_in_reverse_order() {
    let source = "\
do
  local a <close> = open1()
  mid()
  local b <close> = open2()
  fin()
end
";
    let out = text(source, Dialect::Lua54, Dialect::Lua51);
    let expected_tail = "\
do
  local a = open1()
  __luabox_rt.close_scope(a, function()
  mid()
  local b = open2()
  __luabox_rt.close_scope(b, function()
  fin()
  end)
  end)
end
";
    assert!(out.ends_with(expected_tail), "{out}");
}

#[test]
fn close_with_return_in_tail_is_hard_lb0603() {
    let source = "\
local function f()
  local h <close> = open()
  return h
end
";
    assert_eq!(
        error_codes(source, Dialect::Lua54, Dialect::Lua51),
        vec!["LB0603"]
    );
}

#[test]
fn close_with_vararg_in_tail_is_hard_lb0603() {
    let source = "\
local function f(...)
  local h <close> = open()
  use(...)
end
";
    assert_eq!(
        error_codes(source, Dialect::Lua54, Dialect::Lua51),
        vec!["LB0603"]
    );
}

#[test]
fn close_with_naked_break_in_tail_is_hard_lb0603() {
    let source = "\
while true do
  local h <close> = open()
  break
end
";
    assert_eq!(
        error_codes(source, Dialect::Lua54, Dialect::Lua51),
        vec!["LB0603"]
    );
}

#[test]
fn close_break_inside_inner_loop_in_tail_is_fine() {
    let source = "\
do
  local h <close> = open()
  while p() do break end
end
";
    let lowered = lower(source, Dialect::Lua54, Dialect::Lua51).expect("lower");
    assert!(lowered.text.contains("close_scope"));
}

#[test]
fn close_in_multi_name_local_is_hard_lb0603() {
    let source = "local a, h <close> = 1, open()\n";
    assert_eq!(
        error_codes(source, Dialect::Lua54, Dialect::Lua51),
        vec!["LB0603"]
    );
}

#[test]
fn close_with_empty_tail_still_closes() {
    let source = "\
do
  local h <close> = open()
end
";
    let out = text(source, Dialect::Lua54, Dialect::Lua51);
    let expected_tail = "\
do
  local h = open()
  __luabox_rt.close_scope(h, function()
  end)
end
";
    assert!(out.ends_with(expected_tail), "{out}");
}

#[test]
fn close_straddled_by_goto_region_is_hard_lb0603() {
    // The backward-goto repeat wrapper would interleave with the close
    // closure (the goto also jumps out of the scope tail): rejected, not
    // silently broken.
    let source = "\
do
  ::top::
  local h <close> = open()
  if c then goto top end
end
";
    let codes = error_codes(source, Dialect::Lua54, Dialect::Lua51);
    assert!(codes.contains(&"LB0603"), "{codes:?}");
}

#[test]
fn close_with_goto_pair_fully_inside_tail_is_fine() {
    let source = "\
do
  local h <close> = open()
  ::again::
  step(h)
  if more() then goto again end
end
";
    let out = text(source, Dialect::Lua54, Dialect::Lua51);
    assert!(out.contains("close_scope"), "{out}");
    assert!(out.contains("repeat"), "{out}");
    assert!(!out.contains("goto"), "{out}");
}

#[test]
fn const_reassigned_from_a_local_function_body_is_lb0602() {
    let source = "local x <const> = 1\nlocal function f()\n  x = 2\nend\nf()\n";
    assert_eq!(
        error_codes(source, Dialect::Lua54, Dialect::Lua53),
        vec!["LB0602"]
    );
}

#[test]
fn const_shadowed_by_a_local_function_of_the_same_name_is_fine() {
    // `local function x` is in scope inside its own body, so the constant is
    // no longer visible from that point on.
    let source = "local x <const> = 1\nlocal function x()\n  x = 2\nend\nx()\n";
    assert_eq!(
        text(source, Dialect::Lua54, Dialect::Lua53),
        "local x = 1\nlocal function x()\n  x = 2\nend\nx()\n"
    );
}

#[test]
fn const_reassigned_inside_a_for_body_is_lb0602() {
    for source in [
        "local x <const> = 1\nfor i = 1, 3 do x = i end\n",
        "local x <const> = 1\nfor k in pairs(t) do x = k end\n",
    ] {
        assert_eq!(
            error_codes(source, Dialect::Lua54, Dialect::Lua53),
            vec!["LB0602"],
            "{source}"
        );
    }
}

#[test]
fn const_reassigned_in_an_elseif_or_else_branch_is_lb0602() {
    let source =
        "local x <const> = 1\nif a then\n  p()\nelseif b then\n  x = 2\nelse\n  x = 3\nend\n";
    assert_eq!(
        error_codes(source, Dialect::Lua54, Dialect::Lua53),
        vec!["LB0602", "LB0602"]
    );
}

#[test]
fn const_reassigned_from_a_closure_in_value_position_is_lb0602() {
    // The assignment hides in the *values* list of another assignment, and
    // inside a function expression at that: the scan follows both.
    let source = "local x <const> = 1\ny = (function() x = 2 end)()\n";
    assert_eq!(
        error_codes(source, Dialect::Lua54, Dialect::Lua53),
        vec!["LB0602"]
    );
}

#[test]
fn const_reassigned_through_an_index_target_is_lb0602() {
    // `t[(function() x = 2 end)()] = 1`: the constant is written from inside
    // a non-name assignment *target*, which the scan also descends into.
    let source = "local x <const> = 1\nt[(function() x = 2 end)()] = 1\n";
    assert_eq!(
        error_codes(source, Dialect::Lua54, Dialect::Lua53),
        vec!["LB0602"]
    );
}

#[test]
fn close_tail_containing_a_nested_function_is_fine() {
    // Function bodies are their own boundary: a `return` inside one does not
    // cross the pcall wrapper.
    let source = "\
do
  local h <close> = open()
  local function g()
    return h
  end
  g()
end
";
    let out = text(source, Dialect::Lua54, Dialect::Lua51);
    assert!(
        out.contains("__luabox_rt.close_scope(h, function()"),
        "{out}"
    );
    assert!(out.contains("local function g()"), "{out}");
}

#[test]
fn close_tail_break_inside_a_nested_loop_is_fine() {
    // The `break` binds to the `while` the rewrite does not touch, two levels
    // down inside an `if` — not to anything the wrapper introduces.
    let source = "\
do
  local h <close> = open()
  if c then
    while p() do break end
  end
end
";
    let out = text(source, Dialect::Lua54, Dialect::Lua51);
    assert!(out.contains("close_scope"), "{out}");
}

#[test]
fn close_allow_annotation_may_trail_on_the_declaration_line() {
    let source = "\
do
  local h <close> = open() ---@luabox-allow lossy-lowering
  use(h)
end
";
    let lowered = lower(source, Dialect::Lua54, Dialect::Lua51).expect("lower");
    assert!(
        lowered.warnings.iter().all(|w| w.code != "LB0603"),
        "{:?}",
        lowered.warnings
    );
    assert!(lowered.text.contains("close_scope"), "{}", lowered.text);
}

#[test]
fn a_trailing_comment_without_the_marker_does_not_suppress() {
    let source = "\
do
  local h <close> = open() -- just a comment
  use(h)
end
";
    let lowered = lower(source, Dialect::Lua54, Dialect::Lua51).expect("lower");
    assert_eq!(
        lowered.warnings.iter().map(|w| w.code).collect::<Vec<_>>(),
        vec!["LB0603"]
    );
}

#[test]
fn an_env_local_consumed_by_the_env_rule_is_not_reprocessed_for_attributes() {
    // Both rules are active for 5.4 -> 5.1; `env` replaces the whole
    // statement first, so `attribs` must skip the `LOCAL_NAME` inside it.
    assert_eq!(
        text("local _ENV = t\nx = 1\n", Dialect::Lua54, Dialect::Lua51),
        "setfenv(1, t)\nx = 1\n"
    );
}

// === _ENV ==================================================================

#[test]
fn local_env_at_chunk_top_becomes_setfenv() {
    assert_eq!(
        text("local _ENV = t\nx = 1\n", Dialect::Lua52, Dialect::Lua51),
        "setfenv(1, t)\nx = 1\n"
    );
}

#[test]
fn local_env_in_function_body_becomes_setfenv() {
    let source = "\
local function m(t)
  local _ENV = t
  x = 1
end
";
    let expected = "\
local function m(t)
  setfenv(1, t)
  x = 1
end
";
    assert_eq!(text(source, Dialect::Lua52, Dialect::Lua51), expected);
}

#[test]
fn env_read_becomes_getfenv() {
    assert_eq!(
        text("print(_ENV)\n", Dialect::Lua52, Dialect::Lua51),
        "print(getfenv(1))\n"
    );
    assert_eq!(
        text("_ENV.x = 1\n", Dialect::Lua53, Dialect::Lua51),
        "getfenv(1).x = 1\n"
    );
}

#[test]
fn env_whole_assignment_becomes_setfenv() {
    assert_eq!(
        text("_ENV = t\n", Dialect::Lua52, Dialect::Lua51),
        "setfenv(1, t)\n"
    );
}

#[test]
fn env_multi_name_local_is_lb0604() {
    assert_eq!(
        error_codes("local _ENV, x = a, b\n", Dialect::Lua52, Dialect::Lua51),
        vec!["LB0604"]
    );
}

#[test]
fn env_local_in_nested_do_block_is_lb0604() {
    assert_eq!(
        error_codes(
            "do\n  local _ENV = t\nend\n",
            Dialect::Lua52,
            Dialect::Lua51
        ),
        vec!["LB0604"]
    );
}

#[test]
fn env_parameter_is_lb0604() {
    assert_eq!(
        error_codes(
            "local f = function(_ENV) x = 1 end\n",
            Dialect::Lua52,
            Dialect::Lua51
        ),
        vec!["LB0604"]
    );
}

#[test]
fn env_untouched_when_target_has_env() {
    let source = "local _ENV = t\nx = 1\n";
    assert_eq!(text(source, Dialect::Lua54, Dialect::Lua52), source);
}

#[test]
fn env_read_in_an_assignment_value_list_becomes_getfenv() {
    // `_ENV` in the *values* list is a read (only the first expression list
    // of an assignment is its target list).
    assert_eq!(
        text("t = _ENV\n", Dialect::Lua52, Dialect::Lua51),
        "t = getfenv(1)\n"
    );
    assert_eq!(
        text("a, b = _ENV, 1\n", Dialect::Lua52, Dialect::Lua51),
        "a, b = getfenv(1), 1\n"
    );
}

#[test]
fn env_multi_target_assignment_is_lb0604() {
    assert_eq!(
        error_codes("_ENV, x = a, b\n", Dialect::Lua52, Dialect::Lua51),
        vec!["LB0604"]
    );
}

#[test]
fn env_assignment_with_multiple_values_is_lb0604() {
    assert_eq!(
        error_codes("_ENV = a, b\n", Dialect::Lua52, Dialect::Lua51),
        vec!["LB0604"]
    );
}

#[test]
fn env_assignment_in_a_nested_block_is_lb0604() {
    assert_eq!(
        error_codes("do\n  _ENV = t\nend\n", Dialect::Lua52, Dialect::Lua51),
        vec!["LB0604"]
    );
}

#[test]
fn env_local_without_a_value_is_lb0604() {
    assert_eq!(
        error_codes("local _ENV\nx = 1\n", Dialect::Lua52, Dialect::Lua51),
        vec!["LB0604"]
    );
}

#[test]
fn env_value_is_rendered_through_the_expression_rules() {
    assert_eq!(
        text("local _ENV = t[a // b]\n", Dialect::Lua53, Dialect::Lua51),
        "setfenv(1, t[math.floor(a / b)])\n"
    );
}

#[test]
fn env_name_in_51_source_is_just_a_global() {
    // 5.1 has no _ENV semantics; a global named _ENV stays untouched.
    let source = "print(_ENV)\n";
    assert_eq!(text(source, Dialect::Lua51, Dialect::Lua54), source);
}

// === LuaJIT extensions =====================================================

#[test]
fn jit_bit_calls_map_to_rt_helpers() {
    let lowered = lower("x = bit.band(a, 3)\n", Dialect::LuaJit, Dialect::Lua51).expect("lower");
    assert!(
        lowered.text.ends_with("x = __luabox_rt.band(a, 3)\n"),
        "{}",
        lowered.text
    );
    assert_eq!(lowered.polyfills, vec!["band"]);
    assert!(
        lowered.text.contains("tobit32"),
        "JIT-source band is the signed family"
    );
}

#[test]
fn jit_bit_lshift_keeps_masked_count_semantics() {
    let lowered = lower("x = bit.lshift(a, n)\n", Dialect::LuaJit, Dialect::Lua51).expect("lower");
    assert!(lowered.text.contains("2 ^ (n % 32)"));
    assert!(lowered.text.ends_with("x = __luabox_rt.lshift(a, n)\n"));
}

#[test]
fn jit_first_class_bit_member_is_rewritten() {
    let lowered = lower("local f = bit.bxor\n", Dialect::LuaJit, Dialect::Lua51).expect("lower");
    assert!(lowered.text.ends_with("local f = __luabox_rt.bxor\n"));
}

#[test]
fn jit_require_bit_becomes_the_rt_table() {
    let lowered = lower(
        "local bit = require(\"bit\")\nx = bit.band(a, b)\n",
        Dialect::LuaJit,
        Dialect::Lua51,
    )
    .expect("lower");
    assert!(
        lowered.text.contains("local bit = __luabox_rt\n"),
        "{}",
        lowered.text
    );
    // The whole module family rides along: no member-level shaking possible.
    assert!(lowered.polyfills.contains(&"tohex"));
    assert!(lowered.polyfills.contains(&"bswap"));
}

#[test]
fn jit_ffi_require_is_lb0605() {
    assert_eq!(
        error_codes(
            "local ffi = require(\"ffi\")\n",
            Dialect::LuaJit,
            Dialect::Lua51
        ),
        vec!["LB0605"]
    );
}

#[test]
fn jit_unknown_bit_member_is_lb0605() {
    assert_eq!(
        error_codes("x = bit.frobnicate(1)\n", Dialect::LuaJit, Dialect::Lua51),
        vec!["LB0605"]
    );
}

#[test]
fn jit_64bit_literal_is_lb0605() {
    assert_eq!(
        error_codes("local n = 42LL\n", Dialect::LuaJit, Dialect::Lua51),
        vec!["LB0605"]
    );
}

#[test]
fn a_table_that_merely_looks_like_bit_is_untouched() {
    // Only the exact base name `bit` is the LuaJIT module.
    let source = "x = mybit.band(a, 3)\ny = t.bit.band(a, 3)\n";
    let lowered = lower(source, Dialect::LuaJit, Dialect::Lua51).expect("lower");
    assert_eq!(lowered.text, source);
    assert!(lowered.polyfills.is_empty());
}

#[test]
fn only_a_literal_single_argument_require_is_recognised() {
    // Neither of these is `require "<literal>"`, so nothing is rewritten and
    // nothing is diagnosed.
    let source = "load(\"bit\")\nlocal m = require(\"bit\", \"extra\")\nprint(m)\n";
    let lowered = lower(source, Dialect::LuaJit, Dialect::Lua51).expect("lower");
    assert_eq!(lowered.text, source);
    assert!(lowered.polyfills.is_empty());
}

#[test]
fn jit_bit_calls_left_alone_when_targeting_luajit() {
    let source = "x = bit.band(a, 3)\n";
    assert_eq!(text(source, Dialect::LuaJit, Dialect::LuaJit), source);
}

// === integer/float divergence =============================================

#[test]
fn big_integer_literal_warns_lb0606() {
    let warnings = warning_codes(
        "local n = 9007199254740993\n",
        Dialect::Lua53,
        Dialect::Lua51,
    );
    assert_eq!(warnings, vec!["LB0606"]);
}

#[test]
fn exact_2_pow_53_does_not_warn() {
    let warnings = warning_codes(
        "local n = 9007199254740992\n",
        Dialect::Lua53,
        Dialect::Lua51,
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn string_format_percent_d_warns_lb0606() {
    let warnings = warning_codes(
        "s = string.format(\"%d items\", n)\n",
        Dialect::Lua54,
        Dialect::Lua51,
    );
    assert_eq!(warnings, vec!["LB0606"]);
}

#[test]
fn string_format_escaped_percent_does_not_warn() {
    let warnings = warning_codes(
        "s = string.format(\"100%%done\", n)\n",
        Dialect::Lua54,
        Dialect::Lua51,
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn a_percent_d_after_other_directives_still_warns() {
    let warnings = warning_codes(
        "s = string.format(\"%s scored %5.2f then %d\", who, score, n)\n",
        Dialect::Lua54,
        Dialect::Lua51,
    );
    assert_eq!(warnings, vec!["LB0606"]);
}

#[test]
fn format_strings_without_an_integer_directive_do_not_warn() {
    let warnings = warning_codes(
        "s = string.format(\"%s / %q / %.3f\", a, b, c)\n",
        Dialect::Lua54,
        Dialect::Lua51,
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn only_string_format_is_inspected_for_integer_directives() {
    // Same `%d` payload, but neither call is `string.format`.
    let warnings = warning_codes(
        "s = string.rep(\"%d\", 3)\nt = fmt.format(\"%d\", n)\n",
        Dialect::Lua54,
        Dialect::Lua51,
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn no_divergence_warnings_when_target_has_integers() {
    let warnings = warning_codes(
        "local n = 9007199254740993\ns = string.format(\"%d\", n)\n",
        Dialect::Lua54,
        Dialect::Lua53,
    );
    assert!(warnings.is_empty(), "{warnings:?}");
}

// === formatting preservation ==============================================

#[test]
fn untransformed_formatting_survives_verbatim() {
    let source = "\
-- header comment\nlocal weird   =   { 1,2,   3 }\n\nx = a // b -- trailing\n";
    let out = text(source, Dialect::Lua53, Dialect::Lua51);
    assert_eq!(
        out,
        "-- header comment\nlocal weird   =   { 1,2,   3 }\n\nx = math.floor(a / b) -- trailing\n"
    );
}

#[test]
fn parse_error_input_is_rejected_with_lb0001() {
    assert_eq!(
        error_codes("local = = =\n", Dialect::Lua54, Dialect::Lua51),
        vec!["LB0001"; error_codes("local = = =\n", Dialect::Lua54, Dialect::Lua51).len()]
    );
}

// === bare lowering / hoisted prelude (the bundler's entry points) ==========

#[test]
fn lower_bare_lists_helpers_but_emits_no_prelude() {
    let bare = lower_bare("x = a & b\n", Dialect::Lua53, Dialect::Lua52).expect("lower_bare");
    assert_eq!(bare.text, "x = __luabox_rt.band(a, b)\n");
    assert_eq!(bare.polyfills, vec!["band"]);
    assert!(
        !bare.text.contains("local __luabox_rt"),
        "bare output carries no prelude: {}",
        bare.text
    );
    // `lower` is the same rewrite with the prelude prepended.
    let full = lower("x = a & b\n", Dialect::Lua53, Dialect::Lua52).expect("lower");
    assert!(full.text.ends_with(&bare.text), "{}", full.text);
    assert_eq!(full.polyfills, bare.polyfills);
}

#[test]
fn lower_bare_is_identity_for_the_same_dialect_too() {
    let bare = lower_bare(CORPUS_51, Dialect::Lua51, Dialect::Lua51).expect("identity");
    assert_eq!(bare.text, CORPUS_51);
    assert!(bare.polyfills.is_empty());
}

#[test]
fn lower_bare_reports_the_same_errors_as_lower() {
    let source = "local x <const> = 1\nx = 2\n";
    let diags = lower_bare(source, Dialect::Lua54, Dialect::Lua51).expect_err("const reassigned");
    assert_eq!(
        diags.iter().map(|d| d.code).collect::<Vec<_>>(),
        vec!["LB0602"]
    );
}

#[test]
fn rt_prelude_renders_the_union_of_two_modules_helper_sets() {
    let a = lower_bare("x = a & b\n", Dialect::Lua53, Dialect::Lua52).expect("a");
    let b = lower_bare("y = c | d\n", Dialect::Lua53, Dialect::Lua52).expect("b");
    let used: BTreeSet<Helper> = a
        .polyfills
        .iter()
        .chain(&b.polyfills)
        .filter_map(|n| Helper::from_name(n))
        .collect();
    let prelude = rt_prelude(&used, Dialect::Lua53, Dialect::Lua52).expect("prelude");
    assert_eq!(
        prelude,
        "local __luabox_rt = (function()\n  local M = {}\n  M.band = bit32.band\n  \
         M.bor = bit32.bor\n  return M\nend)()\n\n"
    );
    assert_eq!(
        rt_prelude(&BTreeSet::new(), Dialect::Lua53, Dialect::Lua52),
        None
    );
}

#[test]
fn helper_names_round_trip_and_reject_strangers() {
    for helper in [
        Helper::Band,
        Helper::Bor,
        Helper::Bxor,
        Helper::Bnot,
        Helper::Shl,
        Helper::Shr,
        Helper::CloseScope,
        Helper::Tobit,
        Helper::Lshift,
        Helper::Rshift,
        Helper::Arshift,
        Helper::Rol,
        Helper::Ror,
        Helper::Bswap,
        Helper::Tohex,
    ] {
        assert_eq!(Helper::from_name(helper.name()), Some(helper));
    }
    assert_eq!(Helper::from_name("frobnicate"), None);
    assert_eq!(Helper::from_name(""), None);
}

#[test]
fn close_scope_body_is_shared_by_the_luajit_source_family_too() {
    // `<close>` is a 5.4-source construct and `bit.*` a LuaJIT-source one,
    // so they never co-occur in a real file — but the backend selection must
    // still fall through to the family-independent body.
    let used = BTreeSet::from([Helper::CloseScope]);
    let text = rt_prelude(&used, Dialect::LuaJit, Dialect::Lua51).expect("prelude");
    assert!(text.contains("function M.close_scope(v, body)"), "{text}");
    assert!(text.contains("mt.__close(v, err)"), "{text}");
    assert!(
        !text.contains("tobit32"),
        "close_scope needs no core: {text}"
    );
}

// === indentation of inserted wrappers ======================================

#[test]
fn a_construct_sharing_its_line_gets_no_wrapper_indent() {
    // `local h <close>` does not start its line, so `indent_at` yields the
    // empty string and the wrapper lines are emitted flush left.
    let source = "do local h <close> = open() use(h) end\n";
    let out = text(source, Dialect::Lua54, Dialect::Lua51);
    assert!(
        out.ends_with(
            "do local h = open()\n__luabox_rt.close_scope(h, function() use(h)\nend) end\n"
        ),
        "{out}"
    );
}
