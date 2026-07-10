//! Bundler embedding modes (SPEC.md §7, ticket #32).
//!
//! `luabox bundle --mode <mode>` (or `[build] mode` in the manifest) picks
//! how the single-file bundle from `luabox_bundle::bundle` is packaged for
//! its target runtime. All three modes bundle the project exactly the same
//! way (`bundle_cmd::run` builds the [`luabox_bundle::Bundle`] once); this
//! module only decides where the resulting text lands on disk and what, if
//! anything, goes alongside it.
//!
//! - **`plain`** (default): the bundle is written out verbatim as
//!   `<out>/<name>.lua` — unchanged from before this ticket.
//! - **`love`**: [LÖVE](https://love2d.org) loads a `.love` archive — a
//!   plain zip file — and requires a `main.lua` at the archive root
//!   defining `love.load`/`love.update`/`love.draw`. `luabox bundle --mode
//!   love` writes the bundle as that `main.lua` **unmodified**: the bundle
//!   already inlines the entry chunk last as raw top-level code (SPEC.md
//!   §7 "single-file emit" — see [`luabox_bundle::bundle`]'s module docs),
//!   so it *is* a runnable chunk with the entry's own top-level `return`
//!   (if any) intact, and needs no shim rewriting to work as LÖVE's
//!   `main.lua`. LÖVE runs `conf.lua` in an isolated environment *before*
//!   `main.lua` loads — it cannot `require` anything the bundle defines —
//!   so when `src/conf.lua` exists it is bundled **separately** (its own
//!   require graph, its own `__luabox_rt` prelude if it needs one) and
//!   written as its own archive-root file. An `assets/` directory at the
//!   project root, if present, is copied into the archive verbatim: this
//!   is a luabox convention (not a LÖVE requirement) for keeping images/
//!   audio/fonts out of the require graph while still shipping them in the
//!   `.love` at the paths LÖVE's `love.filesystem` expects (relative to
//!   the archive root).
//! - **`nvim-plugin`**: Neovim's `runtimepath` plugin convention. `luabox
//!   bundle --mode nvim-plugin` writes `<out>/<name>/` containing:
//!     - `lua/<name>/init.lua` — the bundle, unmodified. `require("<name>")`
//!       finds it via Neovim's standard `lua/` runtimepath search, and
//!       (per the `love` mode note above) sees whatever `src/main.lua`
//!       itself would return, because the bundle is the entry chunk.
//!     - `plugin/<name>.lua` — a bootstrap stub Neovim auto-sources on
//!       startup. It does nothing by default and documents, in a comment,
//!       that the convention is lazy `require("<name>")` from user config
//!       rather than eager side effects at startup.
//!     - `doc/<name>.txt` — a minimal `:help`-format stub carrying the
//!       package description (`[package] description`, or a placeholder),
//!       so `:helptags` has a target to index.
//!
//! `--sourcemap` interacts with the modes as follows: `plain` writes
//! `<name>.lua.map` beside the bundle and `nvim-plugin` writes
//! `init.lua.map` beside `init.lua`; `love` mode has nowhere sensible to
//! put a map (`luabox unmap` expects `<bundle>.map` next to a bundle
//! *file*, and LÖVE tracebacks name `main.lua` inside the archive), so no
//! map is emitted there today — a documented follow-up, not an oversight.
//!
//! ## Zip creation (`love` mode)
//!
//! No dependency in this workspace writes zip archives, so `.love`
//! packaging shells out, per-platform:
//!
//! - **Windows**: the system `bsdtar`
//!   (`%SystemRoot%\System32\tar.exe`, the same trick
//!   `toolchain_cmd::tar_program` uses — bundled since Windows 10 1809, so
//!   present on every supported Windows version) via `tar --format zip -cf`.
//!   `--format zip` is required explicitly: `-a`'s extension-sniffing
//!   doesn't recognize `.love`, and silently falls back to a plain POSIX
//!   tar archive (verified empirically while fixing ticket #75). Unlike
//!   `Compress-Archive`, `bsdtar` reliably writes ZIP-spec forward-slash
//!   (`/`) entry separators regardless of Windows/.NET version — that
//!   reliability, not just availability, is why it's preferred over
//!   `Compress-Archive` for entries that are read on other platforms (a
//!   `.love` archive with backslash entries is a cross-platform hazard:
//!   `love.filesystem` and other unzip tools may not accept `\` as a path
//!   separator inside the archive). If `System32\tar.exe` isn't present
//!   (a non-standard Windows install), this falls back to
//!   `Compress-Archive` via `powershell -Command`; after it runs, the
//!   resulting archive's entries are inspected (via .NET
//!   `System.IO.Compression.ZipFile`) and, if any contain a backslash, a
//!   warning is printed naming the tool limitation — the archive is still
//!   produced (there's no reliable way to *rewrite* entry names inside a
//!   zip's central directory from `PowerShell` without re-creating the whole
//!   archive), but the operator is told exactly why and how to fix it
//!   (install/repair `tar.exe`).
//! - **Unix**: `zip -r`, falling back to `python3 -m zipfile` / `python -m
//!   zipfile` when `zip` isn't on `PATH`. Both already emit forward-slash
//!   entries, so this path is unchanged.
//!
//! If none of these are available the error names all of them, so the fix
//! is obvious.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};
use luabox_resolve::manifest::ALLOWED_BUNDLE_MODES;
use luabox_syntax::Dialect;

