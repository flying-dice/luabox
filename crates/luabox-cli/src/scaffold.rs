//! `luabox init` / `luabox new` — project scaffolding (SPEC.md §4, §5).

use std::fs;
use std::path::Path;

use anyhow::{Context, bail};
use luabox_syntax::Dialect;

/// Scaffold a project in `dir` (which must exist). `lib` selects a library
/// layout; the default is a binary/script project.
pub fn init(dir: &Path, lib: bool, edition: &str) -> anyhow::Result<()> {
    let Some(dialect) = Dialect::from_manifest_id(edition) else {
        bail!("unknown edition `{edition}` — expected one of: 5.1, 5.2, 5.3, 5.4, luajit");
    };
    let manifest = dir.join("luabox.toml");
    if manifest.exists() {
        bail!(
            "`{}` already exists — refusing to overwrite an existing project",
            manifest.display()
        );
    }

    let name = package_name(dir)?;
    fs::write(&manifest, manifest_toml(dialect))
        .with_context(|| format!("writing {}", manifest.display()))?;

    // The rockspec is the package manifest (name/version/dependencies) that
    // luarocks reads, pnpm-style (SPEC.md §6): it is what the author points
    // luarocks at to materialize `lua_modules/`. Scaffold the conventional
    // `<name>-<version>-<rockrev>.rockspec` next to luabox.toml.
    let rockspec_name = format!("{name}-0.1.0-1.rockspec");
    fs::write(dir.join(&rockspec_name), rockspec(&name, dialect))
        .with_context(|| format!("writing {rockspec_name}"))?;

    let src = dir.join("src");
    fs::create_dir_all(&src).with_context(|| format!("creating {}", src.display()))?;
    if lib {
        fs::write(src.join("lib.lua"), lib_lua(&name))?;
    } else {
        fs::write(src.join("main.lua"), main_lua(&name))?;
    }

    let gitignore = dir.join(".gitignore");
    if !gitignore.exists() {
        fs::write(gitignore, "dist/\n")?;
    }

    println!(
        "Created {} project `{name}` (edition {})",
        if lib { "library" } else { "binary" },
        dialect.manifest_id()
    );
    Ok(())
}

/// Scaffold a new project in a new directory `parent/name`.
pub fn new(parent: &Path, name: &str, lib: bool, edition: &str) -> anyhow::Result<()> {
    let dir = parent.join(name);
    if dir.exists() {
        bail!("destination `{}` already exists", dir.display());
    }
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    init(&dir, lib, edition)
}

/// Package name from the directory name: lowercased, runs of characters
/// outside `[a-z0-9]` collapsed to `-`.
fn package_name(dir: &Path) -> anyhow::Result<String> {
    let raw = dir
        .file_name()
        .and_then(|n| n.to_str())
        .context("cannot derive a package name from the current directory")?;
    let mut name = String::new();
    for c in raw.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c);
        } else if !name.is_empty() && !name.ends_with('-') {
            name.push('-');
        }
    }
    let name = name.trim_end_matches('-').to_owned();
    if name.is_empty() {
        bail!("cannot derive a package name from directory `{raw}`");
    }
    Ok(name)
}

/// The slimmed `luabox.toml`: tool config only (edition, build, types).
/// Name, version, and the dependency set luarocks resolves live in the
/// rockspec (SPEC.md §6), so there is no `[dependencies]` table — one is
/// added by hand when a dependency's own types should reach this package
/// (see `check`'s `lua_modules/` read path).
fn manifest_toml(dialect: Dialect) -> String {
    let edition = dialect.manifest_id();
    format!(
        r#"[package]
edition = "{edition}"

[build]
target = "{edition}"
out = "dist"

[types]
strict = true
"#
    )
}

/// The scaffolded rockspec: the package manifest luarocks reads. `source.url`
/// is a GitHub placeholder, `dependencies` pins the chosen Lua dialect, and
/// `build` is a pure-Lua `builtin` with an empty module map to fill in.
fn rockspec(name: &str, dialect: Dialect) -> String {
    // luajit is Lua 5.1-compatible; every other edition maps to its own
    // version number for the `lua` dependency constraint.
    let lua_version = if matches!(dialect, Dialect::LuaJit) {
        "5.1"
    } else {
        dialect.manifest_id()
    };
    format!(
        r#"rockspec_format = "3.0"
package = "{name}"
version = "0.1.0-1"

source = {{
   -- TODO: point this at your repository.
   url = "git+https://github.com/OWNER/{name}.git",
}}

dependencies = {{
   "lua >= {lua_version}",
}}

build = {{
   type = "builtin",
   modules = {{
      -- TODO: map module names to Lua files, e.g.
      -- ["{name}"] = "src/{name}.lua"
   }},
}}
"#
    )
}

