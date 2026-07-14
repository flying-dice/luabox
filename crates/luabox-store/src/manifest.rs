//! Tree manifests — the record of what a package *is* in the store.
//!
//! The store is a per-file content-addressed store (pnpm's model, not a
//! blob-per-archive one): every file is hashed and stored once under
//! `objects/`, and a [`TreeManifest`] is the ordered list of `path -> object`
//! entries that reconstitutes a package tree. See the crate docs for why
//! per-file CAS is the right shape (cross-version dedup; hard links need real
//! files).
//!
//! A manifest is itself content-addressed: [`TreeManifest::tree_hash`] is a
//! SHA-256 over a canonical encoding of its entries, so two byte-identical
//! trees always produce the same manifest hash regardless of how they were
//! walked.

use crate::error::StoreError;
use crate::hash::hash_bytes;
use crate::json::{self, Json};

/// One file in a package tree.
///
/// Paths are always stored relative to the tree root and use `/` separators on
/// every platform, so a manifest produced on Windows materializes identically
/// on Unix and vice versa.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// Tree-relative path, `/`-separated, never leading `/` or `.`/`..`.
    pub path: String,
    /// Hex SHA-256 of the file contents — the object address under `objects/`.
    pub hash: String,
    /// Whether the file carries the executable bit (Unix). Always `false` for
    /// trees produced on Windows, which has no such bit.
    pub executable: bool,
    /// File size in bytes (advisory: powers `stats()` and manifest display).
    pub size: u64,
}

/// The ordered, content-addressed contents of a package tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeManifest {
    /// Entries sorted by [`FileEntry::path`] — canonical, deterministic order.
    pub entries: Vec<FileEntry>,
    /// Hex SHA-256 over the canonical encoding of `entries`.
    pub tree_hash: String,
}

/// On-disk manifest schema version, embedded in the JSON.
const SCHEMA_VERSION: u64 = 1;

impl TreeManifest {
    /// Build a manifest from already-hashed entries, sorting them into
    /// canonical order and computing the tree hash.
    #[must_use]
    pub fn from_entries(mut entries: Vec<FileEntry>) -> Self {
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let tree_hash = compute_tree_hash(&entries);
        Self { entries, tree_hash }
    }

    /// Total byte size across all entries.
    #[must_use]
    pub fn total_size(&self) -> u64 {
        self.entries.iter().map(|e| e.size).sum()
    }

    /// The tree hash rendered in the registry/lockfile wire format:
    /// `sha256:<hex>`. This is the single place the `sha256:` algorithm prefix
    /// is spelled, so callers reconciling a store tree against a registry index
    /// checksum (`luabox install`) compare through here rather than re-`format!`
    /// the prefix at each site.
    #[must_use]
    pub fn checksum_string(&self) -> String {
        format!("sha256:{}", self.tree_hash)
    }

    /// The set of distinct object hashes this tree references. A tree can
    /// reference the same object twice (two identical files at different
    /// paths), so this deduplicates.
    #[must_use]
    pub fn object_hashes(&self) -> Vec<&str> {
        let mut hashes: Vec<&str> = self.entries.iter().map(|e| e.hash.as_str()).collect();
        hashes.sort_unstable();
        hashes.dedup();
        hashes
    }

    /// Serialize to the canonical on-disk JSON form.
    #[must_use]
    pub fn to_json(&self) -> String {
        let entries = self
            .entries
            .iter()
            .map(|e| {
                Json::Obj(vec![
                    ("path".to_string(), Json::Str(e.path.clone())),
                    ("hash".to_string(), Json::Str(e.hash.clone())),
                    ("executable".to_string(), Json::Bool(e.executable)),
                    ("size".to_string(), Json::Num(e.size)),
                ])
            })
            .collect();
        let doc = Json::Obj(vec![
            ("version".to_string(), Json::Num(SCHEMA_VERSION)),
            ("tree_hash".to_string(), Json::Str(self.tree_hash.clone())),
            ("entries".to_string(), Json::Arr(entries)),
        ]);
        doc.to_json_string()
    }

