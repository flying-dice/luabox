//! Comment-preserving manifest edits.
//!
//! `toml_edit` was chosen (SPEC.md §16.1) specifically so `luabox add` /
//! `luabox remove` can mutate a `luabox.toml` in place without disturbing
//! comments or formatting elsewhere in the file. [`Manifest::set_dependency`]
//! is the seed for that: a targeted [`toml_edit::DocumentMut`] mutation,
//! not a full-manifest regeneration.

use toml_edit::{Item, Table, Value};

use super::model::{Dependency, Manifest};

impl Manifest {
    /// Add or update a `[dependencies]` entry as a plain version
    /// requirement, preserving comments/formatting of everything else in
    /// the document — including any trailing comment on the entry itself,
    /// when updating one that already exists.
    ///
    /// If `[dependencies]` doesn't exist yet, it's created. If it exists
    /// but isn't a table (malformed manifest edited by hand), it's replaced
    /// with an empty one — the caller should normally have rejected such a
    /// manifest via [`Manifest::parse`] already.
    ///
    /// # Panics
    ///
    /// Never in practice: the `[dependencies]` item is normalized to a
    /// table immediately above the only `expect` in this function.
    pub fn set_dependency(&mut self, name: &str, req: &str) {
        let deps_item = self
            .document
            .as_table_mut()
            .entry("dependencies")
            .or_insert_with(|| Item::Table(Table::new()));
        if !deps_item.is_table_like() {
            *deps_item = Item::Table(Table::new());
        }
        let deps_table = deps_item
            .as_table_like_mut()
            .expect("just ensured this item is table-like");

        let mut new_value = Value::from(req);
        if let Some(existing_decor) = deps_table
            .get(name)
            .and_then(Item::as_value)
            .map(Value::decor)
        {
            *new_value.decor_mut() = existing_decor.clone();
        }
        deps_table.insert(name, Item::Value(new_value));

        self.dependencies
            .insert(name.to_owned(), Dependency::Version(req.to_owned()));
    }
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
}
