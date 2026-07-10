//! The `__luabox_rt` polyfill module (SPEC.md §2.1).
//!
//! A single prelude, emitted as a `local` at chunk top:
//!
//! ```lua
//! local __luabox_rt = (function()
//!   local M = {}
//!   -- only the helpers the file actually uses --
//!   return M
//! end)()
//! ```
//!
//! Tree-shaken: only used helpers (plus their private cores) are included,
//! and when nothing is used no prelude is emitted at all — zero-cost by
//! construction. Being a plain `local`, it cannot collide with user globals
//! and adds no `require` edge (the bundler dedupes it across a bundle).
//!
//! # Backends and semantics
//!
//! Helper bodies are selected per target:
//!
//! - **5.2 target** — `bit32.*` (its shift semantics — |n| ≥ 32 yields 0,
//!   negative n shifts the other way — match Lua 5.3's, restricted to 32
//!   bits).
//! - **LuaJIT target** — `bit.*`, normalized to unsigned via `% 2^32` and
//!   guarded to 5.3 shift-count semantics (`bit` masks counts to 5 bits and
//!   returns signed values; the wrappers paper over both).
//! - **5.1 target** — pure-Lua fallback over `math.floor` arithmetic.
//!
//! When the *source* is LuaJIT (lowering `bit.*` library calls, not 5.3
//! operators) the helpers reproduce `bit`'s **signed** 32-bit results via
//! `tobit` normalization, so `bit.band(-1, -1) == -1` keeps holding. The two
//! families can never collide: a file is either 5.3+ (operators) or LuaJIT
//! (`bit.*`), never both.
//!
//! Documented caveat (SPEC.md §2.1 diagnostic tiers): Lua 5.3 bitwise
//! operators are 64-bit; every shim here is 32-bit (that is all `bit32`,
//! `bit`, and doubles can offer). Operands with significant bits above 2^32
//! diverge — see the `LB0606` explain page.

use std::collections::BTreeSet;

use luabox_syntax::Dialect;

/// One `__luabox_rt` helper. The `Ord` order is the emission order inside
/// the prelude.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Helper {
    /// `a & b` / `bit.band`.
    Band,
    /// `a | b` / `bit.bor`.
    Bor,
    /// `a ~ b` / `bit.bxor`.
    Bxor,
    /// unary `~a` / `bit.bnot`.
    Bnot,
    /// `a << b` (5.3 shift semantics over 32 bits).
    Shl,
    /// `a >> b` (5.3 shift semantics over 32 bits).
    Shr,
    /// `<close>` scope-exit runner.
    CloseScope,
    /// `bit.tobit`.
    Tobit,
    /// `bit.lshift` (LuaJIT semantics: count masked to 5 bits).
    Lshift,
    /// `bit.rshift` (logical).
    Rshift,
    /// `bit.arshift` (arithmetic).
    Arshift,
    /// `bit.rol`.
    Rol,
    /// `bit.ror`.
    Ror,
    /// `bit.bswap`.
    Bswap,
    /// `bit.tohex`.
    Tohex,
}

impl Helper {
    /// The member name inside the emitted `__luabox_rt` table.
    pub fn name(self) -> &'static str {
        match self {
            Helper::Band => "band",
            Helper::Bor => "bor",
            Helper::Bxor => "bxor",
            Helper::Bnot => "bnot",
            Helper::Shl => "shl",
            Helper::Shr => "shr",
            Helper::CloseScope => "close_scope",
            Helper::Tobit => "tobit",
            Helper::Lshift => "lshift",
            Helper::Rshift => "rshift",
            Helper::Arshift => "arshift",
            Helper::Rol => "rol",
            Helper::Ror => "ror",
            Helper::Bswap => "bswap",
            Helper::Tohex => "tohex",
        }
    }

    /// The inverse of [`Helper::name`] — lets callers round-trip the
    /// [`crate::Lowered::polyfills`] name list back into helpers (the
    /// bundler unions per-module sets before rendering one shared prelude
    /// via [`crate::rt_prelude`]).
    pub fn from_name(name: &str) -> Option<Self> {
        const ALL: [Helper; 15] = [
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
        ];
        ALL.into_iter().find(|h| h.name() == name)
    }

    /// Every helper the LuaJIT `bit` module maps onto — what a rewritten
    /// `require("bit")` pulls in wholesale (member-level tree-shaking is
    /// impossible once the module table escapes into a local).
    pub(crate) const JIT_BIT_MODULE: [Helper; 12] = [
        Helper::Band,
        Helper::Bor,
        Helper::Bxor,
        Helper::Bnot,
        Helper::Tobit,
        Helper::Lshift,
        Helper::Rshift,
        Helper::Arshift,
        Helper::Rol,
        Helper::Ror,
        Helper::Bswap,
        Helper::Tohex,
    ];

    /// Signed-family helpers need the `tobit32` core when emitted pure.
    fn needs_tobit32(self) -> bool {
        !matches!(self, Helper::CloseScope | Helper::Shl | Helper::Shr)
    }

    /// Helpers built on the bit-by-bit combiner core.
    fn needs_bitpair(self) -> bool {
        matches!(self, Helper::Band | Helper::Bor | Helper::Bxor)
    }
}