    /// Parse a manifest from its on-disk JSON form.
    ///
    /// The tree hash is recomputed from the parsed entries and checked against
    /// the stored `tree_hash`, so a tampered manifest file is rejected here.
    ///
    /// # Errors
    /// Fails on malformed JSON, an unknown schema version, missing fields, or a
    /// tree-hash mismatch — as the matchable [`StoreError`] variant for each.
    pub fn from_json(text: &str) -> Result<Self, StoreError> {
        let doc = json::parse(text).map_err(|message| StoreError::InvalidManifest { message })?;
        let version = doc
            .get("version")
            .and_then(Json::as_u64)
            .ok_or_else(|| missing("version", false))?;
        if version != SCHEMA_VERSION {
            return Err(StoreError::SchemaVersion { found: version });
        }
        let stored_hash = doc
            .get("tree_hash")
            .and_then(Json::as_str)
            .ok_or_else(|| missing("tree_hash", false))?
            .to_string();
        let raw = doc
            .get("entries")
            .and_then(Json::as_array)
            .ok_or_else(|| missing("entries", false))?;
        let mut entries = Vec::with_capacity(raw.len());
        for item in raw {
            entries.push(FileEntry {
                path: field_str(item, "path")?,
                hash: field_str(item, "hash")?,
                executable: item
                    .get("executable")
                    .and_then(Json::as_bool)
                    .ok_or_else(|| missing("executable", true))?,
                size: item
                    .get("size")
                    .and_then(Json::as_u64)
                    .ok_or_else(|| missing("size", true))?,
            });
        }
        let manifest = Self::from_entries(entries);
        if manifest.tree_hash != stored_hash {
            return Err(StoreError::TreeHashMismatch {
                stored: stored_hash,
                computed: manifest.tree_hash,
            });
        }
        Ok(manifest)
    }
}

/// A [`StoreError::MissingField`] for `field` (an entry field when `entry`).
fn missing(field: &str, entry: bool) -> StoreError {
    StoreError::MissingField {
        field: field.to_string(),
        entry,
    }
}

fn field_str(item: &Json, key: &str) -> Result<String, StoreError> {
    item.get(key)
        .and_then(Json::as_str)
        .map(str::to_string)
        .ok_or_else(|| missing(key, true))
}

/// Canonical encoding hashed to form the tree hash.
///
/// Each entry contributes `path\0hash\0mode\n` where `mode` is `1` for
/// executable and `0` otherwise, in sorted path order. Size is deliberately
/// excluded: it is derivable from the object and must not perturb identity.
fn compute_tree_hash(entries: &[FileEntry]) -> String {
    let mut buf = String::new();
    for e in entries {
        buf.push_str(&e.path);
        buf.push('\0');
        buf.push_str(&e.hash);
        buf.push('\0');
        buf.push(if e.executable { '1' } else { '0' });
        buf.push('\n');
    }
    hash_bytes(buf.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, hash: &str) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            hash: hash.to_string(),
            executable: false,
            size: 0,
        }
    }

    #[test]
    fn from_entries_is_order_independent() {
        let a = TreeManifest::from_entries(vec![entry("b", "22"), entry("a", "11")]);
        let b = TreeManifest::from_entries(vec![entry("a", "11"), entry("b", "22")]);
        assert_eq!(a.tree_hash, b.tree_hash);
        assert_eq!(a.entries[0].path, "a");
    }

    #[test]
    fn executable_bit_changes_identity() {
        let plain = TreeManifest::from_entries(vec![entry("x", "11")]);
        let exe = TreeManifest::from_entries(vec![FileEntry {
            executable: true,
            ..entry("x", "11")
        }]);
        assert_ne!(plain.tree_hash, exe.tree_hash);
    }

    #[test]
    fn json_round_trip() {
        let m = TreeManifest::from_entries(vec![
            FileEntry {
                path: "src/init.lua".to_string(),
                hash: "ab".repeat(32),
                executable: false,
                size: 10,
            },
            FileEntry {
                path: "bin/run".to_string(),
                hash: "cd".repeat(32),
                executable: true,
                size: 20,
            },
        ]);
        let round = TreeManifest::from_json(&m.to_json()).unwrap();
        assert_eq!(m, round);
    }

    #[test]
    fn tampered_manifest_is_rejected() {
        let m = TreeManifest::from_entries(vec![entry("a", "11")]);
        let text = m.to_json().replace("\"11\"", "\"99\"");
        assert!(TreeManifest::from_json(&text).is_err());
    }

    #[test]
    fn object_hashes_are_deduplicated() {
        let m = TreeManifest::from_entries(vec![entry("a", "11"), entry("b", "11")]);
        assert_eq!(m.object_hashes(), vec!["11"]);
    }

    #[test]
    fn checksum_string_prefixes_the_tree_hash() {
        let m = TreeManifest::from_entries(vec![entry("a", "11")]);
        assert_eq!(m.checksum_string(), format!("sha256:{}", m.tree_hash));
    }
}
