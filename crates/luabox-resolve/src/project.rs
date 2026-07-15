//! Merging the project's two manifests into one resolvable view (SPEC.md §6).
//!
//! luabox follows the pnpm/bun model: the **rockspec** (`*.rockspec`) is the
//! package manifest — it owns the package name, version, and *registry*
//! dependencies (its `dependencies`/`test_dependencies`, in LuaRocks constraint
//! syntax). `luabox.toml` is tool configuration (edition, build, types, tasks)
//! plus the *source* dependencies a rockspec cannot express: `path`, `git`, and
//! `workspace` entries.
//!
//! [`effective_manifest`] fuses the two into a single [`Manifest`] the resolver
//! consumes unchanged:
//!
//! - registry (version-requirement) dependencies come **only** from the
//!   rockspec — a `name = "^1.2"` entry left in `luabox.toml` is a hard error
//!   pointing the author at the rockspec;
//! - `path`/`git`/`workspace` dependencies come from `luabox.toml`;
//! - the two dependency sets are merged, and a name declared in both is a
//!   clear collision error;
//! - name and version come from the rockspec when one is present, else fall
//!   back to `luabox.toml`'s `[package]` (so a rockspec-less project — the
//!   examples, workspaces — keeps working unchanged).

use crate::luarocks::constraint::translate_version;
use crate::luarocks::dependency_from_spec;
use crate::luarocks::rockspec::Rockspec;
use crate::manifest::{Dependency, Manifest};

/// Why a project's two manifests could not be merged into one resolvable view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectError {
    /// A version-requirement dependency was found in `luabox.toml`. Registry
    /// dependencies live in the rockspec now.
    RegistryDepInToml { name: String, dev: bool },
    /// A rockspec registry dependency shares its name with a `luabox.toml`
    /// path/git/workspace dependency.
    NameCollision { name: String, dev: bool },
    /// A rockspec dependency string could not be translated.
    InvalidRockspecDep { spec: String, message: String },
    /// The rockspec declares no (statically readable) `package` name.
    MissingName,
    /// The rockspec declares no (statically readable) `version`.
    MissingVersion,
    /// The rockspec `version` has no semver image (e.g. `scm`).
    InvalidRockspecVersion { version: String },
    /// Neither a rockspec nor `luabox.toml` `[package]` supplies a name.
    NoName,
    /// Neither a rockspec nor `luabox.toml` `[package]` supplies a version.
    NoVersion,
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RegistryDepInToml { name, dev } => {
                let table = if *dev { "dev-dependencies" } else { "dependencies" };
                write!(
                    f,
                    "`{name}` is a version-requirement dependency in `luabox.toml` \
                     [{table}], but registry dependencies now live in the project's \
                     rockspec (luarocks.org is the registry, pnpm-style). \
                     Move `{name}` to the rockspec's `{}` and keep only path/git \
                     dependencies in `luabox.toml`",
                    if *dev { "test_dependencies" } else { "dependencies" }
                )
            }
            Self::NameCollision { name, dev } => {
                let table = if *dev { "dev-dependencies" } else { "dependencies" };
                write!(
                    f,
                    "`{name}` is declared both as a registry dependency in the rockspec \
                     and as a path/git dependency in `luabox.toml` [{table}] — a package \
                     has exactly one source. Remove one of them"
                )
            }
            Self::InvalidRockspecDep { spec, message } => {
                write!(f, "invalid rockspec dependency `{spec}`: {message}")
            }
            Self::MissingName => write!(
                f,
                "the project's rockspec declares no readable `package` name"
            ),
            Self::MissingVersion => write!(
                f,
                "the project's rockspec declares no readable `version`"
            ),
            Self::InvalidRockspecVersion { version } => write!(
                f,
                "the rockspec `version` \"{version}\" has no semver form (SCM/dev \
                 versions cannot anchor a resolvable project)"
            ),
            Self::NoName => write!(
                f,
                "no package name: add a `*.rockspec` (its `package`), or a \
                 `[package] name` in `luabox.toml`"
            ),
            Self::NoVersion => write!(
                f,
                "no package version: add a `*.rockspec` (its `version`), or a \
                 `[package] version` in `luabox.toml`"
            ),
        }
    }
}

impl std::error::Error for ProjectError {}

