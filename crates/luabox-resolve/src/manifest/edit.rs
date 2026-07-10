//! Comment-preserving manifest edits.
//!
//! `toml_edit` was chosen (SPEC.md §16.1) specifically so `luabox add` /
//! `luabox remove` can mutate a `luabox.toml` in place without disturbing
//! comments or formatting elsewhere in the file. [`Manifest::set_dependency`]
//! is the seed for that: a targeted [`toml_edit::DocumentMut`] mutation,
//! not a full-manifest regeneration.

use toml_edit::{InlineTable, Item, Table, Value};

use super::model::{Dependency, Manifest};

/// The two dependency tables `add`/`remove` operate on.
const DEP_TABLES: [&str; 2] = ["dependencies", "dev-dependencies"];

impl Manifest {
    /// Add or update a `[dependencies]` entry as a plain version
    /// requirement, preserving comments/formatting of everything else in
    /// the document — including any trailing comment on the entry itself,
    /// when updating one that already exists.
    ///
    /// Shorthand for [`Manifest::set_dependency_entry`] with
    /// [`Dependency::Version`] into `[dependencies]`.
    pub fn set_dependency(&mut self, name: &str, req: &str) {
        self.set_dependency_entry(name, &Dependency::Version(req.to_owned()), false);
    }

    /// Add or update a dependency of any kind (`luabox add`), preserving
    /// comments/formatting of everything else in the document — including
    /// any trailing comment on the entry itself, when updating one that
    /// already exists.
    ///
    /// With `dev`, the entry goes to `[dev-dependencies]`; otherwise to
    /// `[dependencies]`. The name is removed from the *other* table first
    /// (a package is a runtime or a dev dependency, not both), and the
    /// target table is created when absent. If the target exists but isn't
    /// a table (malformed manifest edited by hand), it's replaced with an
    /// empty one — the caller should normally have rejected such a
    /// manifest via [`Manifest::parse`] already.
    ///
    /// # Panics
    ///
    /// Never in practice: the target item is normalized to a table
    /// immediately above the only `expect` in this function.
    pub fn set_dependency_entry(&mut self, name: &str, dep: &Dependency, dev: bool) {
        let (target, other) = if dev {
            ("dev-dependencies", "dependencies")
        } else {
            ("dependencies", "dev-dependencies")
        };
        self.remove_from_table(other, name);

        let deps_item = self
            .document
            .as_table_mut()
            .entry(target)
            .or_insert_with(|| Item::Table(Table::new()));
        if !deps_item.is_table_like() {
            *deps_item = Item::Table(Table::new());
        }
        let deps_table = deps_item
            .as_table_like_mut()
            .expect("just ensured this item is table-like");

        let mut new_value = dependency_value(dep);
        if let Some(existing_decor) = deps_table
            .get(name)
            .and_then(Item::as_value)
            .map(Value::decor)
        {
            *new_value.decor_mut() = existing_decor.clone();
        }
        deps_table.insert(name, Item::Value(new_value));

        let typed = if dev {
            &mut self.dev_dependencies
        } else {
            &mut self.dependencies
        };
        typed.insert(name.to_owned(), dep.clone());
    }

    /// Remove `name` from `[dependencies]` and `[dev-dependencies]`
    /// (`luabox remove`), preserving comments/formatting of everything
    /// else — including the tables themselves, even when they end up
    /// empty. Returns whether an entry was actually removed.
    pub fn remove_dependency(&mut self, name: &str) -> bool {
        let mut removed = false;
        for table in DEP_TABLES {
            removed |= self.remove_from_table(table, name);
        }
        removed |= self.dependencies.remove(name).is_some();
        removed |= self.dev_dependencies.remove(name).is_some();
        removed
    }

    /// Drops `name` from the named top-level dependency table in the
    /// lossless document, if both exist.
    fn remove_from_table(&mut self, table: &str, name: &str) -> bool {
        self.document
            .as_table_mut()
            .get_mut(table)
            .and_then(Item::as_table_like_mut)
            .is_some_and(|deps| deps.remove(name).is_some())
    }
}

/// Renders a [`Dependency`] as the TOML value `luabox add` writes: a bare
/// requirement string, or an inline table (`{ git = …, tag = … }`) with
/// keys in the conventional order.
fn dependency_value(dep: &Dependency) -> Value {
    let mut table = InlineTable::new();
    match dep {
        Dependency::Version(req) => return Value::from(req.as_str()),
        Dependency::Git(git) => {
            table.insert("git", git.git.as_str().into());
            if let Some(rev) = &git.rev {
                table.insert("rev", rev.as_str().into());
            }
            if let Some(tag) = &git.tag {
                table.insert("tag", tag.as_str().into());
            }
            if let Some(branch) = &git.branch {
                table.insert("branch", branch.as_str().into());
            }
            if let Some(version) = &git.version {
                table.insert("version", version.as_str().into());
            }
        }
        Dependency::Path(path) => {
            table.insert("path", path.path.as_str().into());
            if let Some(version) = &path.version {
                table.insert("version", version.as_str().into());
            }
        }
        Dependency::Workspace(ws) => {
            table.insert("workspace", true.into());
            if let Some(version) = &ws.version {
                table.insert("version", version.as_str().into());
            }
        }
    }
    // Canonical `{ key = value, … }` spacing, matching hand-written style.
    table.fmt();
    Value::InlineTable(table)
}

#[cfg(test)]
mod tests {
    use crate::manifest::Manifest;