/// Fails with a cargo-style message listing the valid modes unless `mode`
/// is one of [`ALLOWED_BUNDLE_MODES`]. Called on a `--mode` value as soon
/// as it's parsed, before any bundling work happens; manifest-sourced
/// modes are already validated by `Manifest::parse`.
pub fn validate(mode: &str) -> anyhow::Result<()> {
    if ALLOWED_BUNDLE_MODES.contains(&mode) {
        Ok(())
    } else {
        bail!(
            "unknown bundle mode `{mode}` (valid: {})",
            ALLOWED_BUNDLE_MODES.join(", ")
        );
    }
}

/// Emit LÖVE packaging: `<out_dir>/<name>.love`. `bundle_text` is the
/// already-produced plain bundle (see module docs for why it's written
/// unmodified as `main.lua`). Returns the written `.love` path.
pub fn emit_love(
    project_root: &Path,
    out_dir: &Path,
    name: &str,
    bundle_text: &str,
    edition: Dialect,
    target: Dialect,
) -> anyhow::Result<PathBuf> {
    let staging =
        tempfile::tempdir().context("cannot create a staging directory for `.love` packaging")?;
    let stage = staging.path();

    fs::write(stage.join("main.lua"), bundle_text)
        .context("cannot stage `main.lua` for `.love` packaging")?;

    let conf_src = project_root.join("src").join("conf.lua");
    if conf_src.is_file() {
        let request = luabox_bundle::BundleRequest {
            root: project_root,
            entry: &conf_src,
            edition,
            target,
            name: "conf.lua",
            minify: false,
            sourcemap: false,
        };
        let conf_bundle = luabox_bundle::bundle(&request).map_err(|e| {
            anyhow::anyhow!("cannot bundle `src/conf.lua` for `.love` packaging: {e}")
        })?;
        fs::write(stage.join("conf.lua"), conf_bundle.text)
            .context("cannot stage `conf.lua` for `.love` packaging")?;
    }

    let assets_src = project_root.join("assets");
    if assets_src.is_dir() {
        copy_dir_recursive(&assets_src, &stage.join("assets"))
            .context("cannot stage `assets/` for `.love` packaging")?;
    }

    fs::create_dir_all(out_dir)
        .with_context(|| format!("cannot create `{}`", out_dir.display()))?;
    let dest = out_dir.join(format!("{name}.love"));
    zip_directory(stage, &dest)?;
    Ok(dest)
}

