//! The `.lua.map` sourcemap: a line-based JSON side file mapping every
//! bundle line back to the module file (and line) it came from — plus the
//! `luabox unmap` traceback rewriter (SPEC.md §7).
//!
//! # Format (version 1)
//!
//! ```json
//! {
//!   "version": 1,
//!   "bundle": "app.lua",
//!   "files": ["src/main.lua", "src/util.lua"],
//!   "lines": [null, [1, 1], [1, 2], [0, 1]]
//! }
//! ```
//!
//! - `files` — module files, project-root-relative, forward slashes.
//! - `lines[i]` — the mapping for bundle line `i + 1` (1-based lines):
//!   `[file_index, original_line]`, or `null` for bundler-generated lines
//!   (banner, module map, require shim, hoisted `__luabox_rt` prelude).
//! - `original_line` is the line within the module text as it entered the
//!   bundle: exact for unminified bundles whose `edition == target`; when
//!   lowering restructures statements the line may drift by the size of
//!   the rewrite, and under `--minify` a whole module collapses to one
//!   output line, so every mapping for it points at line 1 (module-level
//!   granularity — token-level mappings are a follow-up).

use serde::{Deserialize, Serialize};

/// The deserialized `.lua.map` payload. See the module docs for the format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleMap {
    /// Format version; this crate writes and reads version 1.
    pub version: u32,
    /// File name of the bundle this map describes (basename, e.g. `app.lua`).
    pub bundle: String,
    /// Module files, project-root-relative with forward slashes.
    pub files: Vec<String>,
    /// Per-output-line mappings; index `i` is bundle line `i + 1`.
    pub lines: Vec<Option<(usize, u32)>>,
}

impl BundleMap {
    /// Serialize to the on-disk JSON form.
    #[allow(
        clippy::missing_panics_doc,
        reason = "plain data struct; serde_json serialization cannot fail"
    )]
    pub fn to_json(&self) -> String {
        #[expect(
            clippy::expect_used,
            reason = "BundleMap is a plain data struct; serde_json serialization cannot fail"
        )]
        let json = serde_json::to_string(self).expect("BundleMap serialization cannot fail");
        json
    }

    /// Parse the on-disk JSON form.
    pub fn from_json(text: &str) -> Result<Self, crate::BundleError> {
        let map: Self =
            serde_json::from_str(text).map_err(|e| crate::BundleError::SourceMap(e.to_string()))?;
        if map.version != 1 {
            return Err(crate::BundleError::SourceMapVersion(map.version));
        }
        Ok(map)
    }

    /// The `(file, original_line)` a 1-based bundle line maps to, if any.
    pub fn lookup(&self, line: u32) -> Option<(&str, u32)> {
        let entry = self
            .lines
            .get(usize::try_from(line).ok()?.checked_sub(1)?)?;
        let (file, original) = entry.as_ref()?;
        Some((self.files.get(*file)?.as_str(), *original))
    }
}

/// Rewrite every `<bundle>:<line>` reference in `traceback` to the mapped
/// `<file>:<original line>` — the engine behind `luabox unmap`.
///
/// `names` are the spellings of the bundle a traceback may use (the path as
/// the user typed it, its basename, the map's own `bundle` field, slash
/// variants); the longest match at each position wins. References whose
/// line has no mapping (bundler-generated lines) are left untouched.
pub fn unmap_traceback(map: &BundleMap, names: &[String], traceback: &str) -> String {
    let mut names: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|n| !n.is_empty())
        .collect();
    // Longest first, so `dist/app.lua:3` is not half-matched as `app.lua:3`.
    names.sort_by_key(|n| std::cmp::Reverse(n.len()));
    names.dedup();

    let mut out = String::with_capacity(traceback.len());
    let mut rest = traceback;
    'scan: while !rest.is_empty() {
        for name in &names {
            if let Some(tail) = rest.strip_prefix(name)
                && let Some((line, tail)) = split_line_suffix(tail)
            {
                if let Some((file, original)) = map.lookup(line) {
                    out.push_str(file);
                    out.push(':');
                    out.push_str(&original.to_string());
                } else {
                    // No mapping (bundler-generated line): keep verbatim.
                    out.push_str(name);
                    out.push(':');
                    out.push_str(&line.to_string());
                }
                rest = tail;
                continue 'scan;
            }
        }
        let mut chars = rest.chars();
        let Some(ch) = chars.next() else {
            break;
        };
        out.push(ch);
        rest = chars.as_str();
    }
    out
}

/// Split a leading `:<digits>` off `tail`, returning the parsed line and
/// the remainder. `None` when `tail` does not start with `:<digit>`.
fn split_line_suffix(tail: &str) -> Option<(u32, &str)> {
    let tail = tail.strip_prefix(':')?;
    let digits = tail.len() - tail.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return None;
    }
    // `digits` counts leading ASCII digit bytes, so it lands on a char
    // boundary — `split_at` cannot panic here.
    let (num, rest) = tail.split_at(digits);
    let line: u32 = num.parse().ok()?;
    Some((line, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> BundleMap {
        BundleMap {
            version: 1,
            bundle: "app.lua".into(),
            files: vec!["src/main.lua".into(), "src/util.lua".into()],
            lines: vec![None, Some((1, 1)), Some((1, 2)), Some((0, 1))],
        }
    }

    #[test]
    fn json_round_trip() {
        let map = sample();
        let parsed = BundleMap::from_json(&map.to_json()).expect("round trip");
        assert_eq!(parsed.files, map.files);
        assert_eq!(parsed.lines, map.lines);
    }

    #[test]
    fn lookup_is_one_based_and_none_for_generated_lines() {
        let map = sample();
        assert_eq!(map.lookup(1), None);
        assert_eq!(map.lookup(2), Some(("src/util.lua", 1)));
        assert_eq!(map.lookup(4), Some(("src/main.lua", 1)));
        assert_eq!(map.lookup(0), None);
        assert_eq!(map.lookup(99), None);
    }

    #[test]
    fn unmap_rewrites_mapped_references_only() {
        let map = sample();
        let names = vec!["dist/app.lua".to_string(), "app.lua".to_string()];
        let traceback = "lua: dist/app.lua:3: boom\nstack traceback:\n\
                         \tapp.lua:3: in main chunk\n\tapp.lua:1: internals\n";
        let out = unmap_traceback(&map, &names, traceback);
        assert!(out.contains("lua: src/util.lua:2: boom"), "{out}");
        assert!(out.contains("\tsrc/util.lua:2: in main chunk"), "{out}");
        assert!(
            out.contains("\tapp.lua:1: internals"),
            "unmapped line kept: {out}"
        );
    }

    #[test]
    fn version_gate() {
        let err = BundleMap::from_json(r#"{"version":2,"bundle":"x","files":[],"lines":[]}"#);
        assert!(err.is_err());
    }
}