/// Render the `__luabox_rt` prelude for the used helper set, or `None` when
/// the set is empty (zero-cost invariant: no helpers, no prelude).
pub(crate) fn prelude(used: &BTreeSet<Helper>, from: Dialect, to: Dialect) -> Option<String> {
    if used.is_empty() {
        return None;
    }
    let mut used = used.clone();
    if used.contains(&Helper::Ror) {
        used.insert(Helper::Rol); // ror is defined in terms of rol
    }
    let jit_source = from == Dialect::LuaJit;

    let mut out = String::from("local __luabox_rt = (function()\n  local M = {}\n");
    let pure = !matches!(to, Dialect::Lua52 | Dialect::LuaJit);
    if (pure || jit_source) && used.iter().any(|h| h.needs_bitpair()) {
        out.push_str(BITPAIR);
    }
    if jit_source && used.iter().any(|h| h.needs_tobit32()) {
        out.push_str(TOBIT32);
    }
    for helper in &used {
        out.push_str(body(*helper, to, jit_source));
    }
    out.push_str("  return M\nend)()\n\n");
    Some(out)
}

/// The private bit-by-bit combiner shared by pure `band`/`bor`/`bxor`.
const BITPAIR: &str = "  local function bitpair(a, b, f)
    a = a % 4294967296
    b = b % 4294967296
    local r, p = 0, 1
    for _ = 1, 32 do
      if f(a % 2, b % 2) then
        r = r + p
      end
      a = math.floor(a / 2)
      b = math.floor(b / 2)
      p = p * 2
    end
    return r
  end
";

/// The private signed-32-bit normalizer for the LuaJIT-source family.
const TOBIT32: &str = "  local function tobit32(x)
    x = x % 4294967296
    if x >= 0x80000000 then
      x = x - 4294967296
    end
    return x
  end
";

/// The body lines for one helper under the given target backend and source
/// family. LuaJIT-source (`jit_source`) helpers are always the pure signed
/// family — SPEC.md §2 only downgrades LuaJIT to 5.1.
fn body(helper: Helper, to: Dialect, jit_source: bool) -> &'static str {
    if jit_source {
        return jit_family(helper);
    }
    match to {
        Dialect::Lua52 => bit32_family(helper),
        Dialect::LuaJit => bitjit_family(helper),
        _ => pure_family(helper),
    }
}

/// Unsigned 32-bit family over `bit32` (5.2 target).
fn bit32_family(helper: Helper) -> &'static str {
    match helper {
        Helper::Band => "  M.band = bit32.band\n",
        Helper::Bor => "  M.bor = bit32.bor\n",
        Helper::Bxor => "  M.bxor = bit32.bxor\n",
        Helper::Bnot => "  M.bnot = bit32.bnot\n",
        Helper::Shl => "  M.shl = bit32.lshift\n",
        Helper::Shr => "  M.shr = bit32.rshift\n",
        other => common_family(other),
    }
}

