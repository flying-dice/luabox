//! Runtime resolution (SPEC.md §11, §12).
//!
//! Luabox is never a runtime — it *acquires* one. Until the toolchain
//! manager (#27) lands, acquisition is PATH-based:
//!
//!   * `LUABOX_LUA` env var, if set, wins outright (documented override /
//!     the toolchain-pin stand-in until #27).
//!   * otherwise the manifest `[package] edition` picks an ordered list of
//!     candidate binary names, probed on `PATH`; first hit wins.
//!
//! `--matrix` ignores the edition and probes the whole known set
//! (`5.1/5.2/5.3/5.4/luajit` plus a generic `lua`), degrading gracefully to
//! whatever is installed.

use std::path::{Path, PathBuf};

/// A resolved way to launch a Lua runtime: `program` plus any fixed leading
/// `args`. The runner appends the harness path and one test file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSpec {
    /// Human label for reports (e.g. `"5.4"`, `"luajit"`, `"LUABOX_LUA"`).
    pub label: String,
    /// Executable to spawn (a bare name resolved on PATH, or a full path).
    pub program: String,
    /// Fixed leading arguments (empty for a plain interpreter).
    pub args: Vec<String>,
}

/// Why a default runtime could not be resolved. Rendered with a clear,
/// actionable message that names exactly what was probed and points at #27.
#[derive(Debug, Clone)]
pub enum ResolveError {
    /// `LUABOX_LUA` was set but its value isn't an existing file or a
    /// program on `PATH`.
    OverrideMissing { value: String },
    /// No candidate binary for the edition was found on `PATH`.
    NotFound {
        edition: String,
        probed: Vec<String>,
    },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OverrideMissing { value } => write!(
                f,
                "LUABOX_LUA is set to `{value}`, but no such runtime was found \
                 (not an existing file, and not on PATH)"
            ),
            Self::NotFound { edition, probed } => write!(
                f,
                "no Lua runtime found for edition `{edition}` \
                 (probed on PATH: {}). Install one and put it on PATH, or set \
                 LUABOX_LUA to an interpreter; managed toolchains arrive with \
                 `luabox toolchain` (#27)",
                probed.join(", ")
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

/// Candidate binary names for an edition, in priority order. Unknown
/// editions fall back to the bare `lua`.
#[must_use]
pub fn candidate_names(edition: &str) -> Vec<String> {
    let names: &[&str] = match edition {
        "5.1" => &["lua5.1", "lua51", "lua"],
        "5.2" => &["lua5.2", "lua52", "lua"],
        "5.3" => &["lua5.3", "lua53", "lua"],
        "5.4" => &["lua5.4", "lua54", "lua"],
        "luajit" => &["luajit"],
        _ => &["lua"],
    };
    names.iter().map(|s| (*s).to_string()).collect()
}

/// Resolve the single runtime `luabox test` should use for `edition`,
/// honoring the `LUABOX_LUA` override.
pub fn resolve_default(edition: &str) -> Result<RuntimeSpec, ResolveError> {
    resolve_with_override(edition, env_override())
}

/// The override-aware core of [`resolve_default`], taking the override value
/// explicitly so it can be unit-tested without mutating the process
/// environment (the workspace denies `unsafe_code`, so `set_var` is out).
fn resolve_with_override(
    edition: &str,
    override_value: Option<String>,
) -> Result<RuntimeSpec, ResolveError> {
    if let Some(value) = override_value {
        return if find_on_path(&value).is_some() {
            Ok(RuntimeSpec {
                label: "LUABOX_LUA".to_string(),
                program: value,
                args: Vec::new(),
            })
        } else {
            Err(ResolveError::OverrideMissing { value })
        };
    }

    let probed = candidate_names(edition);
    for name in &probed {
        if let Some(resolved) = find_on_path(name) {
            return Ok(RuntimeSpec {
                label: edition.to_string(),
                // The *resolved* path, not the bare name: on Windows,
                // `CreateProcess` won't append `.exe` to a name that already
                // looks like it has an extension (`lua5.1` → ext `.1`), so a
                // bare `lua5.1` would fail to launch.
                program: resolved.to_string_lossy().into_owned(),
                args: Vec::new(),
            });
        }
    }
    Err(ResolveError::NotFound {
        edition: edition.to_string(),
        probed,
    })
}

/// The `(label, candidate names)` set probed by `--matrix`, in report order.
const MATRIX_LABELS: &[(&str, &[&str])] = &[
    ("5.1", &["lua5.1", "lua51"]),
    ("5.2", &["lua5.2", "lua52"]),
    ("5.3", &["lua5.3", "lua53"]),
    ("5.4", &["lua5.4", "lua54"]),
    ("luajit", &["luajit"]),
    ("lua", &["lua"]),
];

/// The outcome of probing the whole matrix: the runtimes that were found,
/// and the labels that weren't (for a "what's missing" note).
#[derive(Debug, Clone, Default)]
pub struct MatrixResolution {
    pub found: Vec<RuntimeSpec>,
    pub missing: Vec<String>,
}

/// Probe every known runtime for `--matrix`. If `LUABOX_LUA` is set it is
/// added as an extra entry (so a hermetic fake runtime can drive the matrix
/// too). Runtimes resolving to the same executable path are de-duplicated,
/// so `lua` pointing at the same binary as `lua5.1` isn't run twice.
#[must_use]
pub fn resolve_matrix() -> MatrixResolution {
    let mut resolution = MatrixResolution::default();
    let mut seen_paths: Vec<PathBuf> = Vec::new();

    if let Some(value) = env_override()
        && let Some(resolved) = find_on_path(&value)
    {
        seen_paths.push(resolved);
        resolution.found.push(RuntimeSpec {
            label: "LUABOX_LUA".to_string(),
            program: value,
            args: Vec::new(),
        });
    }

    for (label, names) in MATRIX_LABELS {
        let mut hit = None;
        for name in *names {
            if let Some(resolved) = find_on_path(name) {
                hit = Some((name, resolved));
                break;
            }
        }
        match hit {
            Some((_name, resolved)) if !seen_paths.contains(&resolved) => {
                resolution.found.push(RuntimeSpec {
                    label: (*label).to_string(),
                    // Resolved path, not the bare name — see `resolve_default`.
                    program: resolved.to_string_lossy().into_owned(),
                    args: Vec::new(),
                });
                seen_paths.push(resolved);
            }
            // Found but a duplicate binary: don't list as missing either.
            Some(_) => {}
            None => resolution.missing.push((*label).to_string()),
        }
    }

    resolution
}

fn env_override() -> Option<String> {
    match std::env::var("LUABOX_LUA") {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => None,
    }
}

/// Resolve `name` to an executable path: a full/relative path is checked
/// directly, a bare name is searched on `PATH` (honoring `PATHEXT` on
/// Windows). Returns the resolved path if it exists.
#[must_use]
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let as_path = Path::new(name);
    let looks_like_path =
        as_path.is_absolute() || as_path.components().count() > 1 || name.contains(['/', '\\']);
    if looks_like_path {
        return as_path.is_file().then(|| as_path.to_path_buf());
    }

    let path_var = std::env::var_os("PATH")?;
    let dirs: Vec<PathBuf> = std::env::split_paths(&path_var).collect();
    find_in_dirs(name, &dirs, &path_exts())
}

/// Executable extensions to try for a bare name. `[""]` on Unix; the
/// `PATHEXT` list (lower-cased) on Windows, always including the empty
/// extension so an exact match still works.
fn path_exts() -> Vec<String> {
    if cfg!(windows) {
        let raw = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.BAT;.CMD;.COM".to_string());
        let mut exts = vec![String::new()];
        for ext in raw.split(';') {
            let ext = ext.trim();
            if !ext.is_empty() {
                exts.push(ext.to_ascii_lowercase());
            }
        }
        exts
    } else {
        vec![String::new()]
    }
}

/// Pure PATH search: the first `dir/name{ext}` that is a file. Extracted so
/// the search can be unit-tested against a temp directory without touching
/// the process environment.
#[must_use]
pub fn find_in_dirs(name: &str, dirs: &[PathBuf], exts: &[String]) -> Option<PathBuf> {
    for dir in dirs {
        for ext in exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{ResolveError, candidate_names, find_in_dirs, find_on_path, resolve_with_override};
    use std::fs;

    #[test]
    fn candidate_names_per_edition() {
        assert_eq!(candidate_names("5.4"), ["lua5.4", "lua54", "lua"]);
        assert_eq!(candidate_names("5.1"), ["lua5.1", "lua51", "lua"]);
        assert_eq!(candidate_names("luajit"), ["luajit"]);
        // Unknown editions fall back to the bare interpreter.
        assert_eq!(candidate_names("weird"), ["lua"]);
    }

    #[test]
    fn find_in_dirs_finds_first_hit() {
        let dir = tempfile::tempdir().unwrap();
        let exe = if cfg!(windows) {
            "lua5.4.exe"
        } else {
            "lua5.4"
        };
        fs::write(dir.path().join(exe), "").unwrap();
        let exts = if cfg!(windows) {
            vec![String::new(), ".exe".to_string()]
        } else {
            vec![String::new()]
        };
        let hit = find_in_dirs("lua5.4", &[dir.path().to_path_buf()], &exts);
        assert_eq!(hit, Some(dir.path().join(exe)));
        assert!(find_in_dirs("nope", &[dir.path().to_path_buf()], &exts).is_none());
    }

    #[test]
    fn override_pointing_at_a_real_file_resolves() {
        // A real, existing file used as a stand-in interpreter path.
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("fake-lua");
        fs::write(&fake, "").unwrap();
        assert!(find_on_path(&fake.to_string_lossy()).is_some());

        let spec = resolve_with_override("5.4", Some(fake.to_string_lossy().into_owned())).unwrap();
        assert_eq!(spec.label, "LUABOX_LUA");
        assert_eq!(spec.program, fake.to_string_lossy());
    }

    #[test]
    fn override_missing_is_reported() {
        // A bare name that cannot exist on PATH.
        let err = resolve_with_override("5.4", Some("luabox-no-such-runtime-zzz".to_string()));
        assert!(matches!(err, Err(ResolveError::OverrideMissing { .. })));
    }

    #[test]
    fn not_found_names_what_it_probed() {
        // No override, an edition whose candidates certainly aren't present
        // under these fabricated names.
        let err = resolve_with_override("luajit", None);
        // luajit may actually be installed on some machines; only assert the
        // message shape when it is genuinely absent.
        if let Err(ResolveError::NotFound { probed, .. }) = err {
            assert_eq!(probed, ["luajit"]);
        }
    }
}
