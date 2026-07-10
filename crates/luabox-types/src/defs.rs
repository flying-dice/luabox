//! Definition packages — ambient `---@meta` `.d.lua` type surfaces
//! (SPEC.md §3).
//!
//! Each supported dialect ships a set of `.d.lua` files (under
//! `assets/defs/<dialect>/`) describing its real stdlib: basic globals plus
//! the `string`, `table`, `math`, `io`, `os`, `coroutine`, `debug` modules
//! and version-specific ones (`utf8` on 5.3+, `bit32` on 5.2, `bit`/`jit`
//! on LuaJIT). They are embedded into the binary with [`include_str!`],
//! parsed and lowered **once per dialect** (cached in a [`OnceLock`]), and
//! merged *beneath* per-file annotations by [`crate::TypeEnv`].
//!
//! Project-local packages named in `[types] defs` (SPEC.md §5) layer on top
//! of the stdlib set: the frontend resolves them to sources and calls
//! [`combined`]. Registry-distributed defs are P2+.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use luabox_syntax::lua::{self, Dialect};
use luabox_syntax::luacats::{self, AliasTag, AnnotatedItem};

use crate::env::TypeEnv;

/// A parsed, lowered definition-package layer: the ambient environment plus
/// the `---@alias`es it declares (so a consuming file's lowerer can expand
/// them). Passed by reference to [`crate::check_file_shaped`].
#[derive(Debug)]
pub struct Ambient {
    pub(crate) env: TypeEnv,
    pub(crate) aliases: BTreeMap<String, AliasTag>,
}

impl Ambient {
    /// Build an ambient layer from a set of `.d.lua` source strings.
    #[must_use]
    pub(crate) fn build(sources: &[&str]) -> Ambient {
        let files: Vec<(lua::Parse, Vec<AnnotatedItem>)> = sources
            .iter()
            .map(|src| {
                // Definition files use only the common syntax; parse them
                // with the richest dialect so nothing is rejected.
                let parse = lua::parse(src, Dialect::Lua54);
                let items = luacats::harvest(&parse);
                (parse, items)
            })
            .collect();
        let (env, aliases) = TypeEnv::build_ambient(&files);
        Ambient { env, aliases }
    }

    /// Undeclared type names referenced by the definition files themselves —
    /// a self-consistency check for the shipped packages (should be empty).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn unknown_names(&self) -> &[(String, luacats::Span)] {
        &self.env.unknown_names
    }

    /// Whether the ambient layer declares a callable of this (possibly
    /// dotted) name — the test/introspection surface.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn has_function(&self, name: &str) -> bool {
        self.env.function(name).is_some()
    }

    /// Whether the ambient layer binds this global value (module table or
    /// scalar).
    #[cfg(test)]
    #[must_use]
    pub(crate) fn has_global(&self, name: &str) -> bool {
        self.env.global_type(name).is_some()
    }
}

