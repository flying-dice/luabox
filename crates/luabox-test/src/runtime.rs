//! Runtime resolution (SPEC.md §11, §12).
//!
//! Luabox is never a runtime — it *acquires* one. With the toolchain manager
//! (#27) landed, resolution consults managed toolchains **before** `PATH`.
//! The single-runtime order for `luabox test` / `luabox run` is:
//!
//!   1. **Project pin** — the nearest `luabox-toolchain.toml` (walking up from
//!      the project root) names a toolchain id; that toolchain wins outright.
//!      A pin that names a toolchain which isn't installed is a hard error
//!      (we never silently fall back past an explicit pin).
//!   2. **`LUABOX_LUA`** — a documented interpreter override (an existing file
//!      or a program on `PATH`).
//!   3. **Managed toolchains** — a toolchain in `~/.luabox/toolchains`
//!      (`LUABOX_TOOLCHAINS` override) whose id matches the manifest edition.
//!   4. **`PATH`** — the edition's candidate binary names, probed in order.
//!
//! `--matrix` ignores the edition and probes the whole known set
//! (`5.1/5.2/5.3/5.4/luajit` plus a generic `lua`), degrading gracefully to
//! whatever is installed.
//!
//! ## For `luabox run` (#28)
//!
//! [`resolve_default`] is the single blessed entry point for one-runtime
//! resolution — call it with the edition and the project root; it threads the
//! pin lookup, the `LUABOX_LUA` override, managed toolchains and `PATH` in the
//! order above. Do not re-implement any leg of it.

use std::path::{Path, PathBuf};

/// The project-level runtime pin file (rust-toolchain.toml analog, SPEC §12).
/// A tiny TOML with a single `toolchain = "<id>"` key, written by
/// `luabox toolchain pin` and honored first by [`resolve_default`].
pub const PIN_FILE: &str = "luabox-toolchain.toml";

/// Candidate interpreter file names inside a managed toolchain directory, in
/// priority order. Version-specific names beat the generic ones so a mixed
/// install still resolves deterministically; `PATHEXT` supplies the
/// extensions (`.exe`, and — for hermetic fake runtimes — `.cmd`/`.bat`).
const TOOLCHAIN_INTERP_NAMES: &[&str] = &[
    "lua5.4", "lua54", "lua5.3", "lua53", "lua5.2", "lua52", "lua5.1", "lua51", "luajit", "lua",
];

/// A resolved way to launch a Lua runtime: `program` plus any fixed leading
/// `args`. The runner appends the harness path and one test file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSpec {
    /// Human label for reports (e.g. `"5.4"`, `"luajit"`, `"LUABOX_LUA"`,
    /// `"toolchain:5.4.6"`).
    pub label: String,
    /// Executable to spawn (a bare name resolved on PATH, or a full path).
    pub program: String,
    /// Fixed leading arguments (empty for a plain interpreter).
    pub args: Vec<String>,
}

