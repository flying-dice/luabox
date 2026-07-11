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

use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;

use luabox_diag::{Code, Diagnostic, Label, Span};
use luabox_hir::{Expr, ExprId, HirId, Resolution, Stmt};
use luabox_syntax::lua::{self, Dialect};
use luabox_syntax::luacats::{self, AliasTag, AnnotatedItem, Tag};

use crate::env::TypeEnv;

/// A definition-package source paired with the file it was read from — the
/// unit cross-package collision reporting (`LB0307`, #108) attributes to.
#[derive(Debug, Clone)]
pub struct DefFile {
    /// A display label for the declaring file (e.g. `defs/geometry.d.lua`, or
    /// `<dep>/defs/geometry.d.lua` for a dependency's def) — the name a
    /// collision diagnostic prints.
    pub file: String,
    /// The `.d.lua` source text.
    pub text: String,
}

/// A parsed, lowered definition-package layer: the ambient environment plus
/// the `---@alias`es it declares (so a consuming file's lowerer can expand
/// them). Passed by reference to [`crate::check_file_shaped`].
#[derive(Debug)]
pub struct Ambient {
    pub(crate) env: TypeEnv,
    pub(crate) aliases: BTreeMap<String, AliasTag>,
    /// Every top-level global name this definition package declares (module
    /// tables like `math`, scalar globals like `_VERSION`, and bare
    /// functions like `print`) — the `undefined-global` lint's read-only
    /// name-enumeration surface (ticket #103). `TypeEnv` only exposes
    /// by-name lookups (`function`/`global_type`), not enumeration, so this
    /// is harvested independently by lowering each source through
    /// `luabox-hir` and collecting every assignment target that resolves to
    /// a bare global (dotted targets like `function math.abs() end` count
    /// their *first* segment, `math`).
    global_names: HashSet<String>,
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
        let mut global_names = HashSet::new();
        for (parse, _) in &files {
            global_names.extend(declared_global_names(parse));
        }
        Ambient {
            env,
            aliases,
            global_names,
        }
    }

    /// Every top-level global name this ambient layer declares — the
    /// `undefined-global` lint's known-globals surface (ticket #103).
    #[must_use]
    pub fn global_names(&self) -> &HashSet<String> {
        &self.global_names
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

/// Every top-level global name a `.d.lua` source declares: the base name of
/// every assignment target that resolves to a global, anywhere in the file
/// (mirroring `luabox-lint`'s `global-write` detection). A dotted target —
/// `function math.abs(x) end` desugars to an assignment to `math.abs` —
/// contributes only its first segment (`math`), the name that must already
/// exist as a global for the declaration to make sense.
fn declared_global_names(parse: &lua::Parse) -> HashSet<String> {
    let lowered = luabox_hir::lower(parse);
    let mut names = HashSet::new();
    for (body_id, body) in lowered.bodies() {
        for (_, stmt) in body.stmts() {
            let Stmt::Assign { targets, .. } = stmt else {
                continue;
            };
            for &target in targets {
                let base = base_expr(body, target);
                let hir = HirId::expr(body_id, base);
                if let Some(Resolution::Global(name)) = lowered.resolution(hir) {
                    names.insert(name.clone());
                }
            }
        }
    }
    names
}

/// Walk an assignment target's `Expr::Index` chain (`a.b.c` desugars to
/// nested `Index` nodes) down to its innermost base expression.
fn base_expr(body: &luabox_hir::Body, mut id: ExprId) -> ExprId {
    while let Expr::Index { base, .. } = body.expr(id) {
        id = *base;
    }
    id
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
            include_str!("../../../assets/defs/lua51/package.d.lua"),
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
            include_str!("../../../assets/defs/lua52/package.d.lua"),
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
            include_str!("../../../assets/defs/lua53/package.d.lua"),
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
            include_str!("../../../assets/defs/lua54/package.d.lua"),
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
            include_str!("../../../assets/defs/luajit/package.d.lua"),
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

/// Build the ambient layer combining the dialect stdlib with attributed
/// project-local and dependency definition files (`[types] defs`), and report
/// cross-package `---@class` name collisions (`LB0307`, #108).
///
/// This is the manifest-native form of luals's `workspace.library` model: a
/// dependency's own `[types] defs` files join the consumer's ambient scope, so
/// their `---@class` declarations become referenceable and checkable across
/// the package boundary. Class names are a single global namespace (as in
/// luals), but where luals silently merges duplicate declarations, luabox
/// emits a warning at every declaration after the first.
///
/// **Order is precedence.** Callers pass `defs` in winner-first order —
/// project-local defs first (the consumer wins), then each direct dependency
/// alphabetically. The first source to declare a class name wins: its fields
/// are the ones [`TypeEnv`] resolves, and every later declaration of that name
/// yields an `LB0307` warning naming the file that already declared it.
#[must_use]
pub fn combined_checked(dialect: Dialect, defs: &[DefFile]) -> (Ambient, Vec<Diagnostic>) {
    let mut all: Vec<&str> = sources(dialect).to_vec();
    for def in defs {
        all.push(def.text.as_str());
    }
    let ambient = Ambient::build(&all);
    (ambient, class_collisions(defs))
}

/// Detect `---@class` names declared by more than one `.d.lua` in `defs`,
/// producing an `LB0307` warning at each declaration after the first. The
/// first declarer (earliest in `defs`, which is winner-first order) wins and
/// is never reported; each later duplicate points at the file that already
/// owns the name. Only cross-*file* duplicates are reported — a file that
/// declares a class once (the norm) and the trusted stdlib layer are never
/// involved.
fn class_collisions(defs: &[DefFile]) -> Vec<Diagnostic> {
    let mut owner: BTreeMap<String, String> = BTreeMap::new();
    let mut diags = Vec::new();
    for def in defs {
        let parse = lua::parse(&def.text, Dialect::Lua54);
        let items = luacats::harvest(&parse);
        // A class name may only be *claimed* once per file even if the same
        // file repeats it; dedup within-file so an intra-file repeat does not
        // masquerade as a cross-package collision.
        let mut claimed_here: HashSet<String> = HashSet::new();
        for item in &items {
            for tag in &item.block.tags {
                let Tag::Class(class) = tag else { continue };
                if class.name.is_empty() || !claimed_here.insert(class.name.clone()) {
                    continue;
                }
                if let Some(first) = owner.get(&class.name) {
                    diags.push(
                        Diagnostic::warning(
                            Code::new(307),
                            format!(
                                "class `{}` is declared by more than one definition package",
                                class.name
                            ),
                        )
                        .with_label(Label::primary(
                            Span::new(def.file.clone(), class.span.start..class.span.end),
                            "duplicate declaration here",
                        ))
                        .with_note(format!(
                            "first declared in `{first}`; that declaration wins"
                        )),
                    );
                } else {
                    owner.insert(class.name.clone(), def.file.clone());
                }
            }
        }
    }
    diags
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
    fn global_names_enumerates_top_level_declarations() {
        // Module tables and scalar globals, keyed by their own top-level
        // name — not the dotted stdlib functions they carry.
        let names = stdlib(Dialect::Lua54).global_names();
        for expected in [
            "print", "assert", "pairs", "math", "string", "_G", "_VERSION",
        ] {
            assert!(names.contains(expected), "missing `{expected}`: {names:?}");
        }
        // `math.abs` is a stdlib function, not itself a top-level global.
        assert!(!names.contains("math.abs"));
    }

    #[test]
    fn global_names_include_project_defs() {
        let extra = vec!["---@meta\nlove = {}\nfunction love.load() end\n".to_string()];
        let ambient = combined(Dialect::Lua54, &extra);
        assert!(ambient.global_names().contains("love"));
        // The stdlib set is still layered in underneath.
        assert!(ambient.global_names().contains("print"));
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

    // --- cross-package class collisions (#108) ---------------------------

    #[test]
    fn combined_checked_reports_collision_and_first_wins() {
        let defs = vec![
            DefFile {
                file: "defs/a.d.lua".to_string(),
                text: "---@meta\n---@class Widget\n---@field a number\n".to_string(),
            },
            DefFile {
                file: "dep/defs/b.d.lua".to_string(),
                text: "---@meta\n---@class Widget\n---@field b number\n".to_string(),
            },
        ];
        let (ambient, diags) = combined_checked(Dialect::Lua54, &defs);
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].code.to_string(), "LB0307");
        assert_eq!(diags[0].severity, luabox_diag::Severity::Warning);
        // Loser at the span, winner named in the note.
        let label = diags[0].primary_label().expect("primary label");
        assert_eq!(label.span.file, "dep/defs/b.d.lua");
        assert!(
            diags[0].notes.iter().any(|n| n.contains("defs/a.d.lua")),
            "note names the winner: {:?}",
            diags[0].notes
        );
        // Deterministic winner: the first (project-local) `Widget` — field `a`.
        let parse = lua::parse("---@type Widget\nlocal w = { a = 1 }\n", Dialect::Lua54);
        let clean = crate::check_file_shaped(
            &parse,
            "t.lua",
            crate::Strictness::Strict,
            None,
            Some(&ambient),
        );
        assert!(
            clean.is_empty(),
            "project decl (field `a`) must win: {clean:?}"
        );
        let parse = lua::parse("---@type Widget\nlocal w = { b = 1 }\n", Dialect::Lua54);
        let loser = crate::check_file_shaped(
            &parse,
            "t.lua",
            crate::Strictness::Strict,
            None,
            Some(&ambient),
        );
        assert!(
            !loser.is_empty(),
            "dependency decl (field `b`) must have lost"
        );
    }

    #[test]
    fn combined_checked_distinct_classes_no_collision() {
        let defs = vec![
            DefFile {
                file: "a.d.lua".to_string(),
                text: "---@meta\n---@class Alpha\n---@field a number\n".to_string(),
            },
            DefFile {
                file: "b.d.lua".to_string(),
                text: "---@meta\n---@class Beta\n---@field b number\n".to_string(),
            },
        ];
        let (_ambient, diags) = combined_checked(Dialect::Lua54, &defs);
        assert!(diags.is_empty(), "{diags:?}");
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
