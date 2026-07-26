//! `luabox upgrade [VERSION]` — replace the running binary with a GitHub release.
//!
//! Mirrors the `scripts/install.{sh,ps1}` contract: the same asset names
//! (`luabox-<target>.{tar.gz,zip}`), the same `SHA256SUMS` integrity check, and
//! the same set of prebuilt targets. With no argument it installs the latest
//! release; given `0.1.0` or `v0.1.0` it installs that exact tag.
//!
//! ## House divergences from the shared toolchain conventions
//!
//! No HTTP crate is linked (SPEC.md §6): downloads shell out to `curl -fsSL`
//! exactly as the install scripts do. The
//! release archive is unpacked with `tar` — `tar -xzf` for the unix `.tar.gz`
//! and `tar -xf` for the Windows `.zip` (the bundled `bsdtar` in `System32`
//! reads zip natively) — so no `zip`/`flate2` crate is pulled in. Only the
//! `SHA256SUMS` verification runs in-process, via `sha2`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

/// The release repository, matching the install scripts.
const REPO: &str = "flying-dice/luabox";

/// `luabox upgrade`: resolve the target tag, then download, verify, and
/// self-replace the running executable.
pub fn run(version: Option<String>) -> Result<()> {
    let explicit = version.is_some();
    let target = current_target()?;
    let tag = match version {
        Some(v) => normalize_tag(&v),
        None => fetch_latest_tag()?,
    };

    let current = env!("CARGO_PKG_VERSION");
    if !explicit && tag.trim_start_matches('v') == current {
        println!("luabox is already up to date (v{current}).");
        return Ok(());
    }

    println!("Upgrading luabox v{current} -> {tag} ({target})...");

    let tmp = tempfile::tempdir().context("creating a temp dir for the download")?;
    install(&tag, target, tmp.path())?;

    println!("luabox upgraded to {tag}.");
    Ok(())
}

/// Downloads the release archive into `tmp`, verifies it against `SHA256SUMS`,
/// extracts the binary, and replaces the running executable.
fn install(tag: &str, target: &str, tmp: &Path) -> Result<()> {
    let asset = format!("luabox-{target}.{}", archive_ext());
    let base = format!("https://github.com/{REPO}/releases/download/{tag}");
    let archive = tmp.join(&asset);

    println!("  downloading {asset} ...");
    download(&format!("{base}/{asset}"), &archive)
        .with_context(|| format!("downloading {asset} — does release {tag} exist?"))?;

    println!("  verifying checksum ...");
    let sums = http_text(&format!("{base}/SHA256SUMS")).context("downloading SHA256SUMS")?;
    verify(&archive, &asset, &sums)?;

    let bin = extract(&archive, tmp)?;
    #[cfg(not(windows))]
    set_executable(&bin)?;

    self_replace::self_replace(&bin).context("replacing the running luabox binary")?;
    Ok(())
}

/// The target triple of the running binary, restricted to targets with a
/// prebuilt release asset (see `scripts/install.sh`).
fn current_target() -> Result<&'static str> {
    let target = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        (os, arch) => bail!(
            "no prebuilt binary for {arch} {os}; \
             build from source: cargo install --git https://github.com/{REPO} luabox-cli"
        ),
    };
    Ok(target)
}

/// The release archive extension for the running platform.
const fn archive_ext() -> &'static str {
    if cfg!(windows) { "zip" } else { "tar.gz" }
}

/// The binary's name inside the release archive.
const fn bin_in_archive() -> &'static str {
    if cfg!(windows) {
        "luabox.exe"
    } else {
        "luabox"
    }
}

/// Normalizes a user-supplied version to a release tag: `0.1.0` and `v0.1.0`
/// both yield `v0.1.0`.
fn normalize_tag(version: &str) -> String {
    format!("v{}", version.trim().trim_start_matches('v'))
}

