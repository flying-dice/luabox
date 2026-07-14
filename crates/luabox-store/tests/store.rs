//! Integration tests for the content-addressed store.
//!
//! Everything lives under a single `TempDir` so the store, the source tree, and
//! the materialization target share one volume — a precondition for hard links
//! on NTFS (and the platform tests exercise that path directly).

// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use luabox_store::{CorruptKind, GcOptions, LinkMode, Store, TreeManifest};
use tempfile::TempDir;

/// Write `contents` to `dir/rel`, creating parent directories.
fn write_file(dir: &Path, rel: &str, contents: &[u8]) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

/// Recursively read a tree into a `rel-path -> bytes` map for comparison.
fn read_tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(rel, fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// Recompute an object's on-disk path from its hash (mirrors the store layout).
fn object_path(store_root: &Path, hash: &str) -> PathBuf {
    let (prefix, rest) = hash.split_at(2);
    store_root
        .join("objects")
        .join("sha256")
        .join(prefix)
        .join(rest)
}

/// Force a store object writable so a test can corrupt or delete it.
#[allow(clippy::permissions_set_readonly_false)]
fn make_writable(path: &Path) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_readonly(false);
    fs::set_permissions(path, perms).unwrap();
}

#[test]
fn put_then_materialize_round_trips_including_nested_dirs() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    write_file(&src, "init.lua", b"return 1\n");
    write_file(&src, "lib/util.lua", b"local M = {}\nreturn M\n");
    write_file(&src, "lib/deep/nested.lua", b"-- deep\n");

    let store = Store::open(tmp.path().join("store"));
    let manifest = store.put_tree(&src).unwrap();
    assert_eq!(manifest.entries.len(), 3);

    let dest = tmp.path().join("out");
    let report = store.materialize(&manifest, &dest, LinkMode::Auto).unwrap();
    assert_eq!(report.total(), 3);

    assert_eq!(read_tree(&src), read_tree(&dest));
}

#[test]
fn manifest_ordering_is_deterministic() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    write_file(&src, "z.lua", b"z");
    write_file(&src, "a.lua", b"a");
    write_file(&src, "m/b.lua", b"b");

    let store = Store::open(tmp.path().join("store"));
    let a = store.put_tree(&src).unwrap();
    let b = store.put_tree(&src).unwrap();

    assert_eq!(a.tree_hash, b.tree_hash);
    let paths: Vec<_> = a.entries.iter().map(|e| e.path.as_str()).collect();
    let mut sorted = paths.clone();
    sorted.sort_unstable();
    assert_eq!(paths, sorted);
}

#[test]
fn shared_files_across_versions_are_deduplicated() {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(tmp.path().join("store"));

    // v1 and v2 share init.lua (identical) but differ in changed.lua.
    let v1 = tmp.path().join("v1");
    write_file(&v1, "init.lua", b"shared\n");
    write_file(&v1, "changed.lua", b"version one\n");

    let v2 = tmp.path().join("v2");
    write_file(&v2, "init.lua", b"shared\n");
    write_file(&v2, "changed.lua", b"version two\n");

    let m1 = store.put_tree(&v1).unwrap();
    let m2 = store.put_tree(&v2).unwrap();

    // Three distinct objects: shared init, changed-v1, changed-v2 — not four.
    assert_eq!(store.stats().unwrap().objects, 3);

    // The shared object is genuinely the same address in both manifests.
    let shared1 = &m1
        .entries
        .iter()
        .find(|e| e.path == "init.lua")
        .unwrap()
        .hash;
    let shared2 = &m2
        .entries
        .iter()
        .find(|e| e.path == "init.lua")
        .unwrap()
        .hash;
    assert_eq!(shared1, shared2);
    assert!(store.has(shared1));
}

#[test]
fn hardlink_materialization_links_without_copy_fallback() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    write_file(&src, "a.lua", b"aaaa");
    write_file(&src, "b/c.lua", b"cccc");

    // Store and dest share the temp-dir volume, so hard links must succeed.
    let store = Store::open(tmp.path().join("store"));
    let manifest = store.put_tree(&src).unwrap();

    let dest = tmp.path().join("linked");
    let report = store
        .materialize(&manifest, &dest, LinkMode::HardLink)
        .unwrap();

    // Proof of linking: every file linked, nothing fell back to a copy.
    assert_eq!(report.hard_linked, 2);
    assert_eq!(report.copied, 0);
    assert_eq!(read_tree(&src), read_tree(&dest));

    // A hard link shares the object's inode and therefore its read-only bit.
    let linked = fs::metadata(dest.join("a.lua")).unwrap();
    assert!(linked.permissions().readonly());
}

#[test]
fn copy_mode_produces_writable_files() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    write_file(&src, "a.lua", b"aaaa");

    let store = Store::open(tmp.path().join("store"));
    let manifest = store.put_tree(&src).unwrap();

    let dest = tmp.path().join("copied");
    let report = store.materialize(&manifest, &dest, LinkMode::Copy).unwrap();
    assert_eq!(report.copied, 1);
    assert_eq!(report.hard_linked, 0);

    // The copy is the writable escape hatch.
    let copied = fs::metadata(dest.join("a.lua")).unwrap();
    assert!(!copied.permissions().readonly());
    assert_eq!(fs::read(dest.join("a.lua")).unwrap(), b"aaaa");
}