/// Emit Neovim plugin layout: `<out_dir>/<name>/{lua/<name>/init.lua,
/// plugin/<name>.lua, doc/<name>.txt}`. See module docs for the
/// convention. Returns the `<out_dir>/<name>` directory.
pub fn emit_nvim_plugin(
    out_dir: &Path,
    name: &str,
    bundle_text: &str,
    description: Option<&str>,
) -> anyhow::Result<PathBuf> {
    let root = out_dir.join(name);

    let lua_dir = root.join("lua").join(name);
    fs::create_dir_all(&lua_dir)
        .with_context(|| format!("cannot create `{}`", lua_dir.display()))?;
    let init_path = lua_dir.join("init.lua");
    fs::write(&init_path, bundle_text)
        .with_context(|| format!("cannot write `{}`", init_path.display()))?;

    let plugin_dir = root.join("plugin");
    fs::create_dir_all(&plugin_dir)
        .with_context(|| format!("cannot create `{}`", plugin_dir.display()))?;
    let plugin_path = plugin_dir.join(format!("{name}.lua"));
    fs::write(&plugin_path, plugin_bootstrap_stub(name))
        .with_context(|| format!("cannot write `{}`", plugin_path.display()))?;

    let doc_dir = root.join("doc");
    fs::create_dir_all(&doc_dir)
        .with_context(|| format!("cannot create `{}`", doc_dir.display()))?;
    let doc_path = doc_dir.join(format!("{name}.txt"));
    fs::write(&doc_path, doc_stub(name, description))
        .with_context(|| format!("cannot write `{}`", doc_path.display()))?;

    Ok(root)
}

/// A two-line bootstrap that does nothing by default — Neovim plugins are
/// conventionally lazy-`require`d from user config, not eager-loaded at
/// startup (SPEC.md §7).
fn plugin_bootstrap_stub(name: &str) -> String {
    format!(
        "-- {name}: bootstrap stub, auto-sourced by Neovim on startup.\n\
         -- Does nothing by default — this plugin is meant to be lazy-required\n\
         -- (`require(\"{name}\")`) from user config, not eager-loaded here.\n"
    )
}

/// A minimal `:help`-format stub: just enough structure (`*tag*` header,
/// a bar of `=`, a heading) for `:helptags` to index without erroring.
fn doc_stub(name: &str, description: Option<&str>) -> String {
    let desc = description.unwrap_or("(no description)");
    format!(
        "*{name}.txt*  {desc}\n\n\
         ==============================================================================\n\
         {name}                                                                 *{name}*\n\n\
         {desc}\n\n\
         vim:tw=78:ts=8:noet:ft=help:norl:\n"
    )
}

/// Recursively copy `src` onto `dest`, creating directories as needed.
fn copy_dir_recursive(src: &Path, dest: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dest).with_context(|| format!("cannot create `{}`", dest.display()))?;
    for entry in fs::read_dir(src).with_context(|| format!("cannot read `{}`", src.display()))? {
        let entry =
            entry.with_context(|| format!("cannot read an entry of `{}`", src.display()))?;
        let path = entry.path();
        let target = dest.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else {
            fs::copy(&path, &target)
                .with_context(|| format!("cannot copy `{}`", path.display()))?;
        }
    }
    Ok(())
}