/// The embedded `.d.lua` sources for one dialect, in a stable order.
fn sources(dialect: Dialect) -> &'static [&'static str] {
    match dialect {
        Dialect::Lua51 => &[
            include_str!("../../../assets/defs/lua51/basic.d.lua"),
            include_str!("../../../assets/defs/lua51/string.d.lua"),
            include_str!("../../../assets/defs/lua51/table.d.lua"),
            include_str!("../../../assets/defs/lua51/math.d.lua"),
            include_str!("../../../assets/defs/lua51/io.d.lua"),
            include_str!("../../../assets/defs/lua51/os.d.lua"),
            include_str!("../../../assets/defs/lua51/coroutine.d.lua"),
            include_str!("../../../assets/defs/lua51/debug.d.lua"),
        ],
        Dialect::Lua52 => &[
            include_str!("../../../assets/defs/lua52/basic.d.lua"),
            include_str!("../../../assets/defs/lua52/string.d.lua"),
            include_str!("../../../assets/defs/lua52/table.d.lua"),
            include_str!("../../../assets/defs/lua52/math.d.lua"),
            include_str!("../../../assets/defs/lua52/io.d.lua"),
            include_str!("../../../assets/defs/lua52/os.d.lua"),
            include_str!("../../../assets/defs/lua52/coroutine.d.lua"),
            include_str!("../../../assets/defs/lua52/debug.d.lua"),
            include_str!("../../../assets/defs/lua52/bit32.d.lua"),
        ],
        Dialect::Lua53 => &[
            include_str!("../../../assets/defs/lua53/basic.d.lua"),
            include_str!("../../../assets/defs/lua53/string.d.lua"),
            include_str!("../../../assets/defs/lua53/table.d.lua"),
            include_str!("../../../assets/defs/lua53/math.d.lua"),
            include_str!("../../../assets/defs/lua53/io.d.lua"),
            include_str!("../../../assets/defs/lua53/os.d.lua"),
            include_str!("../../../assets/defs/lua53/coroutine.d.lua"),
            include_str!("../../../assets/defs/lua53/debug.d.lua"),
            include_str!("../../../assets/defs/lua53/utf8.d.lua"),
        ],
        Dialect::Lua54 => &[
            include_str!("../../../assets/defs/lua54/basic.d.lua"),
            include_str!("../../../assets/defs/lua54/string.d.lua"),
            include_str!("../../../assets/defs/lua54/table.d.lua"),
            include_str!("../../../assets/defs/lua54/math.d.lua"),
            include_str!("../../../assets/defs/lua54/io.d.lua"),
            include_str!("../../../assets/defs/lua54/os.d.lua"),
            include_str!("../../../assets/defs/lua54/coroutine.d.lua"),
            include_str!("../../../assets/defs/lua54/debug.d.lua"),
            include_str!("../../../assets/defs/lua54/utf8.d.lua"),
        ],
        Dialect::LuaJit => &[
            include_str!("../../../assets/defs/luajit/basic.d.lua"),
            include_str!("../../../assets/defs/luajit/string.d.lua"),
            include_str!("../../../assets/defs/luajit/table.d.lua"),
            include_str!("../../../assets/defs/luajit/math.d.lua"),
            include_str!("../../../assets/defs/luajit/io.d.lua"),
            include_str!("../../../assets/defs/luajit/os.d.lua"),
            include_str!("../../../assets/defs/luajit/coroutine.d.lua"),
            include_str!("../../../assets/defs/luajit/debug.d.lua"),
            include_str!("../../../assets/defs/luajit/bit.d.lua"),
            include_str!("../../../assets/defs/luajit/jit.d.lua"),
        ],
    }
}

/// The stdlib ambient layer for a dialect, built once and cached for the
/// process lifetime (definition files never change at runtime — the perf
/// gate depends on this being paid only once).
#[must_use]
pub fn stdlib(dialect: Dialect) -> &'static Ambient {
    // One slot per `Dialect` variant, in `Dialect::ALL` order.
    static CACHE: [OnceLock<Ambient>; Dialect::ALL.len()] =
        [const { OnceLock::new() }; Dialect::ALL.len()];
    let index = match dialect {
        Dialect::Lua51 => 0,
        Dialect::Lua52 => 1,
        Dialect::Lua53 => 2,
        Dialect::Lua54 => 3,
        Dialect::LuaJit => 4,
    };
    CACHE[index].get_or_init(|| Ambient::build(sources(dialect)))
}