    const SRC: &str = "\
# top-of-file comment, must survive
[package]
name = \"my-lib\" # inline comment on name
version = \"1.2.0\"
edition = \"5.4\"

[dependencies]
penlight = \"1.14\" # existing dep comment

[tasks]
start = \"luabox run src/main.lua\"
";

    #[test]
    fn set_dependency_adds_new_entry_and_preserves_rest() {
        let mut manifest = Manifest::parse(SRC).expect("valid manifest");
        manifest.set_dependency("promise", "2.0");

        let out = manifest.to_string();
        assert!(out.contains("# top-of-file comment, must survive"));
        assert!(out.contains("name = \"my-lib\" # inline comment on name"));
        assert!(out.contains("penlight = \"1.14\" # existing dep comment"));
        assert!(out.contains("start = \"luabox run src/main.lua\""));
        assert!(out.contains("promise = \"2.0\""));

        // Round-trips back through the parser with the new dependency present.
        let reparsed = Manifest::parse(&out).expect("edited manifest still valid");
        assert_eq!(
            reparsed.dependencies.get("promise"),
            Some(&crate::manifest::Dependency::Version("2.0".to_owned()))
        );
        assert_eq!(
            reparsed.dependencies.get("penlight"),
            Some(&crate::manifest::Dependency::Version("1.14".to_owned()))
        );
    }

    #[test]
    fn set_dependency_updates_existing_entry_in_place() {
        let mut manifest = Manifest::parse(SRC).expect("valid manifest");
        manifest.set_dependency("penlight", "2.0");

        let out = manifest.to_string();
        assert!(out.contains("penlight = \"2.0\""));
        assert!(!out.contains("1.14"));
        // The comment on the line survives even though the value changed.
        assert!(out.contains("# existing dep comment"));
    }

    #[test]
    fn creates_dependencies_table_when_absent() {
        let mut manifest = Manifest::parse(
            "[package]\nname = \"my-lib\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n",
        )
        .expect("valid manifest");
        manifest.set_dependency("penlight", "1.14");

        let out = manifest.to_string();
        assert!(out.contains("[dependencies]"));
        assert!(out.contains("penlight = \"1.14\""));
    }

    #[test]
    fn set_dependency_entry_writes_inline_table_forms() {
        use crate::manifest::{Dependency, GitDependency, PathDependency};

        let mut manifest = Manifest::parse(SRC).expect("valid manifest");
        manifest.set_dependency_entry(
            "promise",
            &Dependency::Git(GitDependency {
                git: "https://example.com/promise.git".to_owned(),
                rev: None,
                tag: Some("v2.0.0".to_owned()),
                branch: None,
                version: None,
            }),
            false,
        );
        manifest.set_dependency_entry(
            "mylib",
            &Dependency::Path(PathDependency {
                path: "libs/mylib".to_owned(),
                version: None,
            }),
            false,
        );

        let out = manifest.to_string();
        assert!(
            out.contains(
                "promise = { git = \"https://example.com/promise.git\", tag = \"v2.0.0\" }"
            ),
            "{out}"
        );
        assert!(out.contains("mylib = { path = \"libs/mylib\" }"), "{out}");
        // Untouched content survives.
        assert!(out.contains("# top-of-file comment, must survive"));
        assert!(out.contains("penlight = \"1.14\" # existing dep comment"));

        // Round-trips: the edited document is a valid manifest with the
        // same typed view.
        let reparsed = Manifest::parse(&out).expect("edited manifest still valid");
        assert_eq!(reparsed.dependencies, manifest.dependencies);
        assert_eq!(reparsed.dev_dependencies, manifest.dev_dependencies);
    }

    #[test]
    fn set_dependency_entry_dev_moves_between_tables() {
        let mut manifest = Manifest::parse(SRC).expect("valid manifest");
        manifest.set_dependency_entry(
            "penlight",
            &crate::manifest::Dependency::Version("1.14".to_owned()),
            true,
        );

        let out = manifest.to_string();
        assert!(out.contains("[dev-dependencies]"), "{out}");
        let reparsed = Manifest::parse(&out).expect("still valid");
        assert!(reparsed.dependencies.is_empty());
        assert!(reparsed.dev_dependencies.contains_key("penlight"));
    }

    #[test]
    fn remove_dependency_deletes_entry_and_preserves_comments() {
        let mut manifest = Manifest::parse(SRC).expect("valid manifest");
        assert!(manifest.remove_dependency("penlight"));
        assert!(!manifest.dependencies.contains_key("penlight"));

        let out = manifest.to_string();
        assert!(!out.contains("penlight"), "{out}");
        assert!(out.contains("# top-of-file comment, must survive"));
        assert!(out.contains("start = \"luabox run src/main.lua\""));
        Manifest::parse(&out).expect("edited manifest still valid");
    }

    #[test]
    fn remove_dependency_unknown_name_reports_false() {
        let mut manifest = Manifest::parse(SRC).expect("valid manifest");
        assert!(!manifest.remove_dependency("nonexistent"));
        assert_eq!(manifest.to_string(), SRC, "document untouched");
    }

    #[test]
    fn remove_dependency_covers_dev_dependencies() {
        let src = "[package]\nname = \"x\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[dev-dependencies]\nbusted-compat = \"1.0\"\n";
        let mut manifest = Manifest::parse(src).expect("valid manifest");
        assert!(manifest.remove_dependency("busted-compat"));
        assert!(!manifest.to_string().contains("busted-compat"));
        assert!(manifest.dev_dependencies.is_empty());
    }
}
