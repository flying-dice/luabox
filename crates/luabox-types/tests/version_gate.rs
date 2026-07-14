//! `---@version` edition gating — luals parity.
//!
//! luals has no dedicated version diagnostic: `---@version` rides the
//! `deprecated` diagnostic (`script/vm/doc.lua` `getDeprecated` +
//! `script/core/diagnostics/deprecated.lua`, which swaps the message to
//! `DIAG_DEFINED_VERSION` — "Defined in {}, current is {}." — when the
//! deprecation source is a `doc.version`). A symbol whose valid-version set
//! (`vm.getValidVersions`) excludes the configured `Lua.runtime.version` is
//! flagged at its use sites, Warning severity, suppressible with
//! `---@diagnostic disable: deprecated`. luabox mirrors this on `LB0308`, with
//! the project `edition` (manifest `edition`) as the configured version.

use luabox_diag::{Diagnostic, Severity};
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::{Strictness, build_ambient, check_file, check_file_with_ambient};

/// Warn-mode diagnostics for a self-contained file under `edition`.
fn diags(src: &str, edition: Dialect) -> Vec<Diagnostic> {
    let parsed = parse(src, edition);
    assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
    check_file(&parsed, "test.lua", Strictness::Warn, edition)
}

fn codes(src: &str, edition: Dialect) -> Vec<String> {
    diags(src, edition)
        .iter()
        .map(|d| d.code.to_string())
        .collect()
}

/// Warn-mode codes for a file checked against the `edition` stdlib plus the
/// given extra `---@meta` def sources.
fn ambient_codes(src: &str, defs: &[&str], edition: Dialect) -> Vec<String> {
    let parsed = parse(src, edition);
    assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
    let defs_owned: Vec<String> = defs.iter().map(|s| (*s).to_string()).collect();
    let ambient = build_ambient(edition, &defs_owned);
    check_file_with_ambient(
        &parsed,
        "test.lua",
        Strictness::Warn,
        edition,
        Some(&ambient),
    )
    .iter()
    .map(|d| d.code.to_string())
    .collect()
}

#[test]
fn use_under_excluded_edition_flagged_with_defined_in_message() {
    let src = "\
---@version 5.2
local function legacy() end
legacy()
";
    let ds = diags(src, Dialect::Lua54);
    assert_eq!(ds.len(), 1, "one finding: {ds:?}");
    assert_eq!(ds[0].code.to_string(), "LB0308");
    assert_eq!(ds[0].severity, Severity::Warning);
    assert!(
        ds[0].message.contains("defined in `5.2`"),
        "message names the valid versions: {:?}",
        ds[0].message
    );
    assert!(
        ds[0].message.contains("current is `5.4`"),
        "message names the current edition: {:?}",
        ds[0].message
    );
}

#[test]
fn matching_edition_is_clean() {
    let src = "\
---@version 5.2
local function legacy() end
legacy()
";
    assert_eq!(codes(src, Dialect::Lua52), Vec::<String>::new());
}

#[test]
fn declaration_site_never_flagged() {
    // Only uses are flagged, never the annotated declaration itself.
    let src = "\
---@version 5.2
local function legacy() end
";
    assert_eq!(codes(src, Dialect::Lua54), Vec::<String>::new());
}

#[test]
fn ge_range_excludes_lower_edition() {
    let src = "\
---@version >5.2
local function modern() end
modern()
";
    // 5.1 is below the `>5.2` set {5.2,5.3,5.4}.
    let ds = diags(src, Dialect::Lua51);
    assert_eq!(ds.len(), 1, "{ds:?}");
    assert_eq!(ds[0].code.to_string(), "LB0308");
    assert!(
        ds[0].message.contains("defined in `5.2/5.3/5.4`"),
        "{:?}",
        ds[0].message
    );
    // 5.3 is inside the set — clean.
    assert_eq!(codes(src, Dialect::Lua53), Vec::<String>::new());
}

#[test]
fn jit_only_excludes_numeric_editions() {
    let src = "\
---@version JIT
local function jitonly() end
jitonly()
";
    assert_eq!(codes(src, Dialect::Lua54), vec!["LB0308"]);
    assert_eq!(codes(src, Dialect::LuaJit), Vec::<String>::new());
}

#[test]
fn comma_list_unions_valid_versions() {
    let src = "\
---@version 5.2, 5.4
local function twover() end
twover()
";
    // 5.3 is not in {5.2, 5.4}.
    assert_eq!(codes(src, Dialect::Lua53), vec!["LB0308"]);
    // Both listed versions are clean.
    assert_eq!(codes(src, Dialect::Lua52), Vec::<String>::new());
    assert_eq!(codes(src, Dialect::Lua54), Vec::<String>::new());
}

#[test]
fn lua51_compat_rule_admits_luajit() {
    // luals: valid-for-5.1 implies valid-for-LuaJIT.
    let src = "\
---@version 5.1
local function classic() end
classic()
";
    assert_eq!(codes(src, Dialect::LuaJit), Vec::<String>::new());
    // But a plain 5.4 is still excluded.
    assert_eq!(codes(src, Dialect::Lua54), vec!["LB0308"]);
}

#[test]
fn suppressed_by_deprecated_directive() {
    // `---@version` rides `deprecated`, so its rule name suppresses it.
    let src = "\
---@diagnostic disable: deprecated
---@version 5.2
local function legacy() end
legacy()
legacy()
";
    assert_eq!(codes(src, Dialect::Lua54), Vec::<String>::new());
}

#[test]
fn suppressed_by_next_line_directive() {
    let src = "\
---@version 5.2
local function legacy() end
---@diagnostic disable-next-line: deprecated
legacy()
";
    assert_eq!(codes(src, Dialect::Lua54), Vec::<String>::new());
}

#[test]
fn dotted_member_use_flagged() {
    let src = "\
local M = {}
---@version 5.2
function M.legacy() end
M.legacy()
";
    assert_eq!(codes(src, Dialect::Lua54), vec!["LB0308"]);
}

#[test]
fn cross_file_via_defs_flagged() {
    // A `---@version`-restricted global from an ambient `---@meta` def package
    // (mirrors a real gated library member such as `bit32.*` under 5.4): the
    // predicate rides the ambient signature and flags the consumer's use site.
    let defs = "\
---@meta
---@version 5.2
function oldGlobal() end
";
    assert_eq!(
        ambient_codes("oldGlobal()\n", &[defs], Dialect::Lua54),
        vec!["LB0308"]
    );
    // Under the matching edition the same def is clean.
    assert_eq!(
        ambient_codes("oldGlobal()\n", &[defs], Dialect::Lua52),
        Vec::<String>::new()
    );
}
