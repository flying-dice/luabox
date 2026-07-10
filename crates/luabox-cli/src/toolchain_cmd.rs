//! `luabox toolchain [install|pin|list]` — the toolchain manager (SPEC.md §12,
//! ticket #27).
//!
//! An *acquirer* of runtimes, never a runtime. It downloads (or copies) a
//! prebuilt Lua interpreter, verifies its SHA-256, and unpacks it into
//! `~/.luabox/toolchains/<id>/` (`LUABOX_TOOLCHAINS` override). `pin` records a
//! project's chosen toolchain in `luabox-toolchain.toml`; `list` reports what
//! is installed and which is pinned. Runtime *resolution* — how `luabox test`
//! and `luabox run` pick an interpreter, honoring the pin before PATH — lives
//! in `luabox-test`'s [`runtime`](luabox_test::runtime) module.
//!
//! ## The index
//!
//! What is installable is described by a small TOML *index* mapping
//! `"<id>-<platform>"` to `{ url, sha256 }`. A built-in index
//! ([`BUILTIN_INDEX`]) ships in the binary with verified Windows entries and is
//! updated per release; `LUABOX_TOOLCHAIN_INDEX` names an additional index
//! (a local path, a `file://` URL, or an `http(s)` URL) whose entries override
//! the built-ins. `url` values may themselves be local paths / `file://` URLs,
//! which is how the hermetic tests install fixture archives with no network.
//!
//! ## Extraction
//!
//! Archives are unpacked with `tar -xf` — no zip crate is pulled in. On Windows
//! the bundled `bsdtar` (`%SystemRoot%\System32\tar.exe`, preferred so a
//! git-shipped GNU tar on PATH can't shadow it) handles `.zip`; `.tar.gz` works
//! with any `tar` on every platform.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use luabox_test::runtime::{
    PIN_FILE, installed_toolchains, read_pin, toolchain_interpreter, toolchains_dir,
};

/// The index shipped in the binary. Updated per release (see the file header).
const BUILTIN_INDEX: &str = include_str!("toolchain_index.toml");

/// One installable runtime: where to fetch it and its expected digest.
struct IndexEntry {
    url: String,
    sha256: String,
}

/// A parsed toolchain index: `"<id>-<platform>"` → [`IndexEntry`].
struct Index {
    entries: BTreeMap<String, IndexEntry>,
}

impl Index {
    /// Parse an index from TOML. Deliberately tolerant of extra keys.
    fn parse(text: &str) -> Result<Self> {
        let doc: toml_edit::DocumentMut =
            text.parse().context("toolchain index is not valid TOML")?;
        let mut entries = BTreeMap::new();
        if let Some(item) = doc.get("toolchain") {
            let table = item
                .as_table_like()
                .ok_or_else(|| anyhow!("`toolchain` must be a table of entries"))?;
            for (key, value) in table.iter() {
                let entry = value
                    .as_table_like()
                    .ok_or_else(|| anyhow!("index entry `{key}` must be a table"))?;
                let url = entry
                    .get("url")
                    .and_then(|i| i.as_str())
                    .ok_or_else(|| anyhow!("index entry `{key}` is missing a string `url`"))?;
                let sha256 = entry
                    .get("sha256")
                    .and_then(|i| i.as_str())
                    .ok_or_else(|| anyhow!("index entry `{key}` is missing a string `sha256`"))?;
                entries.insert(
                    key.to_string(),
                    IndexEntry {
                        url: url.to_string(),
                        sha256: sha256.to_string(),
                    },
                );
            }
        }
        Ok(Self { entries })
    }
}

/// The current platform id used to key the index: `"<os>-<arch>"`.
fn current_platform() -> String {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    format!("{}-{arch}", std::env::consts::OS)
}

/// Load the effective index: the built-in entries, then any entries from
/// `LUABOX_TOOLCHAIN_INDEX` layered on top (override wins).
fn load_index() -> Result<Index> {
    let mut index = Index::parse(BUILTIN_INDEX).context("built-in toolchain index is malformed")?;
    if let Ok(src) = std::env::var("LUABOX_TOOLCHAIN_INDEX")
        && !src.trim().is_empty()
    {
        let staging = tempfile::tempdir().context("cannot create a temp dir for the index")?;
        let path = obtain(&src, staging.path())
            .with_context(|| format!("cannot fetch toolchain index `{src}`"))?;
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read toolchain index `{src}`"))?;
        let extra =
            Index::parse(&text).with_context(|| format!("toolchain index `{src}` is malformed"))?;
        index.entries.extend(extra.entries);
    }
    Ok(index)
}

/// Interpret an index `url` (or the index location itself) as a local path if
/// it is one: a `file://` URL or an existing filesystem path.
fn local_path(url: &str) -> Option<PathBuf> {
    if let Some(rest) = url.strip_prefix("file://") {
        // `file:///C:/x` (windows) and `file:///home/x` (unix). Strip the
        // authority-empty leading slash before a Windows drive letter.
        let trimmed = if cfg!(windows) {
            rest.trim_start_matches('/')
        } else {
            rest
        };
        return Some(PathBuf::from(trimmed));
    }
    let path = Path::new(url);
    path.exists().then(|| path.to_path_buf())
}

