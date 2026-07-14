//! `---@version` predicate parsing and edition matching.
//!
//! luals rides the `deprecated` diagnostic for `---@version`: a symbol whose
//! valid-version set excludes the configured `Lua.runtime.version` is marked
//! deprecated at its use sites, with a version-specific message
//! (`script/vm/doc.lua` `getDeprecated` + `script/core/diagnostics/
//! deprecated.lua`, which swaps the message to `DIAG_DEFINED_VERSION` for a
//! `doc.version` source). luabox mirrors this: [`VersionReq`] is parsed out of
//! the raw `---@version` body and, when it excludes the project `edition`, the
//! carrier's use sites report `LB0308` (luals `deprecated`) — so
//! `---@diagnostic disable: deprecated` suppresses it exactly as in luals.
//!
//! Grammar (`script/parser/luadoc.lua` `doc.version`): comma-separated
//! entries, each an optional `>` (ge) or `<` (le) then a version name —
//! `5.1`..`5.4` or `JIT`. Expansion (`vm.getValidVersions`): a plain name is
//! itself; `>N`/`<N` span the ordered numeric set `5.1 < 5.2 < 5.3 < 5.4`;
//! `JIT` is LuaJIT; and if Lua 5.1 is valid, LuaJIT is auto-added (the luals
//! 5.1/LuaJIT compat rule).

use luabox_syntax::lua::Dialect;

/// A parsed `---@version` predicate: the set of Lua editions the annotated
/// symbol is valid for (luals `vm.getValidVersions`).
///
/// [`VersionReq::parse`] returns `None` when the body names no version luabox
/// recognises (empty, or only unsupported tokens such as `5.5`), so an
/// unrecognised annotation gates nothing rather than gating everything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionReq {
    /// The valid editions, canonically ordered (`Dialect`'s `Ord`:
    /// `5.1 < 5.2 < 5.3 < 5.4 < luajit`). Always non-empty for a `Some`.
    valid: Vec<Dialect>,
}

/// The comparison a single `---@version` entry carries.
#[derive(Clone, Copy)]
enum Op {
    /// A plain version name — exactly that version.
    Eq,
    /// `>N` — that version and every higher numeric version.
    Ge,
    /// `<N` — that version and every lower numeric version.
    Le,
}

/// The four numeric editions in ascending order, used to expand `>`/`<`.
const NUMERIC: [Dialect; 4] = [
    Dialect::Lua51,
    Dialect::Lua52,
    Dialect::Lua53,
    Dialect::Lua54,
];

/// The ascending rank of a numeric edition (`None` for LuaJIT, which is not
/// part of the `>`/`<` ordering).
fn numeric_rank(d: Dialect) -> Option<u8> {
    match d {
        Dialect::Lua51 => Some(1),
        Dialect::Lua52 => Some(2),
        Dialect::Lua53 => Some(3),
        Dialect::Lua54 => Some(4),
        Dialect::LuaJit => None,
    }
}

/// Resolve one `---@version` version name to an edition, or `None` if luabox
/// does not model it. `JIT` is accepted case-insensitively (luals writes it
/// uppercase; being lenient here never gates a symbol it should not).
fn dialect_of_name(name: &str) -> Option<Dialect> {
    Some(match name {
        "5.1" => Dialect::Lua51,
        "5.2" => Dialect::Lua52,
        "5.3" => Dialect::Lua53,
        "5.4" => Dialect::Lua54,
        _ if name.eq_ignore_ascii_case("jit") => Dialect::LuaJit,
        _ => return None,
    })
}

fn push_unique(valid: &mut Vec<Dialect>, d: Dialect) {
    if !valid.contains(&d) {
        valid.push(d);
    }
}