/// Resolves the `latest` release tag via the GitHub API, scanning `tag_name`
/// out of the JSON the same way `scripts/install.sh` does (no `serde_json` on
/// the production path).
fn fetch_latest_tag() -> Result<String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = http_text(&url).context("reading release API response")?;
    parse_tag_name(&body)
        .context("release API response had no tag_name — are there any releases yet?")
}

/// Extracts the `"tag_name"` string value from a GitHub release API response.
///
/// A minimal scan mirroring the install scripts: find the key, take the text
/// after its colon, and read the first double-quoted segment. Uses `split`
/// rather than byte-index slicing so it never panics on a UTF-8 boundary.
fn parse_tag_name(json: &str) -> Option<String> {
    json.split("\"tag_name\"")
        .nth(1)?
        .split_once(':')?
        .1
        .split('"')
        .nth(1)
        .map(str::to_owned)
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

/// `curl -fsSL <url>` capturing stdout as text (following redirects to the
/// asset's storage backend). curl always sends a `User-Agent`, which the GitHub
/// API requires.
fn http_text(url: &str) -> Result<String> {
    let output = Command::new("curl")
        .args(["-fsSL", url])
        .output()
        .with_context(|| format!("running `curl` for {url}"))?;
    if !output.status.success() {
        bail!(
            "`curl` failed for {url} (exit {}): {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).with_context(|| format!("{url} returned non-UTF-8 text"))
}

/// Downloads `url` to `dest` with `curl -fsSL <url> -o <dest>`.
fn download(url: &str, dest: &Path) -> Result<()> {
    let status = Command::new("curl")
        .args(["-fsSL", url, "-o"])
        .arg(dest)
        .status()
        .with_context(|| format!("running `curl` for {url}"))?;
    if !status.success() {
        bail!(
            "`curl` failed for {url} (exit {})",
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

/// Verifies `archive` against the `name` entry in `SHA256SUMS`, failing closed
/// if the entry is missing or the digest differs.
fn verify(archive: &Path, name: &str, sums: &str) -> Result<()> {
    let expected =
        expected_hash(sums, name).with_context(|| format!("no checksum listed for {name}"))?;

    let bytes = std::fs::read(archive).context("reading archive for checksum")?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let actual_hex = to_hex(&hasher.finalize());

    if !actual_hex.eq_ignore_ascii_case(expected) {
        bail!("checksum mismatch for {name}\n  expected: {expected}\n  actual:   {actual_hex}");
    }
    Ok(())
}

/// Lowercase hex encoding of `bytes`. `write!` into a `String` is infallible,
/// so the `Result` is discarded rather than unwrapped (the restriction lints
/// forbid `expect` on production paths).
fn to_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// The expected hex digest for `name` in a `sha256sum`-format listing
/// (`<hash>  <filename>` per line), or `None` if absent.
fn expected_hash<'a>(sums: &'a str, name: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let hash = fields.next()?;
        let file = fields.next()?;
        (file == name).then_some(hash)
    })
}

/// Unpacks `archive` into `dest_dir` with `tar` (`-xzf` for the unix `.tar.gz`,
/// `-xf` for the Windows `.zip`, which `bsdtar` reads natively), then returns
/// the path to the extracted binary at the archive root.
fn extract(archive: &Path, dest_dir: &Path) -> Result<PathBuf> {
    let tar = tar_program();
    let flag = if cfg!(windows) { "-xf" } else { "-xzf" };
    let status = Command::new(&tar)
        .arg(flag)
        .arg(archive)
        .arg("-C")
        .arg(dest_dir)
        .status()
        .with_context(|| {
            format!(
                "failed to run `{}` — archive extraction needs `tar` on PATH",
                tar.display()
            )
        })?;
    if !status.success() {
        bail!(
            "`tar {flag}` failed to unpack `{}` (exit {})",
            archive.display(),
            status.code().unwrap_or(-1)
        );
    }

    let bin = dest_dir.join(bin_in_archive());
    if !bin.is_file() {
        bail!("`{}` not found in archive", bin_in_archive());
    }
    Ok(bin)
}

/// Marks `path` executable (`0o755`).
#[cfg(not(windows))]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut perms = std::fs::metadata(path)
        .context("stat extracted binary")?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).context("chmod extracted binary")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::{
        archive_ext, bin_in_archive, current_target, download, expected_hash, extract, http_text,
        normalize_tag, parse_tag_name, tar_program, to_hex, verify,
    };
    use sha2::{Digest, Sha256};

    #[test]
    fn normalizes_versions_to_v_prefixed_tags() {
        assert_eq!(normalize_tag("0.1.0"), "v0.1.0");
        assert_eq!(normalize_tag("v0.1.0"), "v0.1.0");
        assert_eq!(normalize_tag("  1.2.3  "), "v1.2.3");
    }

    #[test]
    fn finds_the_matching_checksum_line() {
        let sums = "\
aaaa  luabox-aarch64-apple-darwin.tar.gz
bbbb  luabox-x86_64-unknown-linux-gnu.tar.gz
";
        assert_eq!(
            expected_hash(sums, "luabox-x86_64-unknown-linux-gnu.tar.gz"),
            Some("bbbb")
        );
    }

    #[test]
    fn missing_checksum_entry_is_none() {
        let sums = "aaaa  luabox-aarch64-apple-darwin.tar.gz\n";
        assert_eq!(
            expected_hash(sums, "luabox-x86_64-pc-windows-msvc.zip"),
            None
        );
    }

    #[test]
    fn parses_tag_name_from_release_json() {
        let json = r#"{"url":"...","tag_name":"v0.1.0","name":"luabox 0.1.0"}"#;
        assert_eq!(parse_tag_name(json).as_deref(), Some("v0.1.0"));
    }

    #[test]
    fn parses_tag_name_with_whitespace() {
        let json = "{\n  \"tag_name\": \"v1.2.3\",\n  \"draft\": false\n}";
        assert_eq!(parse_tag_name(json).as_deref(), Some("v1.2.3"));
    }

    #[test]
    fn missing_tag_name_is_none() {
        assert_eq!(parse_tag_name(r#"{"message":"Not Found"}"#), None);
    }

    #[test]
    fn asset_names_match_the_install_scripts() {
        // unix builds ship .tar.gz + `luabox`; windows .zip + `luabox.exe`.
        if cfg!(windows) {
            assert_eq!(archive_ext(), "zip");
            assert_eq!(bin_in_archive(), "luabox.exe");
        } else {
            assert_eq!(archive_ext(), "tar.gz");
            assert_eq!(bin_in_archive(), "luabox");
        }
    }

    #[test]
    fn verify_accepts_a_matching_digest() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("luabox-x86_64-pc-windows-msvc.zip");
        let contents = b"the release archive bytes";
        std::fs::write(&archive, contents).unwrap();

        let digest = to_hex(&Sha256::digest(contents));
        let sums = format!("{digest}  luabox-x86_64-pc-windows-msvc.zip\n");

        verify(&archive, "luabox-x86_64-pc-windows-msvc.zip", &sums).unwrap();
    }

    #[test]
    fn verify_rejects_a_tampered_archive() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("luabox-x86_64-pc-windows-msvc.zip");
        std::fs::write(&archive, b"tampered bytes").unwrap();

        let sums = format!("{}  luabox-x86_64-pc-windows-msvc.zip\n", "0".repeat(64));
        let err = verify(&archive, "luabox-x86_64-pc-windows-msvc.zip", &sums).unwrap_err();
        assert!(err.to_string().contains("checksum mismatch"), "{err}");
    }

    #[test]
    fn verify_fails_closed_when_the_entry_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("luabox-x86_64-pc-windows-msvc.zip");
        std::fs::write(&archive, b"bytes").unwrap();

        let err = verify(&archive, "luabox-x86_64-pc-windows-msvc.zip", "").unwrap_err();
        assert!(err.to_string().contains("no checksum listed"), "{err}");
    }

    // -----------------------------------------------------------------
    // Offline coverage of the local halves of the upgrade pipeline. The
    // live download path (`run`/`install`/`fetch_latest_tag`) needs the
    // GitHub API and release assets, so it is deliberately left to manual
    // release verification; everything reachable without the network —
    // target selection, tar dispatch, archive extraction, permissions —
    // is exercised here against locally built fixtures.
    // -----------------------------------------------------------------

    /// Build a release-shaped archive in `dir` containing the given
    /// top-level entries, using the same `tar` the extractor will use
    /// (`.tar.gz` on unix, `.zip` via `bsdtar` on Windows).
    fn make_archive(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
        let stage = dir.join("stage");
        std::fs::create_dir_all(&stage).unwrap();
        for (name, contents) in entries {
            std::fs::write(stage.join(name), contents).unwrap();
        }

        let archive = dir.join(format!("luabox-fixture.{}", archive_ext()));
        let tar = tar_program();
        let mut command = Command::new(&tar);
        command.current_dir(&stage);
        if cfg!(windows) {
            command.args(["--format", "zip", "-cf"]);
        } else {
            command.arg("-czf");
        }
        command.arg(&archive);
        for (name, _) in entries {
            command.arg(name);
        }
        let status = command
            .status()
            .unwrap_or_else(|e| panic!("running `{}`: {e}", tar.display()));
        assert!(status.success(), "packing the fixture archive failed");
        archive
    }

    #[test]
    fn the_running_platform_maps_to_a_prebuilt_target_or_says_why_not() {
        let expected = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
            ("macos", "aarch64") => Some("aarch64-apple-darwin"),
            ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
            _ => None,
        };
        if let Some(triple) = expected {
            assert_eq!(current_target().unwrap(), triple);
        } else {
            // An unsupported platform must say so and point at a source
            // build rather than downloading something wrong.
            let err = current_target().unwrap_err().to_string();
            assert!(err.contains("no prebuilt binary"), "{err}");
            assert!(err.contains("cargo install"), "{err}");
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn the_extractor_uses_plain_tar_off_windows() {
        assert_eq!(tar_program(), PathBuf::from("tar"));
    }

    #[test]
    fn extract_unpacks_the_archive_and_returns_the_binary_at_its_root() {
        let dir = tempfile::tempdir().unwrap();
        let archive = make_archive(
            dir.path(),
            &[(bin_in_archive(), b"#!/bin/sh\necho luabox\n")],
        );

        let dest = dir.path().join("unpacked");
        std::fs::create_dir_all(&dest).unwrap();
        let bin = extract(&archive, &dest).unwrap();

        assert_eq!(bin, dest.join(bin_in_archive()));
        assert_eq!(
            std::fs::read(&bin).unwrap(),
            b"#!/bin/sh\necho luabox\n".to_vec()
        );
    }

    #[test]
    fn extract_fails_when_the_archive_has_no_luabox_binary_at_its_root() {
        let dir = tempfile::tempdir().unwrap();
        let archive = make_archive(dir.path(), &[("README.md", b"not the binary")]);

        let dest = dir.path().join("unpacked");
        std::fs::create_dir_all(&dest).unwrap();
        let err = extract(&archive, &dest).unwrap_err().to_string();
        assert!(err.contains("not found in archive"), "{err}");
        assert!(err.contains(bin_in_archive()), "{err}");
    }

    #[test]
    fn extract_reports_a_corrupt_archive_rather_than_a_missing_binary() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join(format!("luabox-bad.{}", archive_ext()));
        std::fs::write(&archive, b"this is not an archive at all").unwrap();

        let dest = dir.path().join("unpacked");
        std::fs::create_dir_all(&dest).unwrap();
        let err = extract(&archive, &dest).unwrap_err().to_string();
        assert!(err.contains("failed to unpack"), "{err}");
    }

    #[cfg(not(windows))]
    #[test]
    fn the_extracted_binary_is_marked_executable() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("luabox");
        std::fs::write(&bin, b"binary").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o600)).unwrap();

        super::set_executable(&bin).unwrap();
        let mode = std::fs::metadata(&bin).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    /// The transport is `curl` (SPEC.md §6: no HTTP crate is linked). These
    /// tests drive it over `file://` URLs, so they exercise the real code
    /// path with no network at all — skipped where `curl` is unavailable,
    /// since it is a runtime dependency of the command, not of the crate.
    fn curl_available() -> bool {
        Command::new("curl").arg("--version").output().is_ok()
    }

    #[test]
    fn http_text_returns_the_body_of_a_successful_fetch() {
        if !curl_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let body = dir.path().join("releases.json");
        std::fs::write(&body, r#"{"tag_name":"v9.9.9"}"#).unwrap();

        let text = http_text(&format!("file://{}", body.display())).unwrap();
        assert_eq!(parse_tag_name(&text).as_deref(), Some("v9.9.9"));
    }

    #[test]
    fn http_text_fails_loudly_when_the_resource_is_missing() {
        if !curl_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.json");
        let err = http_text(&format!("file://{}", missing.display()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("`curl` failed for"), "{err}");
    }

    #[test]
    fn download_writes_the_fetched_bytes_to_the_destination() {
        if !curl_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("asset.bin");
        std::fs::write(&source, b"release bytes").unwrap();

        let dest = dir.path().join("downloaded.bin");
        download(&format!("file://{}", source.display()), &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"release bytes".to_vec());
    }

    #[test]
    fn download_fails_loudly_when_the_asset_is_missing() {
        if !curl_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let err = download(
            &format!("file://{}", dir.path().join("nope.bin").display()),
            &dir.path().join("out.bin"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("`curl` failed for"), "{err}");
    }

    #[test]
    fn a_downloaded_archive_verifies_against_a_sha256sums_listing() {
        // The download → verify handshake end to end, offline: build an
        // archive, hash it the way the release job does, and check that
        // `verify` accepts exactly that listing.
        let dir = tempfile::tempdir().unwrap();
        let archive = make_archive(dir.path(), &[(bin_in_archive(), b"binary")]);
        let name = archive
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap()
            .to_owned();

        let digest = to_hex(&Sha256::digest(std::fs::read(&archive).unwrap()));
        let sums = format!("0000  some-other-asset.tar.gz\n{digest}  {name}\n");
        verify(&archive, &name, &sums).unwrap();
    }

    #[test]
    fn to_hex_pads_every_byte_to_two_lowercase_digits() {
        assert_eq!(to_hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
        assert_eq!(to_hex(&[]), "");
    }

    #[test]
    fn a_checksum_listing_with_blank_and_short_lines_is_tolerated() {
        let sums = "\n\njustonefield\nabc123  luabox-x86_64-unknown-linux-gnu.tar.gz\n";
        assert_eq!(
            expected_hash(sums, "luabox-x86_64-unknown-linux-gnu.tar.gz"),
            Some("abc123")
        );
    }

    #[test]
    fn a_checksum_is_matched_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("asset.tar.gz");
        std::fs::write(&archive, b"bytes").unwrap();
        let sums = format!(
            "{}  asset.tar.gz\n",
            to_hex(&Sha256::digest(b"bytes")).to_uppercase()
        );
        verify(&archive, "asset.tar.gz", &sums).unwrap();
    }

    #[test]
    fn parse_tag_name_ignores_a_tag_name_shaped_value_before_the_key() {
        // The scan looks for the key, then the first quoted run after the
        // colon — a value mentioning the key earlier must not confuse it.
        let json = r#"{"body":"see tag_name notes","tag_name":"v2.0.0"}"#;
        assert_eq!(parse_tag_name(json).as_deref(), Some("v2.0.0"));
    }

    #[test]
    fn normalize_tag_leaves_a_non_semver_tag_alone_apart_from_the_prefix() {
        assert_eq!(normalize_tag("nightly"), "vnightly");
        // Every leading `v` is stripped before the single prefix is added.
        assert_eq!(normalize_tag("vv1.0.0"), "v1.0.0");
    }
}