/// Zip every top-level entry of `stage` into `dest`, dispatching to
/// whichever tool is available (see module docs).
fn zip_directory(stage: &Path, dest: &Path) -> anyhow::Result<()> {
    let entries: Vec<String> = fs::read_dir(stage)
        .with_context(|| format!("cannot read staging directory `{}`", stage.display()))?
        .filter_map(std::result::Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    if entries.is_empty() {
        bail!("nothing to package into `{}`", dest.display());
    }

    if cfg!(windows) {
        if let Some(bsdtar) = system_bsdtar() {
            zip_with_bsdtar(&bsdtar, stage, dest, &entries)
        } else {
            zip_with_powershell(stage, dest, &entries)
        }
    } else if which("zip") {
        zip_with_zip(stage, dest, &entries)
    } else if which("python3") {
        zip_with_python("python3", stage, dest, &entries)
    } else if which("python") {
        zip_with_python("python", stage, dest, &entries)
    } else {
        bail!(
            "cannot create `{}`: no zip tool found (tried `zip`, `python3 -m zipfile`, \
             `python -m zipfile`) — install one of these to use `--mode love`",
            dest.display()
        )
    }
}

/// Best-effort "is this tool runnable" probe: spawns `<tool> --version` and
/// reports whether the process could even start (its exit code doesn't
/// matter — `python --version` and friends reliably exist if this spawns).
fn which(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().is_ok()
}

/// Escape a string for interpolation into a `PowerShell` single-quoted
/// literal (double any embedded `'`).
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Locate the Windows-bundled `bsdtar` at `%SystemRoot%\System32\tar.exe`
/// — the same trick `toolchain_cmd::tar_program` uses to make sure a
/// git-shipped GNU tar earlier on `PATH` (which cannot create zip archives
/// at all) doesn't shadow it. Returns `None` on a non-standard Windows
/// install missing it, in which case the caller falls back to
/// `Compress-Archive`.
fn system_bsdtar() -> Option<PathBuf> {
    let root = std::env::var("SystemRoot").ok()?;
    let candidate = Path::new(&root).join("System32").join("tar.exe");
    candidate.is_file().then_some(candidate)
}

/// Windows preferred path: the System32 `bsdtar` can create zip archives
/// directly and — unlike `Compress-Archive` — reliably writes ZIP-spec
/// forward-slash entry separators (see module docs). Run with `stage` as
/// the working directory so archive paths are relative (no leading
/// directory), matching `zip_with_zip`/`zip_with_python` on Unix.
/// `--format zip` is passed explicitly rather than relying on `-a`'s
/// extension-sniffing, which doesn't recognize `.love` and silently falls
/// back to a plain POSIX tar archive (verified empirically — ticket #75).
fn zip_with_bsdtar(
    bsdtar: &Path,
    stage: &Path,
    dest: &Path,
    entries: &[String],
) -> anyhow::Result<()> {
    let output = Command::new(bsdtar)
        .current_dir(stage)
        .args(["--format", "zip", "-cf"])
        .arg(dest)
        .args(entries)
        .output()
        .with_context(|| format!("failed to spawn `{}`", bsdtar.display()))?;
    if !output.status.success() {
        bail!(
            "`{} --format zip` failed:\n{}",
            bsdtar.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Windows fallback when `System32\tar.exe` is missing: `Compress-Archive`
/// via `powershell -Command`. Zips to a `.zip`-suffixed temp path first
/// (`Compress-Archive` has no opinion on the destination extension, but
/// staying on the safe, documented side) then renames onto `dest`,
/// whatever its extension. Warns (doesn't fail) if the result has
/// backslash entry paths — see module docs and
/// [`warn_if_backslash_entries`].
fn zip_with_powershell(stage: &Path, dest: &Path, entries: &[String]) -> anyhow::Result<()> {
    let paths = entries
        .iter()
        .map(|e| ps_quote(&stage.join(e).display().to_string()))
        .collect::<Vec<_>>()
        .join(",");
    let tmp_zip = dest.with_extension("zip-staging.zip");
    let script = format!(
        "Compress-Archive -Path {paths} -DestinationPath {} -Force",
        ps_quote(&tmp_zip.display().to_string())
    );
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .context("failed to spawn `powershell` to create the archive (`--mode love` needs it on Windows)")?;
    if !output.status.success() {
        bail!(
            "powershell Compress-Archive failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::rename(&tmp_zip, dest)
        .or_else(|_| fs::copy(&tmp_zip, dest).and_then(|_| fs::remove_file(&tmp_zip)))
        .with_context(|| {
            format!(
                "cannot move `{}` to `{}`",
                tmp_zip.display(),
                dest.display()
            )
        })?;
    warn_if_backslash_entries(dest);
    Ok(())
}

/// Best-effort, never-fails post-creation check for the `Compress-Archive`
/// fallback (ticket #75): lists `dest`'s entries via .NET
/// `System.IO.Compression.ZipFile` and, if any contain a backslash, prints
/// a warning naming the tool limitation. There's no reliable way to
/// *rewrite* entry names inside a zip's central directory from `PowerShell`
/// short of re-creating the whole archive, so this only warns — the
/// archive is still produced and returned to the caller.
fn warn_if_backslash_entries(dest: &Path) {
    let script = format!(
        "Add-Type -AssemblyName System.IO.Compression.FileSystem; \
         $zip = [System.IO.Compression.ZipFile]::OpenRead({}); \
         try {{ $zip.Entries | Where-Object {{ $_.FullName -match '\\\\' }} | \
         ForEach-Object {{ $_.FullName }} }} finally {{ $zip.Dispose() }}",
        ps_quote(&dest.display().to_string())
    );
    let Ok(output) = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
    else {
        return;
    };
    let bad = String::from_utf8_lossy(&output.stdout);
    let bad_entries: Vec<&str> = bad
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if !bad_entries.is_empty() {
        eprintln!(
            "warning: `{}` contains backslash-separated entry paths ({}) — this Windows \
             `Compress-Archive` wrote OS path separators instead of the ZIP-spec `/`, which can \
             break LÖVE's `love.filesystem` and other unzip tools on non-Windows platforms. \
             Install/repair the Windows `tar.exe` (bundled since Windows 10 1809, normally at \
             `%SystemRoot%\\System32\\tar.exe`) so luabox can use it instead of \
             `Compress-Archive` for `.love` packaging.",
            dest.display(),
            bad_entries.join(", ")
        );
    }
}

/// Unix: `zip -r <dest> <entries…>`, run with `stage` as the working
/// directory so archive paths are relative (no leading directory).
fn zip_with_zip(stage: &Path, dest: &Path, entries: &[String]) -> anyhow::Result<()> {
    let output = Command::new("zip")
        .current_dir(stage)
        .arg("-r")
        .arg(dest)
        .args(entries)
        .output()
        .context("failed to spawn `zip`")?;
    if !output.status.success() {
        bail!("`zip` failed:\n{}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(())
}

/// Unix fallback when `zip` isn't on `PATH`: `python[3] -m zipfile -c`,
/// which recurses into directories on its own.
fn zip_with_python(
    python: &str,
    stage: &Path,
    dest: &Path,
    entries: &[String],
) -> anyhow::Result<()> {
    let output = Command::new(python)
        .current_dir(stage)
        .args(["-m", "zipfile", "-c"])
        .arg(dest)
        .args(entries)
        .output()
        .with_context(|| format!("failed to spawn `{python}`"))?;
    if !output.status.success() {
        bail!(
            "`{python} -m zipfile` failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_all_allowed_modes() {
        for mode in ALLOWED_BUNDLE_MODES {
            validate(mode).unwrap_or_else(|e| panic!("`{mode}` should validate: {e}"));
        }
    }

    #[test]
    fn validate_rejects_unknown_mode_listing_valid_ones() {
        let error = validate("roblox").unwrap_err().to_string();
        assert!(error.contains("roblox"));
        for mode in ALLOWED_BUNDLE_MODES {
            assert!(error.contains(mode), "{error} should mention {mode}");
        }
    }

    #[test]
    fn nvim_plugin_layout_preserves_bundle_text_verbatim() {
        let out = tempfile::tempdir().expect("tempdir");
        let bundle_text = "local M = {}\nfunction M.hi() return \"hi\" end\nreturn M\n";
        let root = emit_nvim_plugin(out.path(), "mypkg", bundle_text, Some("a test plugin"))
            .expect("emit_nvim_plugin succeeds");

        assert_eq!(root, out.path().join("mypkg"));
        let init = fs::read_to_string(root.join("lua").join("mypkg").join("init.lua"))
            .expect("init.lua written");
        // The entry chunk's own `return` survives unmodified — a Neovim
        // `require("mypkg")` sees exactly what the bundle would return.
        assert_eq!(init, bundle_text);

        let plugin = fs::read_to_string(root.join("plugin").join("mypkg.lua"))
            .expect("plugin/mypkg.lua written");
        assert!(plugin.contains("mypkg"));
        assert!(plugin.contains("lazy"));

        let doc =
            fs::read_to_string(root.join("doc").join("mypkg.txt")).expect("doc/mypkg.txt written");
        assert!(doc.contains("mypkg"));
        assert!(doc.contains("a test plugin"));
    }

    #[test]
    fn nvim_plugin_doc_stub_has_placeholder_without_description() {
        let out = tempfile::tempdir().expect("tempdir");
        let root = emit_nvim_plugin(out.path(), "mypkg", "return {}\n", None)
            .expect("emit_nvim_plugin succeeds");
        let doc = fs::read_to_string(root.join("doc").join("mypkg.txt")).expect("doc written");
        assert!(doc.contains("no description"));
    }

    /// Regression test for ticket #75: on Windows, `.love` archives must
    /// use ZIP-spec forward-slash entry separators for nested paths, not
    /// the OS separator. `zip_directory` should prefer the System32
    /// `bsdtar` path (`zip_with_bsdtar`), which writes `/` regardless of
    /// platform; this asserts against the listing of the archive it
    /// actually produces, so it fails if the dispatch in `zip_directory`
    /// ever silently falls back to a separator-unsafe tool.
    #[cfg(windows)]
    #[test]
    fn love_archive_entries_use_forward_slash_separators_on_windows() {
        let bsdtar = system_bsdtar()
            .expect("System32\\tar.exe should be present on any supported Windows version");

        let staging = tempfile::tempdir().expect("tempdir");
        let stage = staging.path();
        fs::write(stage.join("main.lua"), "return 1\n").expect("write main.lua");
        fs::create_dir_all(stage.join("assets").join("sub")).expect("mkdir nested assets");
        fs::write(stage.join("assets").join("README.txt"), b"hi").expect("write asset");
        fs::write(stage.join("assets").join("sub").join("deep.txt"), b"deep")
            .expect("write nested asset");

        let out = tempfile::tempdir().expect("tempdir");
        let dest = out.path().join("test.love");
        zip_directory(stage, &dest).expect("zip_directory succeeds");

        let output = Command::new(&bsdtar)
            .arg("-tf")
            .arg(&dest)
            .output()
            .expect("bsdtar -tf should run against the archive we just created");
        assert!(
            output.status.success(),
            "bsdtar -tf failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let listing = String::from_utf8_lossy(&output.stdout);

        assert!(
            listing.contains("assets/README.txt"),
            "expected a forward-slash nested entry `assets/README.txt`, got:\n{listing}"
        );
        assert!(
            listing.contains("assets/sub/deep.txt"),
            "expected a forward-slash doubly-nested entry `assets/sub/deep.txt`, got:\n{listing}"
        );
        assert!(
            !listing.contains('\\'),
            "no entry path should contain a backslash separator:\n{listing}"
        );
    }

    #[test]
    fn copy_dir_recursive_preserves_nested_structure() {
        let src = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(src.path().join("sub")).expect("mkdir");
        fs::write(src.path().join("top.png"), b"top").expect("write");
        fs::write(src.path().join("sub").join("nested.png"), b"nested").expect("write");

        let dest = tempfile::tempdir().expect("tempdir");
        let target = dest.path().join("assets");
        copy_dir_recursive(src.path(), &target).expect("copy succeeds");

        assert_eq!(fs::read(target.join("top.png")).expect("top.png"), b"top");
        assert_eq!(
            fs::read(target.join("sub").join("nested.png")).expect("nested.png"),
            b"nested"
        );
    }
}
