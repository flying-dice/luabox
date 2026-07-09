//! Workspace member glob expansion (SPEC.md §5: `[workspace] members`).
//!
//! std-only: no `glob` dependency. Only a whole path-segment `*` wildcard is
//! supported (`packages/*`), which is the common case cargo itself
//! documents and the one SPEC.md's example uses. A member matches cargo
//! semantics: a directory is a member iff it contains a `luabox.toml`.

use std::path::{Path, PathBuf};

use super::model::Manifest;

impl Manifest {
    /// Expand `[workspace] members` globs relative to `root` (the directory
    /// containing this manifest) into the concrete member directories that
    /// actually contain a `luabox.toml`.
    ///
    /// Returns an empty vec when there's no `[workspace]` table. Entries are
    /// deduplicated and sorted for determinism.
    #[must_use]
    pub fn workspace_members(&self, root: &Path) -> Vec<PathBuf> {
        let Some(workspace) = &self.workspace else {
            return Vec::new();
        };
        let mut members: Vec<PathBuf> = workspace
            .members
            .iter()
            .flat_map(|pattern| expand_member_glob(root, pattern))
            .collect();
        members.sort();
        members.dedup();
        members
    }
}

/// Expands one member glob against `root`. Path segments are matched
/// literally except for a segment that is exactly `*`, which matches every
/// immediate subdirectory at that point. Only directories that contain a
/// `luabox.toml` are returned (cargo-style member detection).
fn expand_member_glob(root: &Path, pattern: &str) -> Vec<PathBuf> {
    let segments: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();

    let mut candidates = vec![root.to_path_buf()];
    for segment in segments {
        let mut next = Vec::new();
        if segment == "*" {
            for base in &candidates {
                let Ok(entries) = std::fs::read_dir(base) else {
                    continue;
                };
                for entry in entries.flatten() {
                    if entry.file_type().is_ok_and(|t| t.is_dir()) {
                        next.push(entry.path());
                    }
                }
            }
        } else {
            for base in &candidates {
                next.push(base.join(segment));
            }
        }
        candidates = next;
    }

    candidates
        .into_iter()
        .filter(|dir| dir.join("luabox.toml").is_file())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Manifest;

    fn write_member(root: &Path, name: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).expect("create member dir");
        std::fs::write(
            dir.join("luabox.toml"),
            "[package]\nname = \"pkg\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n",
        )
        .expect("write member manifest");
    }

    #[test]
    fn expands_star_glob_to_member_dirs_with_manifests() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        std::fs::create_dir_all(root.join("packages")).expect("packages dir");
        write_member(root, "packages/a");
        write_member(root, "packages/b");
        // Not a member: no luabox.toml inside.
        std::fs::create_dir_all(root.join("packages/not-a-member")).expect("dir");

        let manifest = Manifest::parse(
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[workspace]\nmembers = [\"packages/*\"]\n",
        )
        .expect("valid manifest");

        let members = manifest.workspace_members(root);
        assert_eq!(
            members,
            vec![
                root.join("packages").join("a"),
                root.join("packages").join("b")
            ]
        );
    }

    #[test]
    fn literal_member_path_without_wildcard() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        write_member(root, "solo");

        let manifest = Manifest::parse(
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[workspace]\nmembers = [\"solo\"]\n",
        )
        .expect("valid manifest");

        assert_eq!(manifest.workspace_members(root), vec![root.join("solo")]);
    }

    #[test]
    fn no_workspace_table_yields_no_members() {
        let manifest =
            Manifest::parse("[package]\nname = \"root\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n")
                .expect("valid manifest");
        assert!(manifest.workspace_members(Path::new(".")).is_empty());
    }
}