/// Obtain the bytes named by `url` into `staging`, returning the local file
/// path. Local paths / `file://` URLs are used in place; `http(s)` URLs are
/// downloaded with `curl -fL`.
fn obtain(url: &str, staging: &Path) -> Result<PathBuf> {
    if let Some(path) = local_path(url) {
        if !path.is_file() {
            bail!("`{}` does not exist", path.display());
        }
        return Ok(path);
    }
    let name = url
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or("archive");
    let dest = staging.join(name);
    let status = Command::new("curl")
        .args(["-fsSL", "--max-time", "120", "-o"])
        .arg(&dest)
        .arg(url)
        .status()
        .context("failed to run `curl` (needed to download over the network)")?;
    if !status.success() {
        bail!("download failed: `curl` exited with {status} for `{url}`");
    }
    Ok(dest)
}

/// `tar` to shell out to. On Windows, prefer the system `bsdtar` so a
/// git-shipped GNU tar on PATH (which can't read `.zip`) doesn't shadow it.
fn tar_program() -> PathBuf {
    if cfg!(windows)
        && let Ok(root) = std::env::var("SystemRoot")
    {
        let system_tar = Path::new(&root).join("System32").join("tar.exe");
        if system_tar.is_file() {
            return system_tar;
        }
    }
    PathBuf::from("tar")
}