#[test]
fn verify_reports_corruption_and_materialize_fails_cleanly() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    write_file(&src, "a.lua", b"original");

    let store = Store::open(tmp.path().join("store"));
    let manifest = store.put_tree(&src).unwrap();
    let hash = manifest.entries[0].hash.clone();
    assert!(store.verify(&manifest).unwrap().is_empty());

    // Corrupt the object's bytes: verify must catch the hash mismatch.
    let obj = object_path(store.root(), &hash);
    make_writable(&obj);
    fs::write(&obj, b"tampered").unwrap();

    let corrupt = store.verify(&manifest).unwrap();
    assert_eq!(corrupt.len(), 1);
    assert!(matches!(corrupt[0].kind, CorruptKind::HashMismatch { .. }));

    // Delete the object entirely: verify reports Missing and materialize fails.
    fs::remove_file(&obj).unwrap();
    let corrupt = store.verify(&manifest).unwrap();
    assert!(matches!(corrupt[0].kind, CorruptKind::Missing));

    let dest = tmp.path().join("out");
    assert!(store.materialize(&manifest, &dest, LinkMode::Auto).is_err());
}

#[test]
fn gc_removes_only_unreferenced_objects() {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(tmp.path().join("store"));

    let live = tmp.path().join("live");
    write_file(&live, "keep.lua", b"keep me\n");
    write_file(&live, "shared.lua", b"shared\n");
    let live_manifest = store.put_tree(&live).unwrap();

    let dead = tmp.path().join("dead");
    write_file(&dead, "gone.lua", b"delete me\n");
    write_file(&dead, "shared.lua", b"shared\n"); // shared object stays live
    let _dead_manifest = store.put_tree(&dead).unwrap();

    assert_eq!(store.stats().unwrap().objects, 3);

    // Zero grace so freshly-written objects are eligible immediately.
    let report = store
        .gc_with_options(
            std::slice::from_ref(&live_manifest),
            GcOptions {
                grace: Duration::ZERO,
            },
        )
        .unwrap();

    assert_eq!(report.removed, 1); // only gone.lua's object
    assert_eq!(report.kept, 2); // keep.lua + shared.lua
    assert_eq!(store.stats().unwrap().objects, 2);

    // The live manifest still materializes intact after collection.
    let dest = tmp.path().join("out");
    store
        .materialize(&live_manifest, &dest, LinkMode::Auto)
        .unwrap();
    assert_eq!(read_tree(&live), read_tree(&dest));
}

#[test]
fn gc_grace_window_spares_recent_objects() {
    let tmp = TempDir::new().unwrap();
    let store = Store::open(tmp.path().join("store"));
    let dead = tmp.path().join("dead");
    write_file(&dead, "fresh.lua", b"just written\n");
    store.put_tree(&dead).unwrap();

    // A wide grace window protects the just-written (unreferenced) object.
    let report = store
        .gc_with_options(
            &[],
            GcOptions {
                grace: Duration::from_secs(3600),
            },
        )
        .unwrap();
    assert_eq!(report.removed, 0);
    assert_eq!(report.skipped_recent, 1);
    assert_eq!(store.stats().unwrap().objects, 1);
}

#[test]
fn concurrent_double_put_is_safe() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    for i in 0..12 {
        write_file(
            &src,
            &format!("file{i}.lua"),
            format!("contents {i}\n").as_bytes(),
        );
    }

    let store = Arc::new(Store::open(tmp.path().join("store")));
    let src = Arc::new(src);

    let handles: Vec<_> = (0..2)
        .map(|_| {
            let store = Arc::clone(&store);
            let src = Arc::clone(&src);
            thread::spawn(move || store.put_tree(src.as_path()).unwrap())
        })
        .collect();

    let manifests: Vec<TreeManifest> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    // Both threads agree on the tree, and no object was duplicated or torn.
    assert_eq!(manifests[0].tree_hash, manifests[1].tree_hash);
    assert_eq!(store.stats().unwrap().objects, 12);
    assert!(store.verify(&manifests[0]).unwrap().is_empty());
}

#[test]
fn package_manifest_persists_and_reloads() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    write_file(&src, "init.lua", b"return {}\n");
    write_file(&src, "doc/readme.md", b"# hi\n");

    let store = Store::open(tmp.path().join("store"));
    let manifest = store.put_tree(&src).unwrap();

    let path = store
        .write_package_manifest("penlight", "1.14.0", &manifest)
        .unwrap();
    assert!(path.exists());

    let reloaded = store.read_package_manifest(&path).unwrap();
    assert_eq!(manifest, reloaded);
}

#[cfg(unix)]
#[test]
fn executable_bit_survives_round_trip() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    write_file(&src, "bin/run", b"#!/bin/sh\necho hi\n");
    fs::set_permissions(src.join("bin/run"), fs::Permissions::from_mode(0o755)).unwrap();

    let store = Store::open(tmp.path().join("store"));
    let manifest = store.put_tree(&src).unwrap();
    assert!(manifest.entries.iter().any(|e| e.executable));

    let dest = tmp.path().join("out");
    store.materialize(&manifest, &dest, LinkMode::Copy).unwrap();
    let mode = fs::metadata(dest.join("bin/run"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o111, 0o111);
}