/// Unsigned 32-bit family over LuaJIT's `bit` (luajit target): normalize
/// the signed results with `% 2^32` and widen the masked shift counts to
/// Lua 5.3 semantics (|n| ≥ 32 → 0, negative n shifts the other way).
fn bitjit_family(helper: Helper) -> &'static str {
    match helper {
        Helper::Band => {
            "  function M.band(a, b)
    return bit.band(a, b) % 4294967296
  end
"
        }
        Helper::Bor => {
            "  function M.bor(a, b)
    return bit.bor(a, b) % 4294967296
  end
"
        }
        Helper::Bxor => {
            "  function M.bxor(a, b)
    return bit.bxor(a, b) % 4294967296
  end
"
        }
        Helper::Bnot => {
            "  function M.bnot(a)
    return bit.bnot(a) % 4294967296
  end
"
        }
        Helper::Shl => {
            "  function M.shl(a, n)
    if n <= -32 or n >= 32 then return 0 end
    if n < 0 then return math.floor((a % 4294967296) / 2 ^ -n) end
    return bit.lshift(a, n) % 4294967296
  end
"
        }
        Helper::Shr => {
            "  function M.shr(a, n)
    if n <= -32 or n >= 32 then return 0 end
    if n < 0 then return ((a % 4294967296) * 2 ^ -n) % 4294967296 end
    return bit.rshift(a, n) % 4294967296
  end
"
        }
        other => common_family(other),
    }
}

/// Pure-Lua unsigned 32-bit family (5.1 target, 5.3+ source).
fn pure_family(helper: Helper) -> &'static str {
    match helper {
        Helper::Band => {
            "  function M.band(a, b)
    return bitpair(a, b, function(x, y) return x == 1 and y == 1 end)
  end
"
        }
        Helper::Bor => {
            "  function M.bor(a, b)
    return bitpair(a, b, function(x, y) return x == 1 or y == 1 end)
  end
"
        }
        Helper::Bxor => {
            "  function M.bxor(a, b)
    return bitpair(a, b, function(x, y) return x ~= y end)
  end
"
        }
        Helper::Bnot => {
            "  function M.bnot(a)
    return 0xFFFFFFFF - a % 4294967296
  end
"
        }
        Helper::Shl => {
            "  function M.shl(a, n)
    if n <= -32 or n >= 32 then return 0 end
    if n < 0 then return math.floor((a % 4294967296) / 2 ^ -n) end
    return ((a % 4294967296) * 2 ^ n) % 4294967296
  end
"
        }
        Helper::Shr => {
            "  function M.shr(a, n)
    if n <= -32 or n >= 32 then return 0 end
    if n < 0 then return ((a % 4294967296) * 2 ^ -n) % 4294967296 end
    return math.floor((a % 4294967296) / 2 ^ n)
  end
"
        }
        other => common_family(other),
    }
}