/// Why a default runtime could not be resolved. Rendered with a clear,
/// actionable message that names exactly what was probed.
#[derive(Debug, Clone)]
pub enum ResolveError {
    /// A `luabox-toolchain.toml` pins a toolchain that isn't installed.
    PinNotInstalled { id: String },
    /// `LUABOX_LUA` was set but its value isn't an existing file or a
    /// program on `PATH`.
    OverrideMissing { value: String },
    /// No candidate binary for the edition was found (managed toolchains or
    /// `PATH`).
    NotFound {
        edition: String,
        probed: Vec<String>,
    },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PinNotInstalled { id } => write!(
                f,
                "the project pins toolchain `{id}` (in {PIN_FILE}), but it is \
                 not installed. Run `luabox toolchain install {id}`, or edit \
                 the pin"
            ),
            Self::OverrideMissing { value } => write!(
                f,
                "LUABOX_LUA is set to `{value}`, but no such runtime was found \
                 (not an existing file, and not on PATH)"
            ),
            Self::NotFound { edition, probed } => write!(
                f,
                "no Lua runtime found for edition `{edition}` \
                 (probed managed toolchains and PATH: {}). Install one with \
                 `luabox toolchain install`, put a Lua on PATH, or set \
                 LUABOX_LUA to an interpreter",
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

/// The inputs to single-runtime resolution, gathered explicitly so the core
/// can be unit-tested without touching the process environment (the
/// workspace denies `unsafe_code`, so `set_var` is out).
struct ResolveInputs {
    edition: String,
    /// The pinned toolchain id, if a `luabox-toolchain.toml` was found.
    pin: Option<String>,
    /// The managed-toolchains root, if one can be located.
    toolchains_dir: Option<PathBuf>,
    /// The `LUABOX_LUA` value, if set and non-empty.
    override_value: Option<String>,
}

/// Resolve the single runtime `luabox test` / `luabox run` should use for
/// `edition`, rooted at `root` (the project root, for the pin lookup).
///
/// This is the blessed entry point — see the module docs for the full order.
pub fn resolve_default(edition: &str, root: &Path) -> Result<RuntimeSpec, ResolveError> {
    let inputs = ResolveInputs {
        edition: edition.to_string(),
        pin: read_pin(root).map(|(id, _)| id),
        toolchains_dir: toolchains_dir(),
        override_value: env_override(),
    };
    resolve_core(&inputs)
}

/// The environment-free core of [`resolve_default`].
fn resolve_core(inputs: &ResolveInputs) -> Result<RuntimeSpec, ResolveError> {
    // 1. Project pin — an explicit pin wins, and never silently falls back.
    if let Some(id) = &inputs.pin {
        if let Some(dir) = &inputs.toolchains_dir
            && let Some(interp) = toolchain_interpreter(&dir.join(id))
        {
            return Ok(toolchain_spec(id, &interp));
        }
        return Err(ResolveError::PinNotInstalled { id: id.clone() });
    }

    // 2. LUABOX_LUA override.
    if let Some(value) = &inputs.override_value {
        return if find_on_path(value).is_some() {
            Ok(RuntimeSpec {
                label: "LUABOX_LUA".to_string(),
                program: value.clone(),
                args: Vec::new(),
            })
        } else {
            Err(ResolveError::OverrideMissing {
                value: value.clone(),
            })
        };
    }

    // 3. A managed toolchain matching the edition.
    if let Some(dir) = &inputs.toolchains_dir {
        for id in installed_toolchains(dir) {
            if matches_edition(&id, &inputs.edition)
                && let Some(interp) = toolchain_interpreter(&dir.join(&id))
            {
                return Ok(toolchain_spec(&id, &interp));
            }
        }
    }

    // 4. PATH.
    let probed = candidate_names(&inputs.edition);
    for name in &probed {
        if let Some(resolved) = find_on_path(name) {
            return Ok(RuntimeSpec {
                label: inputs.edition.clone(),
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
        edition: inputs.edition.clone(),
        probed,
    })
}

/// A [`RuntimeSpec`] for a managed toolchain interpreter path.
fn toolchain_spec(id: &str, interp: &Path) -> RuntimeSpec {
    RuntimeSpec {
        label: format!("toolchain:{id}"),
        program: interp.to_string_lossy().into_owned(),
        args: Vec::new(),
    }
}

/// Does a toolchain `id` satisfy `edition`? `5.4` is satisfied by `5.4` or
/// any `5.4.x`; `luajit` by `luajit` or `luajit-<ver>`.
fn matches_edition(id: &str, edition: &str) -> bool {
    if edition == "luajit" {
        id == "luajit" || id.starts_with("luajit-") || id.starts_with("luajit")
    } else {
        id == edition || id.starts_with(&format!("{edition}."))
    }
}

/// The managed-toolchains root: `LUABOX_TOOLCHAINS` override, else
/// `<home>/.luabox/toolchains`. `None` when no home can be located.
#[must_use]
pub fn toolchains_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("LUABOX_TOOLCHAINS")
        && !dir.trim().is_empty()
    {
        return Some(PathBuf::from(dir));
    }
    Some(home_dir()?.join(".luabox").join("toolchains"))
}

/// `$HOME` (unix) / `%USERPROFILE%` (windows). No directory-discovery crate,
/// matching `luabox-store`'s design.
fn home_dir() -> Option<PathBuf> {
    for var in ["HOME", "USERPROFILE"] {
        if let Ok(dir) = std::env::var(var)
            && !dir.trim().is_empty()
        {
            return Some(PathBuf::from(dir));
        }
    }
    None
}

/// Installed toolchain ids: the immediate sub-directories of `dir` whose name
/// doesn't start with `.` (staging dirs are hidden), sorted for determinism.
#[must_use]
pub fn installed_toolchains(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .collect();
    ids.sort();
    ids
}

/// The interpreter inside a managed toolchain directory, if any. Searches the
/// directory root and a `bin/` sub-directory for a known interpreter name.
#[must_use]
pub fn toolchain_interpreter(dir: &Path) -> Option<PathBuf> {
    let exts = path_exts();
    for base in [dir.to_path_buf(), dir.join("bin")] {
        for name in TOOLCHAIN_INTERP_NAMES {
            for ext in &exts {
                let candidate = base.join(format!("{name}{ext}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// Read the nearest `luabox-toolchain.toml` walking up from `start`, returning
/// the pinned toolchain id and the file it came from.
#[must_use]
pub fn read_pin(start: &Path) -> Option<(String, PathBuf)> {
    let mut dir = Some(start);
    while let Some(current) = dir {
        let path = current.join(PIN_FILE);
        if path.is_file()
            && let Ok(text) = std::fs::read_to_string(&path)
            && let Some(id) = parse_pin(&text)
        {
            return Some((id, path));
        }
        dir = current.parent();
    }
    None
}

/// Extract the `toolchain = "<id>"` value from a pin file. A deliberately
/// tiny parser (no TOML dep in this crate): the first non-comment line of the
/// form `toolchain = "…"` wins.
fn parse_pin(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(rest) = line.strip_prefix("toolchain") else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let value = rest.trim().trim_matches('"').trim_matches('\'').trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
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
                    // Resolved path, not the bare name — see `resolve_core`.
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
    use super::{
        ResolveError, ResolveInputs, candidate_names, find_in_dirs, find_on_path,
        installed_toolchains, matches_edition, parse_pin, read_pin, resolve_core,
        toolchain_interpreter,
    };
    use std::fs;
    use std::path::Path;

    /// A fake interpreter file inside `dir/id` that the toolchain search will
    /// find on this platform (`.cmd` on Windows, plain `lua` on Unix).
    fn fake_toolchain(root: &Path, id: &str) {
        let dir = root.join(id);
        fs::create_dir_all(&dir).unwrap();
        let name = if cfg!(windows) { "lua.cmd" } else { "lua" };
        fs::write(dir.join(name), "").unwrap();
    }

    fn inputs(
        edition: &str,
        pin: Option<&str>,
        toolchains: Option<&Path>,
        override_value: Option<&str>,
    ) -> ResolveInputs {
        ResolveInputs {
            edition: edition.to_string(),
            pin: pin.map(str::to_string),
            toolchains_dir: toolchains.map(Path::to_path_buf),
            override_value: override_value.map(str::to_string),
        }
    }

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

        let spec = resolve_core(&inputs("5.4", None, None, Some(&fake.to_string_lossy()))).unwrap();
        assert_eq!(spec.label, "LUABOX_LUA");
        assert_eq!(spec.program, fake.to_string_lossy());
    }

    #[test]
    fn override_missing_is_reported() {
        // A bare name that cannot exist on PATH.
        let err = resolve_core(&inputs(
            "5.4",
            None,
            None,
            Some("luabox-no-such-runtime-zzz"),
        ));
        assert!(matches!(err, Err(ResolveError::OverrideMissing { .. })));
    }

    #[test]
    fn not_found_names_what_it_probed() {
        // No override, no toolchains, an edition whose candidates certainly
        // aren't present under these fabricated names.
        let err = resolve_core(&inputs("luajit", None, None, None));
        // luajit may actually be installed on some machines; only assert the
        // message shape when it is genuinely absent.
        if let Err(ResolveError::NotFound { probed, .. }) = err {
            assert_eq!(probed, ["luajit"]);
        }
    }

    #[test]
    fn pin_beats_override_and_path() {
        // A pinned toolchain wins even when LUABOX_LUA points at a real file.
        let toolchains = tempfile::tempdir().unwrap();
        fake_toolchain(toolchains.path(), "5.4.6");
        let other = tempfile::tempdir().unwrap();
        let override_file = other.path().join("override-lua");
        fs::write(&override_file, "").unwrap();

        let spec = resolve_core(&inputs(
            "5.4",
            Some("5.4.6"),
            Some(toolchains.path()),
            Some(&override_file.to_string_lossy()),
        ))
        .unwrap();
        assert_eq!(spec.label, "toolchain:5.4.6");
        assert!(spec.program.contains("5.4.6"));
    }

    #[test]
    fn pin_that_is_not_installed_is_a_hard_error() {
        let toolchains = tempfile::tempdir().unwrap();
        let err = resolve_core(&inputs("5.4", Some("5.4.6"), Some(toolchains.path()), None));
        assert!(matches!(err, Err(ResolveError::PinNotInstalled { .. })));
    }

    #[test]
    fn managed_toolchain_matching_edition_beats_path() {
        // No pin, no override: a toolchain matching the edition is used
        // before falling through to PATH.
        let toolchains = tempfile::tempdir().unwrap();
        fake_toolchain(toolchains.path(), "5.4.6");
        let spec = resolve_core(&inputs("5.4", None, Some(toolchains.path()), None)).unwrap();
        assert_eq!(spec.label, "toolchain:5.4.6");
    }

    #[test]
    fn matches_edition_rules() {
        assert!(matches_edition("5.4", "5.4"));
        assert!(matches_edition("5.4.6", "5.4"));
        assert!(!matches_edition("5.1.5", "5.4"));
        assert!(matches_edition("luajit", "luajit"));
        assert!(matches_edition("luajit-2.1", "luajit"));
        assert!(!matches_edition("5.4", "luajit"));
    }

    #[test]
    fn installed_toolchains_lists_visible_dirs_sorted() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("5.4.6")).unwrap();
        fs::create_dir_all(dir.path().join("5.1.5")).unwrap();
        fs::create_dir_all(dir.path().join(".staging")).unwrap();
        fs::write(dir.path().join("not-a-dir"), "").unwrap();
        assert_eq!(installed_toolchains(dir.path()), ["5.1.5", "5.4.6"]);
    }

    #[test]
    fn toolchain_interpreter_scans_root_and_bin() {
        let dir = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) {
            "luajit.exe"
        } else {
            "luajit"
        };
        fs::create_dir_all(dir.path().join("bin")).unwrap();
        fs::write(dir.path().join("bin").join(name), "").unwrap();
        assert!(toolchain_interpreter(dir.path()).is_some());
        assert!(toolchain_interpreter(&dir.path().join("missing")).is_none());
    }

    #[test]
    fn parse_pin_reads_the_toolchain_key() {
        assert_eq!(
            parse_pin("toolchain = \"5.4.6\"\n").as_deref(),
            Some("5.4.6")
        );
        assert_eq!(
            parse_pin("# a comment\ntoolchain = \"luajit-2.1\"\n").as_deref(),
            Some("luajit-2.1")
        );
        assert_eq!(parse_pin("# nothing here\n"), None);
    }

    #[test]
    fn read_pin_walks_up_from_a_subdirectory() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join(super::PIN_FILE), "toolchain = \"5.4.6\"\n").unwrap();
        let sub = root.path().join("src").join("deep");
        fs::create_dir_all(&sub).unwrap();
        let (id, _) = read_pin(&sub).expect("pin found by walking up");
        assert_eq!(id, "5.4.6");
    }
}
