//! Corpus discovery and the per-file `-- DIFFER:` header.
//!
//! Each corpus program is a normal `.lua` file whose first lines carry a
//! machine-readable annotation naming the dialect it is *written* in and the
//! dialects it should be *lowered to and checked against*:
//!
//! ```lua
//! -- DIFFER: from=5.4 targets=5.1,5.2
//! ```
//!
//! `targets=` is optional; when omitted it defaults to every dialect the
//! `from` dialect can be lowered to (the SPEC.md §2 downgrade lattice).

use std::path::{Path, PathBuf};

use luabox_syntax::Dialect;

/// The parsed `-- DIFFER:` header of one corpus file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// The dialect the program is written in (its source runtime).
    pub from: Dialect,
    /// The dialects to lower to and differentially execute against.
    pub targets: Vec<Dialect>,
}

/// The SPEC.md §2 downgrade lattice: the dialects `from` can be lowered *to*.
/// 5.1 is the floor (no lower targets); LuaJIT downgrades only to 5.1.
#[must_use]
pub fn default_targets(from: Dialect) -> Vec<Dialect> {
    match from {
        Dialect::Lua51 => vec![],
        Dialect::Lua52 => vec![Dialect::Lua51],
        Dialect::Lua53 => vec![Dialect::Lua52, Dialect::Lua51],
        Dialect::Lua54 => vec![Dialect::Lua53, Dialect::Lua52, Dialect::Lua51],
        Dialect::LuaJit => vec![Dialect::Lua51],
    }
}

/// The dialect id as it appears in a header (`5.1`, `luajit`, …).
fn dialect_from_id(id: &str) -> Result<Dialect, String> {
    Dialect::from_manifest_id(id).ok_or_else(|| {
        format!("unknown dialect `{id}` (expected one of 5.1, 5.2, 5.3, 5.4, luajit)")
    })
}

/// Parse the `-- DIFFER:` header out of a corpus file's text. Scans only the
/// leading comment block; the first `-- DIFFER:` line wins. Returns an error
/// describing what was malformed if the header is missing or unparseable.
pub fn parse_header(source: &str) -> Result<Header, String> {
    let spec = source
        .lines()
        .take_while(|l| {
            let t = l.trim_start();
            t.is_empty() || t.starts_with("--")
        })
        .find_map(|l| {
            let comment = l.trim_start().strip_prefix("--")?.trim_start();
            comment.strip_prefix("DIFFER:").map(str::trim)
        })
        .ok_or_else(|| "missing `-- DIFFER: from=<dialect> [targets=<a,b>]` header".to_string())?;

    let mut from: Option<Dialect> = None;
    let mut targets: Option<Vec<Dialect>> = None;
    for field in spec.split_whitespace() {
        let (key, value) = field
            .split_once('=')
            .ok_or_else(|| format!("malformed header field `{field}` (expected key=value)"))?;
        match key {
            "from" => from = Some(dialect_from_id(value)?),
            "targets" => {
                let list = value
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(dialect_from_id)
                    .collect::<Result<Vec<_>, _>>()?;
                targets = Some(list);
            }
            other => return Err(format!("unknown header key `{other}`")),
        }
    }

    let from = from.ok_or_else(|| "header is missing `from=<dialect>`".to_string())?;
    let targets = targets.unwrap_or_else(|| default_targets(from));
    Ok(Header { from, targets })
}

/// Every `.lua` file directly under `dir`, sorted for deterministic report
/// order. Files whose name starts with `_` are still included (they are
/// ordinary corpus entries); hidden dotfiles are skipped.
pub fn discover(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "lua"))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| !n.starts_with('.'))
        })
        .collect();
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_from_and_explicit_targets() {
        let h = parse_header("-- DIFFER: from=5.4 targets=5.1,5.2\nprint(1)\n").unwrap();
        assert_eq!(h.from, Dialect::Lua54);
        assert_eq!(h.targets, vec![Dialect::Lua51, Dialect::Lua52]);
    }

    #[test]
    fn targets_default_to_the_downgrade_lattice() {
        let h = parse_header("-- DIFFER: from=5.3\n").unwrap();
        assert_eq!(h.from, Dialect::Lua53);
        assert_eq!(h.targets, vec![Dialect::Lua52, Dialect::Lua51]);
    }

    #[test]
    fn luajit_defaults_to_51_only() {
        let h = parse_header("-- DIFFER: from=luajit\n").unwrap();
        assert_eq!(h.targets, vec![Dialect::Lua51]);
    }

    #[test]
    fn header_may_follow_a_leading_comment() {
        let src = "-- a title comment\n-- DIFFER: from=5.2\nprint(1)\n";
        assert_eq!(parse_header(src).unwrap().from, Dialect::Lua52);
    }

    #[test]
    fn missing_header_is_an_error() {
        assert!(parse_header("print(1)\n").is_err());
    }

    #[test]
    fn unknown_dialect_is_an_error() {
        assert!(parse_header("-- DIFFER: from=5.9\n").is_err());
    }

    #[test]
    fn header_after_code_is_not_picked_up() {
        // Only the leading comment block is scanned.
        assert!(parse_header("print(1)\n-- DIFFER: from=5.4\n").is_err());
    }

    #[test]
    fn five_one_has_no_default_targets() {
        assert_eq!(default_targets(Dialect::Lua51), Vec::<Dialect>::new());
    }
}