/// Fuse `manifest` (`luabox.toml`) and an optional project `rockspec` into the
/// single [`Manifest`] the resolver consumes.
///
/// The returned manifest is a clone of `manifest` with its name/version and
/// dependency maps overridden per the module rules; its lossless document is
/// left as-is (the resolver reads only the typed view, never re-serializes it).
///
/// # Errors
/// See [`ProjectError`]: a registry dep left in `luabox.toml`, a name declared
/// in both manifests, an untranslatable rockspec dep/version, or a project with
/// no name/version at all.
pub fn effective_manifest(
    manifest: &Manifest,
    rockspec: Option<&Rockspec>,
) -> Result<Manifest, ProjectError> {
    let mut eff = manifest.clone();

    // `luabox.toml` may no longer carry registry (version-requirement) deps.
    for (name, dep) in &eff.dependencies {
        if matches!(dep, Dependency::Version(_)) {
            return Err(ProjectError::RegistryDepInToml {
                name: name.clone(),
                dev: false,
            });
        }
    }
    for (name, dep) in &eff.dev_dependencies {
        if matches!(dep, Dependency::Version(_)) {
            return Err(ProjectError::RegistryDepInToml {
                name: name.clone(),
                dev: true,
            });
        }
    }

    if let Some(spec) = rockspec {
        eff.package.name = spec.package.clone().ok_or(ProjectError::MissingName)?;
        let raw_version = spec.version.clone().ok_or(ProjectError::MissingVersion)?;
        let version = translate_version(&raw_version)
            .ok_or(ProjectError::InvalidRockspecVersion { version: raw_version })?;
        eff.package.version = version.to_string();

        merge_registry_deps(&spec.dependencies, &mut eff.dependencies, false)?;
        merge_registry_deps(&spec.test_dependencies, &mut eff.dev_dependencies, true)?;
    }

    if eff.package.name.is_empty() {
        return Err(ProjectError::NoName);
    }
    if eff.package.version.is_empty() {
        return Err(ProjectError::NoVersion);
    }
    Ok(eff)
}

/// Translate each rockspec dependency string and insert it into `target`,
/// erroring on a name that collides with an existing (path/git) entry.
fn merge_registry_deps(
    specs: &[String],
    target: &mut std::collections::BTreeMap<String, Dependency>,
    dev: bool,
) -> Result<(), ProjectError> {
    for spec in specs {
        let Some((name, dep)) =
            dependency_from_spec(spec).map_err(|message| ProjectError::InvalidRockspecDep {
                spec: spec.clone(),
                message,
            })?
        else {
            continue; // `lua` interpreter constraint / empty entry
        };
        if target.contains_key(&name) {
            return Err(ProjectError::NameCollision { name, dev });
        }
        target.insert(name, dep);
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::luarocks::rockspec;

    fn toml(src: &str) -> Manifest {
        Manifest::parse(src).expect("valid manifest")
    }

    #[test]
    fn rockspec_supplies_name_version_and_registry_deps() {
        let manifest = toml("[package]\nedition = \"5.4\"\n");
        let spec = rockspec::read(
            "package = \"widget\"\nversion = \"1.2.3-1\"\n\
             dependencies = { \"lua >= 5.1\", \"lpeg >= 1.0\" }\n\
             test_dependencies = { \"busted >= 2.0\" }\n",
        );
        let eff = effective_manifest(&manifest, Some(&spec)).unwrap();
        assert_eq!(eff.package.name, "widget");
        assert_eq!(eff.package.version, "1.2.3"); // rock revision dropped
        assert_eq!(
            eff.dependencies.get("lpeg"),
            Some(&Dependency::Version(">=1.0".to_owned()))
        );
        assert!(!eff.dependencies.contains_key("lua"), "lua is metadata");
        assert_eq!(
            eff.dev_dependencies.get("busted"),
            Some(&Dependency::Version(">=2.0".to_owned()))
        );
    }

    #[test]
    fn path_deps_in_toml_merge_with_rockspec_registry_deps() {
        let manifest = toml(
            "[package]\nedition = \"5.4\"\n\n[dependencies]\nlocal-lib = { path = \"../local-lib\" }\n",
        );
        let spec = rockspec::read("package = \"app\"\nversion = \"0.1.0-1\"\ndependencies = { \"lpeg\" }\n");
        let eff = effective_manifest(&manifest, Some(&spec)).unwrap();
        assert!(matches!(
            eff.dependencies.get("local-lib"),
            Some(Dependency::Path(_))
        ));
        assert!(matches!(
            eff.dependencies.get("lpeg"),
            Some(Dependency::Version(_))
        ));
    }

    #[test]
    fn version_dep_in_toml_is_a_hard_error() {
        let manifest = toml("[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n[dependencies]\npenlight = \"1.14\"\n");
        let err = effective_manifest(&manifest, None).unwrap_err();
        assert!(matches!(err, ProjectError::RegistryDepInToml { .. }));
        assert!(err.to_string().contains("rockspec"));
    }

    #[test]
    fn name_declared_in_both_collides() {
        let manifest = toml(
            "[package]\nedition = \"5.4\"\n\n[dependencies]\nlpeg = { git = \"https://example/lpeg\" }\n",
        );
        let spec = rockspec::read("package = \"app\"\nversion = \"1.0-1\"\ndependencies = { \"lpeg >= 1.0\" }\n");
        let err = effective_manifest(&manifest, Some(&spec)).unwrap_err();
        assert!(matches!(err, ProjectError::NameCollision { .. }));
    }

    #[test]
    fn no_rockspec_falls_back_to_toml_package() {
        let manifest = toml("[package]\nname = \"app\"\nversion = \"0.2.0\"\nedition = \"5.4\"\n");
        let eff = effective_manifest(&manifest, None).unwrap();
        assert_eq!(eff.package.name, "app");
        assert_eq!(eff.package.version, "0.2.0");
    }

    #[test]
    fn no_name_anywhere_errors() {
        let manifest = toml("[package]\nedition = \"5.4\"\n");
        assert!(matches!(
            effective_manifest(&manifest, None),
            Err(ProjectError::NoName)
        ));
    }
}
