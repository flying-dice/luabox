//! The project's resolvable view of its manifest (SPEC.md §5).
//!
//! `luabox.toml` is the single project manifest: tool configuration (edition,
//! build, types, lints) plus the dependency *names* whose materialized trees
//! `check`/`lint`/the LSP read out of `lua_modules/` (or, for a `path` entry,
//! straight off disk). luabox does not fetch, resolve, or publish those
//! dependencies — the user materializes them with luarocks — so there is no
//! second manifest to fuse in any more.
//!
//! [`effective_manifest`] is what remains of that fusion: the one place a
//! project's *identity* (name and version) is required rather than merely
//! parsed. `Manifest::parse` deliberately leaves `[package] name`/`version`
//! optional so a member or scratch project parses without them; commands that
//! need a named, versioned package ask for it here and get a single, shared
//! error message.

use crate::manifest::Manifest;

/// Why a manifest could not be turned into a resolvable project view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectError {
    /// `luabox.toml`'s `[package]` supplies no name.
    NoName,
    /// `luabox.toml`'s `[package]` supplies no version.
    NoVersion,
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoName => write!(
                f,
                "no package name: add a `[package] name` to `luabox.toml`"
            ),
            Self::NoVersion => write!(
                f,
                "no package version: add a `[package] version` to `luabox.toml`"
            ),
        }
    }
}

impl std::error::Error for ProjectError {}

/// The resolvable view of `manifest`: a clone, once its package identity has
/// been checked.
///
/// # Errors
/// See [`ProjectError`]: a manifest with no `[package] name` or no
/// `[package] version`.
pub fn effective_manifest(manifest: &Manifest) -> Result<Manifest, ProjectError> {
    if manifest.package.name.is_empty() {
        return Err(ProjectError::NoName);
    }
    if manifest.package.version.is_empty() {
        return Err(ProjectError::NoVersion);
    }
    Ok(manifest.clone())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::manifest::Dependency;

    fn toml(src: &str) -> Manifest {
        Manifest::parse(src).expect("valid manifest")
    }

    #[test]
    fn a_named_versioned_manifest_passes_through() {
        let manifest = toml("[package]\nname = \"app\"\nversion = \"0.2.0\"\nedition = \"5.4\"\n");
        let eff = effective_manifest(&manifest).unwrap();
        assert_eq!(eff.package.name, "app");
        assert_eq!(eff.package.version, "0.2.0");
    }

    #[test]
    fn dependencies_are_carried_through_untouched() {
        let manifest = toml(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n\
             [dependencies]\nlocal-lib = { path = \"../local-lib\" }\npenlight = \"1.14\"\n",
        );
        let eff = effective_manifest(&manifest).unwrap();
        assert!(matches!(
            eff.dependencies.get("local-lib"),
            Some(Dependency::Path(_))
        ));
        assert!(matches!(
            eff.dependencies.get("penlight"),
            Some(Dependency::Version(_))
        ));
    }

    #[test]
    fn no_name_errors() {
        let manifest = toml("[package]\nedition = \"5.4\"\n");
        assert!(matches!(
            effective_manifest(&manifest),
            Err(ProjectError::NoName)
        ));
    }

    #[test]
    fn no_version_errors() {
        let manifest = toml("[package]\nname = \"app\"\nedition = \"5.4\"\n");
        assert!(matches!(
            effective_manifest(&manifest),
            Err(ProjectError::NoVersion)
        ));
    }
}