impl VersionReq {
    /// Parse a raw `---@version` body (the trimmed text after the tag name).
    ///
    /// Returns `None` when nothing recognisable is named, so the caller gates
    /// nothing — matching luals, where an empty valid set never marks a symbol
    /// deprecated.
    #[must_use]
    pub fn parse(body: &str) -> Option<VersionReq> {
        let mut valid: Vec<Dialect> = Vec::new();
        let mut saw_entry = false;
        for raw in body.split(',') {
            let entry = raw.trim();
            if entry.is_empty() {
                continue;
            }
            let (op, name) = if let Some(rest) = entry.strip_prefix('>') {
                (Op::Ge, rest.trim())
            } else if let Some(rest) = entry.strip_prefix('<') {
                (Op::Le, rest.trim())
            } else {
                (Op::Eq, entry)
            };
            let Some(base) = dialect_of_name(name) else {
                continue;
            };
            saw_entry = true;
            match (op, numeric_rank(base)) {
                (Op::Eq, _) | (_, None) => push_unique(&mut valid, base),
                (Op::Ge, Some(threshold)) => {
                    for d in NUMERIC {
                        if numeric_rank(d).is_some_and(|r| r >= threshold) {
                            push_unique(&mut valid, d);
                        }
                    }
                }
                (Op::Le, Some(threshold)) => {
                    for d in NUMERIC {
                        if numeric_rank(d).is_some_and(|r| r <= threshold) {
                            push_unique(&mut valid, d);
                        }
                    }
                }
            }
        }
        if !saw_entry {
            return None;
        }
        // luals compat rule: valid-for-5.1 implies valid-for-LuaJIT.
        if valid.contains(&Dialect::Lua51) {
            push_unique(&mut valid, Dialect::LuaJit);
        }
        valid.sort_unstable();
        Some(VersionReq { valid })
    }

    /// Whether `edition` is among the valid versions (no diagnostic fires).
    #[must_use]
    pub fn includes(&self, edition: Dialect) -> bool {
        self.valid.contains(&edition)
    }

    /// The valid editions, canonically ordered.
    #[must_use]
    pub fn valid_editions(&self) -> &[Dialect] {
        &self.valid
    }

    /// The valid editions as a `/`-joined list of manifest ids
    /// (`5.2/5.3/luajit`) — the `defined in <versions>` half of the message,
    /// mirroring luals `DIAG_DEFINED_VERSION`'s slash-joined version list.
    #[must_use]
    pub fn display_list(&self) -> String {
        self.valid
            .iter()
            .map(|d| d.manifest_id())
            .collect::<Vec<_>>()
            .join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editions(req: &VersionReq) -> Vec<&'static str> {
        req.valid_editions()
            .iter()
            .map(|d| d.manifest_id())
            .collect()
    }

    #[test]
    fn plain_numeric_is_itself() {
        let req = VersionReq::parse("5.2").unwrap();
        assert!(req.includes(Dialect::Lua52));
        assert!(!req.includes(Dialect::Lua54));
        assert!(!req.includes(Dialect::LuaJit));
        assert_eq!(req.display_list(), "5.2");
    }

    #[test]
    fn ge_spans_upward() {
        let req = VersionReq::parse(">5.2").unwrap();
        assert_eq!(editions(&req), ["5.2", "5.3", "5.4"]);
        assert!(!req.includes(Dialect::Lua51));
        // 5.1 is not valid, so the LuaJIT compat rule does not apply.
        assert!(!req.includes(Dialect::LuaJit));
    }

    #[test]
    fn le_spans_downward_and_pulls_in_luajit_via_compat() {
        let req = VersionReq::parse("<5.2").unwrap();
        // 5.1 is valid ⇒ LuaJIT auto-added (luals compat rule).
        assert_eq!(editions(&req), ["5.1", "5.2", "luajit"]);
    }

    #[test]
    fn jit_is_luajit_only() {
        let req = VersionReq::parse("JIT").unwrap();
        assert_eq!(editions(&req), ["luajit"]);
        assert!(!req.includes(Dialect::Lua51));
    }

    #[test]
    fn lua51_implies_luajit() {
        let req = VersionReq::parse("5.1").unwrap();
        assert!(req.includes(Dialect::Lua51));
        assert!(req.includes(Dialect::LuaJit));
    }

    #[test]
    fn comma_list_unions_entries() {
        let req = VersionReq::parse("5.2, 5.4").unwrap();
        assert_eq!(editions(&req), ["5.2", "5.4"]);
        assert!(!req.includes(Dialect::Lua53));
    }

    #[test]
    fn mixed_range_and_jit() {
        let req = VersionReq::parse(">5.3, JIT").unwrap();
        assert_eq!(editions(&req), ["5.3", "5.4", "luajit"]);
    }

    #[test]
    fn jit_case_insensitive() {
        assert!(VersionReq::parse("jit").unwrap().includes(Dialect::LuaJit));
    }

    #[test]
    fn empty_or_unknown_is_none() {
        assert!(VersionReq::parse("").is_none());
        assert!(VersionReq::parse("   ").is_none());
        // 5.5 is not an edition luabox models.
        assert!(VersionReq::parse("5.5").is_none());
    }
}