/// Build an ambient layer combining the dialect stdlib with extra
/// project-local definition sources (`[types] defs`). Not cached — the extra
/// sources vary per project; the stdlib portion is small and shared by
/// value here.
#[must_use]
pub fn combined(dialect: Dialect, extra: &[String]) -> Ambient {
    if extra.is_empty() {
        // Common case: reuse the cached stdlib set by cloning nothing —
        // callers hold `&Ambient`, so hand back a fresh build only when
        // extra defs exist. Here we still must return owned; clone-free
        // path is `stdlib` (used directly by the frontend).
        return Ambient::build(sources(dialect));
    }
    let mut all: Vec<&str> = sources(dialect).to_vec();
    for src in extra {
        all.push(src.as_str());
    }
    Ambient::build(&all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_dialect_builds_without_unknown_type_names() {
        for dialect in Dialect::ALL {
            let ambient = stdlib(dialect);
            assert!(
                ambient.unknown_names().is_empty(),
                "{dialect:?} defs reference undeclared type names: {:?}",
                ambient.unknown_names()
            );
        }
    }

    #[test]
    fn basic_globals_present_everywhere() {
        for dialect in Dialect::ALL {
            let a = stdlib(dialect);
            for name in [
                "print",
                "type",
                "pairs",
                "ipairs",
                "tostring",
                "tonumber",
                "pcall",
                "assert",
                "setmetatable",
                "require",
                "string.format",
                "string.rep",
                "table.insert",
                "math.floor",
                "os.time",
            ] {
                assert!(a.has_function(name), "{dialect:?} missing `{name}`");
            }
            assert!(a.has_global("_G"), "{dialect:?} missing `_G`");
            assert!(a.has_global("math"), "{dialect:?} missing `math` table");
        }
    }

    #[test]
    fn version_gated_availability() {
        // bit32 exists in 5.2 only.
        assert!(stdlib(Dialect::Lua52).has_function("bit32.band"));
        assert!(!stdlib(Dialect::Lua51).has_function("bit32.band"));
        assert!(!stdlib(Dialect::Lua54).has_function("bit32.band"));

        // utf8 is 5.3+.
        assert!(!stdlib(Dialect::Lua51).has_function("utf8.char"));
        assert!(!stdlib(Dialect::Lua52).has_function("utf8.char"));
        assert!(stdlib(Dialect::Lua53).has_function("utf8.char"));
        assert!(stdlib(Dialect::Lua54).has_function("utf8.char"));

        // table.pack/unpack are 5.2+; 5.1 has the `unpack` global instead.
        assert!(!stdlib(Dialect::Lua51).has_function("table.pack"));
        assert!(stdlib(Dialect::Lua51).has_function("unpack"));
        assert!(stdlib(Dialect::Lua52).has_function("table.pack"));

        // string.pack is 5.3+.
        assert!(!stdlib(Dialect::Lua52).has_function("string.pack"));
        assert!(stdlib(Dialect::Lua53).has_function("string.pack"));

        // table.move is 5.3+.
        assert!(!stdlib(Dialect::Lua52).has_function("table.move"));
        assert!(stdlib(Dialect::Lua54).has_function("table.move"));

        // jit/bit only on LuaJIT.
        assert!(stdlib(Dialect::LuaJit).has_function("bit.band"));
        assert!(stdlib(Dialect::LuaJit).has_global("jit"));
        assert!(!stdlib(Dialect::Lua54).has_function("bit.band"));

        // warn is 5.4 only.
        assert!(stdlib(Dialect::Lua54).has_function("warn"));
        assert!(!stdlib(Dialect::Lua53).has_function("warn"));

        // math.type is 5.3+.
        assert!(!stdlib(Dialect::Lua51).has_function("math.type"));
        assert!(stdlib(Dialect::Lua53).has_function("math.type"));
    }

    // --- ambient globals flowing into check + inference ------------------

    fn codes(dialect: Dialect, src: &str, strictness: crate::Strictness) -> Vec<String> {
        let ambient = stdlib(dialect);
        let parse = lua::parse(src, dialect);
        assert_eq!(parse.errors(), &[], "fixture must parse cleanly");
        crate::check_file_shaped(&parse, "test.lua", strictness, None, Some(ambient))
            .iter()
            .map(|d| d.code.to_string())
            .collect()
    }

    fn strict(dialect: Dialect, src: &str) -> Vec<String> {
        codes(dialect, src, crate::Strictness::Strict)
    }

    #[test]
    fn stdlib_call_argument_is_typechecked() {
        // `string.rep(s, n)` wants an integer count.
        assert_eq!(
            strict(Dialect::Lua54, "string.rep(\"x\", \"y\")\n"),
            vec!["LB0300"]
        );
        assert_eq!(
            strict(Dialect::Lua54, "string.rep(\"x\", 3)\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn print_accepts_anything() {
        assert_eq!(
            strict(Dialect::Lua54, "print(1, \"two\", true, nil, {})\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn stdlib_result_type_flows_into_calls() {
        // `string.rep` returns `string`, so it satisfies a string parameter
        // and violates a number one.
        let ok = "\
---@param s string
local function f(s) end
f(string.rep(\"x\", 3))
";
        assert_eq!(strict(Dialect::Lua54, ok), Vec::<String>::new());
        let bad = "\
---@param n number
local function f(n) end
f(string.rep(\"x\", 3))
";
        assert_eq!(strict(Dialect::Lua54, bad), vec!["LB0300"]);
    }

    #[test]
    fn tonumber_overloads() {
        // 1-arg primary form.
        assert_eq!(
            strict(Dialect::Lua54, "local n = tonumber(\"3\")\n"),
            Vec::<String>::new()
        );
        // 2-arg (base) overload form.
        assert_eq!(
            strict(Dialect::Lua54, "local n = tonumber(\"ff\", 16)\n"),
            Vec::<String>::new()
        );
        // Neither form takes zero arguments.
        assert_eq!(strict(Dialect::Lua54, "tonumber()\n"), vec!["LB0301"]);
        // The base must be an integer — no form accepts a string base.
        assert_eq!(
            strict(Dialect::Lua54, "tonumber(\"ff\", \"x\")\n"),
            vec!["LB0300"]
        );
    }

    #[test]
    fn table_insert_two_and_three_arg_forms() {
        let src = "\
local t = {}
table.insert(t, 1)
table.insert(t, 2, 3)
";
        assert_eq!(strict(Dialect::Lua54, src), Vec::<String>::new());
    }

    #[test]
    fn local_shadows_ambient_global() {
        // Ambient `tostring` takes exactly one argument, so a 2-arg call
        // errors — unless a local of the same name shadows it.
        assert_eq!(strict(Dialect::Lua54, "tostring(1, 2)\n"), vec!["LB0301"]);
        let shadowed = "\
local function tostring(...) end
tostring(1, 2)
";
        assert_eq!(strict(Dialect::Lua54, shadowed), Vec::<String>::new());
    }

    #[test]
    fn module_constant_field_reads_typed() {
        // `math.pi` is a number; passing it to a string parameter errors.
        let src = "\
---@param s string
local function f(s) end
f(math.pi)
";
        assert_eq!(strict(Dialect::Lua54, src), vec!["LB0300"]);
    }

    #[test]
    fn version_gated_stdlib_call() {
        // string.pack is 5.3+: a wrong first argument errors in 5.4 (the
        // format must be a string)...
        assert_eq!(
            strict(Dialect::Lua54, "string.pack(123, 1)\n"),
            vec!["LB0300"]
        );
        // ...while in 5.1 `string.pack` is undeclared: an unknown receiver,
        // never checked, no crash, no diagnostic.
        assert_eq!(
            strict(Dialect::Lua51, "string.pack(123, 1)\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn bit32_present_only_in_52() {
        // In 5.2 bit32 is a real module; the call typechecks clean.
        assert_eq!(
            strict(Dialect::Lua52, "local x = bit32.band(1, 2)\n"),
            Vec::<String>::new()
        );
        // In 5.4 bit32 is absent: unknown receiver, no diagnostic (an absent
        // global read is not itself an error).
        assert_eq!(
            strict(Dialect::Lua54, "local x = bit32.band(1, 2)\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn project_defs_layer_over_stdlib() {
        // A project-local package adds a global alongside the stdlib set.
        let extra = vec![
            "---@meta\n---@param name string\n---@return boolean\nfunction love_setup(name) end\n"
                .to_string(),
        ];
        let ambient = combined(Dialect::Lua54, &extra);
        let parse = lua::parse("love_setup(1)\nprint(\"still stdlib\")\n", Dialect::Lua54);
        let diags = crate::check_file_shaped(
            &parse,
            "test.lua",
            crate::Strictness::Strict,
            None,
            Some(&ambient),
        );
        let codes: Vec<String> = diags.iter().map(|d| d.code.to_string()).collect();
        // `love_setup` wants a string; `print` (stdlib) still resolves.
        assert_eq!(codes, vec!["LB0300"]);
    }
}
