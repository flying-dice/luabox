//! Exact-output tests for the lowering matrix (SPEC.md §2.1): every rule's
//! rewrite is asserted byte-for-byte, plus tree-shaking, diagnostics, and
//! idempotence invariants. Semantics arguments live in each rule module's
//! doc comment; these tests pin the mechanical output they argue about.

use luabox_lower::{Severity, lower};
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
    assert!(lowered.text.contains("return bit.band(a, b) % 0x100000000"));
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
