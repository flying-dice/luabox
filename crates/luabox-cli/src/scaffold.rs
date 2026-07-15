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

    // The rockspec is the package manifest (name/version/registry deps),
    // pnpm-style (SPEC.md §6). Scaffold the conventional
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

/// The slimmed `luabox.toml`: tool config only (edition, build, types,
/// tasks). Name, version, and registry dependencies live in the rockspec
/// (SPEC.md §6), so there is no `[dependencies]` table — `path`/`git`
/// sources are added under one on demand by `luabox add`.
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

[tasks]
"#
    )
}

/// The scaffolded rockspec: the package manifest. `source.url` is a GitHub
/// placeholder, `dependencies` pins the chosen Lua dialect, and `build` is a
/// pure-Lua `builtin` with an empty module map to fill in.
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
   -- TODO: point this at your repository before publishing.
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
