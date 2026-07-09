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
    fs::write(&manifest, manifest_toml(&name, dialect))
        .with_context(|| format!("writing {}", manifest.display()))?;

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

fn manifest_toml(name: &str, dialect: Dialect) -> String {
    let edition = dialect.manifest_id();
    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "{edition}"

[build]
target = "{edition}"
out = "dist"

[types]
strict = true

[dependencies]
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