fn main_lua(name: &str) -> String {
    format!("print(\"Hello from {name}!\")\n")
}

fn lib_lua(name: &str) -> String {
    let ident = name.replace('-', "_");
    format!(
        r#"local {ident} = {{}}

---Say hello.
---@return string
function {ident}.hello()
    return "Hello from {name}!"
end

return {ident}
"#
    )
}

#[cfg(test)]
mod tests {
    // test code — panics document assumptions
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::path::PathBuf;

    use super::*;
    use luabox_resolve::manifest::Manifest;

    /// A scaffolding target directory named `name` inside a fresh tempdir —
    /// the directory name is what `package_name` derives the package from.
    fn project_dir(name: &str) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join(name);
        fs::create_dir_all(&dir).expect("create project dir");
        (tmp, dir)
    }

    #[test]
    fn init_scaffolds_a_binary_project_with_manifest_rockspec_source_and_gitignore() {
        let (_tmp, dir) = project_dir("hello");
        init(&dir, false, "5.4").expect("init succeeds");

        let manifest = fs::read_to_string(dir.join("luabox.toml")).expect("luabox.toml written");
        assert!(manifest.contains(r#"edition = "5.4""#));
        assert!(manifest.contains(r#"out = "dist""#));
        assert!(manifest.contains("strict = true"));

        let rockspec =
            fs::read_to_string(dir.join("hello-0.1.0-1.rockspec")).expect("rockspec written");
        assert!(rockspec.contains(r#"package = "hello""#));
        assert!(rockspec.contains(r#"version = "0.1.0-1""#));
        assert!(rockspec.contains(r#""lua >= 5.4""#));

        let main = fs::read_to_string(dir.join("src").join("main.lua")).expect("src/main.lua");
        assert_eq!(main, "print(\"Hello from hello!\")\n");
        assert!(!dir.join("src").join("lib.lua").exists());

        assert_eq!(
            fs::read_to_string(dir.join(".gitignore")).expect(".gitignore"),
            "dist/\n"
        );
    }

    #[test]
    fn scaffolded_manifest_parses_as_a_valid_manifest() {
        let (_tmp, dir) = project_dir("parses");
        init(&dir, false, "5.1").expect("init succeeds");
        let text = fs::read_to_string(dir.join("luabox.toml")).expect("manifest");
        let manifest = Manifest::parse(&text).expect("scaffolded manifest must parse");
        assert_eq!(manifest.package.edition, "5.1");
        assert_eq!(manifest.build.target, "5.1");
        assert_eq!(manifest.build.out, "dist");
        assert!(manifest.types.strict);
    }

    #[test]
    fn init_lib_scaffolds_lib_lua_with_a_lua_identifier_derived_from_the_name() {
        let (_tmp, dir) = project_dir("My Cool Lib");
        init(&dir, true, "5.4").expect("init succeeds");

        let lib = fs::read_to_string(dir.join("src").join("lib.lua")).expect("src/lib.lua");
        // `my-cool-lib` is not a legal Lua identifier — dashes become
        // underscores for the local/table name, but the greeting keeps the
        // package name verbatim.
        assert!(lib.contains("local my_cool_lib = {}"));
        assert!(lib.contains("function my_cool_lib.hello()"));
        assert!(lib.contains("Hello from my-cool-lib!"));
        assert!(lib.contains("return my_cool_lib"));
        assert!(!dir.join("src").join("main.lua").exists());
    }

    #[test]
    fn init_for_luajit_pins_the_rockspec_lua_dependency_to_five_one() {
        let (_tmp, dir) = project_dir("jitpkg");
        init(&dir, false, "luajit").expect("init succeeds");

        let manifest = fs::read_to_string(dir.join("luabox.toml")).expect("manifest");
        assert!(manifest.contains(r#"edition = "luajit""#));
        let rockspec =
            fs::read_to_string(dir.join("jitpkg-0.1.0-1.rockspec")).expect("rockspec written");
        // luajit is Lua 5.1-compatible; luarocks has no `luajit` version.
        assert!(rockspec.contains(r#""lua >= 5.1""#), "{rockspec}");
    }

    #[test]
    fn init_rejects_an_unknown_edition_listing_the_valid_ones() {
        let (_tmp, dir) = project_dir("bad-edition");
        let error = init(&dir, false, "5.9").unwrap_err().to_string();
        assert!(error.contains("5.9"), "{error}");
        assert!(error.contains("5.1, 5.2, 5.3, 5.4, luajit"), "{error}");
        // Nothing is written when the edition is rejected.
        assert!(!dir.join("luabox.toml").exists());
        assert!(!dir.join("src").exists());
    }

    #[test]
    fn init_refuses_to_overwrite_an_existing_manifest() {
        let (_tmp, dir) = project_dir("existing");
        fs::write(dir.join("luabox.toml"), "# hand written\n").expect("write manifest");

        let error = init(&dir, false, "5.4").unwrap_err().to_string();
        assert!(error.contains("already exists"), "{error}");
        assert!(error.contains("refusing to overwrite"), "{error}");
        // The existing manifest is untouched.
        assert_eq!(
            fs::read_to_string(dir.join("luabox.toml")).expect("manifest"),
            "# hand written\n"
        );
    }

    #[test]
    fn init_preserves_an_existing_gitignore() {
        let (_tmp, dir) = project_dir("has-gitignore");
        fs::write(dir.join(".gitignore"), "*.log\n").expect("write .gitignore");
        init(&dir, false, "5.4").expect("init succeeds");
        assert_eq!(
            fs::read_to_string(dir.join(".gitignore")).expect(".gitignore"),
            "*.log\n"
        );
    }

    #[test]
    fn new_creates_the_directory_then_scaffolds_into_it() {
        let tmp = tempfile::tempdir().expect("tempdir");
        new(tmp.path(), "fresh-pkg", false, "5.3").expect("new succeeds");

        let dir = tmp.path().join("fresh-pkg");
        assert!(dir.join("luabox.toml").is_file());
        assert!(dir.join("fresh-pkg-0.1.0-1.rockspec").is_file());
        assert!(dir.join("src").join("main.lua").is_file());
        let manifest = fs::read_to_string(dir.join("luabox.toml")).expect("manifest");
        assert!(manifest.contains(r#"edition = "5.3""#));
    }

    #[test]
    fn new_refuses_an_existing_destination() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::create_dir(tmp.path().join("taken")).expect("mkdir");

        let error = new(tmp.path(), "taken", false, "5.4")
            .unwrap_err()
            .to_string();
        assert!(error.contains("already exists"), "{error}");
    }

    #[test]
    fn new_validates_the_edition_before_creating_anything_useful() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let error = new(tmp.path(), "bad", false, "nope")
            .unwrap_err()
            .to_string();
        assert!(error.contains("unknown edition"), "{error}");
        assert!(!tmp.path().join("bad").join("luabox.toml").exists());
    }

    #[test]
    fn package_name_lowercases_and_collapses_non_alphanumeric_runs_to_single_dashes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        for (dir_name, expected) in [
            ("Simple", "simple"),
            ("My Project!", "my-project"),
            ("a___b", "a-b"),
            ("with.dots.v2", "with-dots-v2"),
            ("---leading", "leading"),
            ("trailing---", "trailing"),
            ("MiXeD CaSe 42", "mixed-case-42"),
        ] {
            let dir = tmp.path().join(dir_name);
            fs::create_dir_all(&dir).expect("mkdir");
            assert_eq!(
                package_name(&dir).expect("derives a name"),
                expected,
                "for directory `{dir_name}`"
            );
        }
    }

    #[test]
    fn package_name_rejects_a_directory_with_no_ascii_alphanumerics() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("---");
        fs::create_dir_all(&dir).expect("mkdir");
        let error = package_name(&dir).unwrap_err().to_string();
        assert!(error.contains("cannot derive a package name"), "{error}");
    }

    #[test]
    fn manifest_toml_uses_the_edition_as_both_edition_and_build_target() {
        let toml = manifest_toml(Dialect::Lua52);
        assert!(toml.contains(r#"edition = "5.2""#));
        assert!(toml.contains(r#"target = "5.2""#));
    }

    #[test]
    fn rockspec_placeholder_url_and_empty_module_map_carry_the_package_name() {
        let spec = rockspec("my-pkg", Dialect::Lua54);
        assert!(spec.contains(r#"rockspec_format = "3.0""#));
        assert!(spec.contains("git+https://github.com/OWNER/my-pkg.git"));
        assert!(spec.contains(r#"type = "builtin""#));
        assert!(spec.contains(r#"["my-pkg"] = "src/my-pkg.lua""#));
    }
}
