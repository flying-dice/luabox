//! Integration tests for [`GitProvider`] (SPEC.md §6, #21): real `git`
//! repositories created in temp dirs — no network. Each test skips
//! gracefully (with a note) when `git` is not on `PATH`.

// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]

use std::path::{Path, PathBuf};
use std::process::Command;

use luabox_resolve::{
    GitProvider, GitReference, LockedSource, Manifest, PackageId, PackageProvider, resolve,
};
use semver::Version;

/// Whether the `git` CLI is available in this environment.
fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Runs one git command in `dir`, panicking (with stderr) on failure.
fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("git spawns");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn commit_all(dir: &Path, message: &str) -> String {
    git(dir, &["add", "."]);
    git(
        dir,
        &[
            "-c",
            "user.name=luabox-ci",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            message,
        ],
    );
    git(dir, &["rev-parse", "HEAD"])
}

/// Creates a repo whose tree is a luabox package, committed and tagged.
/// Returns `(repo_path, commit_sha)`.
fn package_repo(root: &Path, name: &str, version: &str, tag: Option<&str>) -> (PathBuf, String) {
    let dir = root.join(name);
    std::fs::create_dir_all(dir.join("src")).expect("create repo dirs");
    std::fs::write(
        dir.join("luabox.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"5.4\"\n"),
    )
    .expect("write manifest");
    std::fs::write(
        dir.join("src").join("init.lua"),
        format!("return \"{name} {version}\"\n"),
    )
    .expect("write source");
    git(&dir, &["init", "--quiet"]);
    let sha = commit_all(&dir, "init");
    if let Some(tag) = tag {
        git(&dir, &["tag", tag]);
    }
    (dir, sha)
}

/// The repo path as an opaque git URL (plain local path — the local
/// transport; hermetic, no network).
fn url_of(dir: &Path) -> String {
    dir.to_string_lossy().replace('\\', "/")
}

#[test]
fn fetches_tag_and_pins_the_resolved_commit() {
    if !git_available() {
        eprintln!("skipping: git is not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    let (repo, sha) = package_repo(tmp.path(), "gitlib", "1.2.0", Some("v1.2.0"));
    let url = url_of(&repo);

    let provider = GitProvider::new(tmp.path().join("cache"));
    let reference = GitReference::Tag("v1.2.0".to_owned());
    let id = PackageId::git("gitlib", &url, reference.clone());

    let versions = provider.list_versions(&id).expect("lists versions");
    assert_eq!(versions, vec![Version::new(1, 2, 0)]);

    let meta = provider.metadata(&id, &versions[0]).expect("metadata");
    assert_eq!(meta.pinned.as_deref(), Some(sha.as_str()), "sha pinned");

    // The exported checkout is store-ready: real files, no `.git`.
    let checkout = provider.checkout(&url, &reference).expect("checkout");
    assert_eq!(checkout.commit, sha);
    assert!(checkout.dir.join("src").join("init.lua").is_file());
    assert!(!checkout.dir.join(".git").exists(), ".git is stripped");
}

#[test]
fn rev_reference_checks_out_the_exact_commit() {
    if !git_available() {
        eprintln!("skipping: git is not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    let (repo, first_sha) = package_repo(tmp.path(), "gitlib", "1.0.0", None);

    // Advance the branch past the commit we will pin.
    std::fs::write(
        repo.join("luabox.toml"),
        "[package]\nname = \"gitlib\"\nversion = \"2.0.0\"\nedition = \"5.4\"\n",
    )
    .expect("bump manifest");
    let second_sha = commit_all(&repo, "bump to 2.0.0");
    assert_ne!(first_sha, second_sha);

    let provider = GitProvider::new(tmp.path().join("cache"));
    let id = PackageId::git(
        "gitlib",
        url_of(&repo),
        GitReference::Rev(first_sha.clone()),
    );
    let versions = provider.list_versions(&id).expect("lists versions");
    assert_eq!(
        versions,
        vec![Version::new(1, 0, 0)],
        "rev pin sees the old tree, not the branch head"
    );
    let meta = provider.metadata(&id, &versions[0]).expect("metadata");
    assert_eq!(meta.pinned.as_deref(), Some(first_sha.as_str()));
}

#[test]
fn cache_reuse_and_refresh_semantics() {
    if !git_available() {
        eprintln!("skipping: git is not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    let (repo, first_sha) = package_repo(tmp.path(), "gitlib", "1.0.0", None);
    let url = url_of(&repo);
    let cache = tmp.path().join("cache");
    let reference = GitReference::DefaultBranch;

    // Prime the cache at the first commit.
    let checkout = GitProvider::new(&cache)
        .checkout(&url, &reference)
        .expect("first fetch");
    assert_eq!(checkout.commit, first_sha);

    // The branch moves…
    std::fs::write(repo.join("extra.lua"), "return 2\n").expect("write");
    let second_sha = commit_all(&repo, "second");

    // …a plain install (fresh provider, same cache) still reuses the
    // cached checkout: deterministic, offline-friendly.
    let cached = GitProvider::new(&cache)
        .checkout(&url, &reference)
        .expect("cached fetch");
    assert_eq!(cached.commit, first_sha, "no refresh → cache wins");

    // `luabox update` refreshes mutable refs and sees the new commit.
    let refreshed = GitProvider::new(&cache)
        .with_refresh(true)
        .checkout(&url, &reference)
        .expect("refreshed fetch");
    assert_eq!(refreshed.commit, second_sha);
    assert!(refreshed.dir.join("extra.lua").is_file());
}

#[test]
fn resolve_records_git_source_with_pinned_sha_in_lockfile() {
    if !git_available() {
        eprintln!("skipping: git is not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    let (repo, sha) = package_repo(tmp.path(), "gitlib", "1.2.0", Some("v1.2.0"));
    let url = url_of(&repo);

    let manifest = Manifest::parse(&format!(
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n[dependencies]\ngitlib = {{ git = \"{url}\", tag = \"v1.2.0\" }}\n"
    ))
    .expect("valid manifest");

    let provider = GitProvider::new(tmp.path().join("cache"));
    let resolution = resolve(&manifest, tmp.path(), &provider, None).expect("resolves");
    let entry = resolution.lockfile.get("gitlib").expect("gitlib locked");
    let Some(LockedSource::Git { spec }) = &entry.source else {
        panic!("expected a git source, got {:?}", entry.source);
    };
    assert_eq!(
        spec,
        &format!("{url}#{sha}"),
        "lock pins the sha, not the tag"
    );
}

#[test]
fn unknown_reference_is_a_clear_error() {
    if !git_available() {
        eprintln!("skipping: git is not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    let (repo, _) = package_repo(tmp.path(), "gitlib", "1.0.0", None);

    let provider = GitProvider::new(tmp.path().join("cache"));
    let err = provider
        .checkout(&url_of(&repo), &GitReference::Tag("v9.9.9".to_owned()))
        .expect_err("unknown tag fails");
    assert!(
        err.to_string().contains("git"),
        "error names the git failure: {err}"
    );
}