/// Pure-Lua signed family reproducing LuaJIT `bit.*` semantics (5.1 target,
/// LuaJIT source): results are normalized through `tobit32`, shift counts
/// masked to 5 bits, exactly as `bit` does.
fn jit_family(helper: Helper) -> &'static str {
    match helper {
        Helper::Band => {
            "  function M.band(a, b)
    return tobit32(bitpair(a, b, function(x, y) return x == 1 and y == 1 end))
  end
"
        }
        Helper::Bor => {
            "  function M.bor(a, b)
    return tobit32(bitpair(a, b, function(x, y) return x == 1 or y == 1 end))
  end
"
        }
        Helper::Bxor => {
            "  function M.bxor(a, b)
    return tobit32(bitpair(a, b, function(x, y) return x ~= y end))
  end
"
        }
        Helper::Bnot => {
            "  function M.bnot(a)
    return tobit32(0xFFFFFFFF - a % 4294967296)
  end
"
        }
        Helper::Tobit => {
            "  function M.tobit(x)
    return tobit32(x)
  end
"
        }
        Helper::Lshift => {
            "  function M.lshift(a, n)
    return tobit32((a % 4294967296) * 2 ^ (n % 32) % 4294967296)
  end
"
        }
        Helper::Rshift => {
            "  function M.rshift(a, n)
    return tobit32(math.floor((a % 4294967296) / 2 ^ (n % 32)))
  end
"
        }
        Helper::Arshift => {
            "  function M.arshift(a, n)
    a = a % 4294967296
    n = n % 32
    local r = math.floor(a / 2 ^ n)
    if a >= 0x80000000 and n > 0 then
      r = r + 4294967296 - 2 ^ (32 - n)
    end
    return tobit32(r)
  end
"
        }
        Helper::Rol => {
            "  function M.rol(a, n)
    a = a % 4294967296
    n = n % 32
    return tobit32((a * 2 ^ n) % 4294967296 + math.floor(a / 2 ^ (32 - n)))
  end
"
        }
        Helper::Ror => {
            "  function M.ror(a, n)
    return M.rol(a, 32 - n % 32)
  end
"
        }
        Helper::Bswap => {
            "  function M.bswap(a)
    a = a % 4294967296
    local b0 = a % 256
    local b1 = math.floor(a / 256) % 256
    local b2 = math.floor(a / 65536) % 256
    local b3 = math.floor(a / 16777216) % 256
    return tobit32(b0 * 16777216 + b1 * 65536 + b2 * 256 + b3)
  end
"
        }
        Helper::Tohex => {
            "  function M.tohex(x, n)
    n = n or 8
    local spec = \"x\"
    if n < 0 then
      n = -n
      spec = \"X\"
    end
    return string.sub(string.format(\"%0\" .. n .. spec, x % 4294967296), -n)
  end
"
        }
        other => common_family(other),
    }
}

/// Helpers whose body is target- and family-independent.
///
/// `close_scope` reproduces Lua 5.4 `<close>` semantics: the scope tail
/// runs under `pcall`; the handle's `__close` metamethod is then invoked
/// (with the error object on the error path, `nil` otherwise, matching
/// 5.4's `__close(v, err)` protocol); the error, if any, is re-raised
/// unmodified (`level 0` keeps the original message intact). `nil`/`false`
/// handles are ignored exactly as 5.4 ignores them.
fn common_family(helper: Helper) -> &'static str {
    match helper {
        Helper::CloseScope => {
            "  function M.close_scope(v, body)
    local ok, err = pcall(body)
    if v then
      local mt = getmetatable(v)
      if mt and mt.__close then
        mt.__close(v, err)
      end
    end
    if not ok then
      error(err, 0)
    end
  end
"
        }
        other => unreachable!("helper {other:?} has a family-specific body"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_set_emits_no_prelude() {
        assert_eq!(
            prelude(&BTreeSet::new(), Dialect::Lua53, Dialect::Lua51),
            None
        );
    }

    #[test]
    fn band_only_on_52_uses_bit32_and_nothing_else() {
        let used = BTreeSet::from([Helper::Band]);
        let text = prelude(&used, Dialect::Lua53, Dialect::Lua52).expect("prelude");
        assert_eq!(
            text,
            "local __luabox_rt = (function()\n  local M = {}\n  M.band = bit32.band\n  return M\nend)()\n\n"
        );
    }

    #[test]
    fn pure_band_pulls_in_the_bitpair_core() {
        let used = BTreeSet::from([Helper::Band]);
        let text = prelude(&used, Dialect::Lua53, Dialect::Lua51).expect("prelude");
        assert!(text.contains("local function bitpair"));
        assert!(text.contains("function M.band"));
        assert!(!text.contains("M.bor"), "tree-shaken: bor not included");
        assert!(!text.contains("tobit32"), "unsigned family needs no tobit");
    }

    #[test]
    fn shifts_need_no_bitpair_core() {
        let used = BTreeSet::from([Helper::Shl, Helper::Shr]);
        let text = prelude(&used, Dialect::Lua53, Dialect::Lua51).expect("prelude");
        assert!(!text.contains("bitpair"));
    }

    #[test]
    fn jit_family_normalizes_signed_and_includes_cores() {
        let used = BTreeSet::from([Helper::Band]);
        let text = prelude(&used, Dialect::LuaJit, Dialect::Lua51).expect("prelude");
        assert!(text.contains("local function tobit32"));
        assert!(text.contains("local function bitpair"));
        assert!(text.contains("return tobit32(bitpair"));
    }

    #[test]
    fn ror_pulls_in_rol() {
        let used = BTreeSet::from([Helper::Ror]);
        let text = prelude(&used, Dialect::LuaJit, Dialect::Lua51).expect("prelude");
        assert!(text.contains("function M.rol"));
        assert!(text.contains("function M.ror"));
    }

    #[test]
    fn close_scope_is_backend_independent() {
        for to in [Dialect::Lua51, Dialect::Lua52, Dialect::Lua53] {
            let used = BTreeSet::from([Helper::CloseScope]);
            let text = prelude(&used, Dialect::Lua54, to).expect("prelude");
            assert!(text.contains("function M.close_scope"));
            assert!(text.contains("mt.__close(v, err)"));
        }
    }
}