/// Unpack `archive` into `dest` with `tar -xf` (auto-detecting `.zip` /
/// `.tar.gz`).
fn extract(archive: &Path, dest: &Path) -> Result<()> {
    let tar = tar_program();
    let status = Command::new(&tar)
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .status()
        .with_context(|| {
            format!(
                "failed to run `{}` — archive extraction needs `tar` on PATH",
                tar.display()
            )
        })?;
    if !status.success() {
        bail!(
            "`tar -xf` failed to unpack `{}` (exit {})",
            archive.display(),
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

/// Install toolchain `id` into `dir` from `index`, verifying the SHA-256 and
/// extracting the archive. Returns the installed toolchain directory. The
/// environment-free core of [`install`], so it can be unit-tested with a local
/// index and a temp root.
fn install_into(dir: &Path, index: &Index, id: &str, platform: &str) -> Result<PathBuf> {
    let dest = dir.join(id);
    if toolchain_interpreter(&dest).is_some() {
        return Ok(dest);
    }

    let key = format!("{id}-{platform}");
    let entry = index.entries.get(&key).ok_or_else(|| {
        anyhow!(
            "no toolchain `{id}` is available for platform `{platform}` \
             (looked up `{key}`). Set LUABOX_TOOLCHAIN_INDEX to an index that \
             provides it, or see SPEC.md §12"
        )
    })?;

    let staging = tempfile::tempdir().context("cannot create a temp dir for the download")?;
    let archive = obtain(&entry.url, staging.path())
        .with_context(|| format!("cannot fetch toolchain `{id}` from `{}`", entry.url))?;

    let digest = luabox_store::hash_file(&archive)
        .with_context(|| format!("cannot hash `{}`", archive.display()))?;
    if !digest.eq_ignore_ascii_case(&entry.sha256) {
        bail!(
            "checksum mismatch for toolchain `{id}`: expected {}, got {digest}. \
             Refusing to install a corrupt or tampered archive",
            entry.sha256
        );
    }

    // Extract into a hidden staging dir *under the toolchains root* (same
    // volume as `dest`, so the final rename is atomic) then swap it in.
    std::fs::create_dir_all(dir)
        .with_context(|| format!("cannot create toolchains dir `{}`", dir.display()))?;
    let unpack = dir.join(format!(".{id}.staging"));
    if unpack.exists() {
        std::fs::remove_dir_all(&unpack).ok();
    }
    std::fs::create_dir_all(&unpack)
        .with_context(|| format!("cannot create staging dir `{}`", unpack.display()))?;
    extract(&archive, &unpack)?;

    if toolchain_interpreter(&unpack).is_none() {
        std::fs::remove_dir_all(&unpack).ok();
        bail!("archive for `{id}` contained no recognizable Lua interpreter");
    }

    if dest.exists() {
        std::fs::remove_dir_all(&dest).ok();
    }
    std::fs::rename(&unpack, &dest)
        .with_context(|| format!("cannot move the toolchain into `{}`", dest.display()))?;
    Ok(dest)
}

/// The managed-toolchains root, or a clear error if no home can be located.
fn require_toolchains_dir() -> Result<PathBuf> {
    toolchains_dir().ok_or_else(|| {
        anyhow!(
            "cannot locate a toolchains directory (set LUABOX_TOOLCHAINS, HOME, or USERPROFILE)"
        )
    })
}

/// `luabox toolchain install <id>`.
pub fn install(_cwd: &Path, id: &str) -> Result<()> {
    let dir = require_toolchains_dir()?;
    let dest = dir.join(id);
    if toolchain_interpreter(&dest).is_some() {
        println!(
            "toolchain `{id}` is already installed at {}",
            dest.display()
        );
        return Ok(());
    }
    let index = load_index()?;
    let dest = install_into(&dir, &index, id, &current_platform())?;
    println!("installed toolchain `{id}` to {}", dest.display());
    Ok(())
}

/// `luabox toolchain pin <id>` — record the project's runtime in
/// `luabox-toolchain.toml`. Requires the toolchain to be installed.
pub fn pin(cwd: &Path, id: &str) -> Result<()> {
    let dir = require_toolchains_dir()?;
    if toolchain_interpreter(&dir.join(id)).is_none() {
        bail!("cannot pin `{id}`: it is not installed. Run `luabox toolchain install {id}` first");
    }
    let contents = format!(
        "# Pins the Lua runtime for this project (rust-toolchain.toml analog).\n\
         # Managed by `luabox toolchain pin` (SPEC.md §12).\n\
         toolchain = \"{id}\"\n"
    );
    let path = cwd.join(PIN_FILE);
    std::fs::write(&path, contents)
        .with_context(|| format!("cannot write `{}`", path.display()))?;
    println!("pinned toolchain `{id}` ({})", path.display());
    Ok(())
}

/// `luabox toolchain list` — installed toolchains and which is pinned here.
pub fn list(cwd: &Path) -> Result<()> {
    let dir = require_toolchains_dir()?;
    let installed = installed_toolchains(&dir);
    let pinned = read_pin(cwd).map(|(id, _)| id);

    if installed.is_empty() {
        println!("no toolchains installed (install one with `luabox toolchain install <id>`)");
    } else {
        println!("installed toolchains (in {}):", dir.display());
        for id in &installed {
            let marker = if pinned.as_deref() == Some(id) {
                "  (pinned)"
            } else {
                ""
            };
            println!("  {id}{marker}");
        }
    }

    if let Some(pinned) = &pinned
        && !installed.iter().any(|id| id == pinned)
    {
        println!("note: this project pins `{pinned}`, which is not installed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Index, current_platform, install_into};
    use luabox_test::runtime::toolchain_interpreter;
    use std::path::Path;
    use std::process::Command;

    /// Build a `.tar.gz` fixture at `archive` containing a single fake
    /// interpreter file (named so the toolchain search finds it on this
    /// platform), and return its SHA-256.
    fn make_fixture(archive: &Path) -> String {
        let src = tempfile::tempdir().unwrap();
        let name = if cfg!(windows) { "lua.cmd" } else { "lua" };
        std::fs::write(src.path().join(name), "@echo off\r\n").unwrap();
        let status = Command::new("tar")
            .arg("-czf")
            .arg(archive)
            .arg("-C")
            .arg(src.path())
            .arg(name)
            .status()
            .expect("tar must be available to build the fixture");
        assert!(status.success(), "tar failed to build the fixture archive");
        luabox_store::hash_file(archive).unwrap()
    }

    /// A one-entry index whose `url` is the local fixture archive.
    fn index_for(id: &str, archive: &Path, sha256: &str) -> Index {
        let key = format!("{id}-{}", current_platform());
        let toml = format!(
            "[toolchain.\"{key}\"]\nurl = \"{}\"\nsha256 = \"{sha256}\"\n",
            archive.display().to_string().replace('\\', "/")
        );
        Index::parse(&toml).unwrap()
    }

    #[test]
    fn install_from_local_index_unpacks_a_runtime() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("fixture.tar.gz");
        let sha = make_fixture(&archive);
        let index = index_for("5.4", &archive, &sha);

        let dir = root.path().join("toolchains");
        let dest = install_into(&dir, &index, "5.4", &current_platform()).unwrap();
        assert!(toolchain_interpreter(&dest).is_some());

        // Idempotent: a second install is a no-op that still succeeds.
        let again = install_into(&dir, &index, "5.4", &current_platform()).unwrap();
        assert_eq!(again, dest);
    }

    #[test]
    fn corrupt_checksum_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("fixture.tar.gz");
        make_fixture(&archive);
        let bad = "0".repeat(64);
        let index = index_for("5.4", &archive, &bad);

        let dir = root.path().join("toolchains");
        let err = install_into(&dir, &index, "5.4", &current_platform()).unwrap_err();
        assert!(
            err.to_string().contains("checksum mismatch"),
            "unexpected error: {err}"
        );
        // Nothing left installed after a rejected install.
        assert!(toolchain_interpreter(&dir.join("5.4")).is_none());
    }

    #[test]
    fn missing_platform_entry_is_a_clear_error() {
        let index = Index::parse("").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let err = install_into(dir.path(), &index, "5.4", &current_platform()).unwrap_err();
        assert!(err.to_string().contains("no toolchain `5.4`"), "{err}");
    }
}
