//! `luabox bundle [--minify] [--sourcemap]` and `luabox unmap` — single-file
//! emit over the static require graph (SPEC.md §7, §18 P3).
//!
//! Pipeline:
//!
//! 1. **Discover** the project (nearest `luabox.toml`). The entry point is
//!    `src/main.lua` by convention (binary projects; the manifest has no
//!    `main` field yet). The bundle lands at `[build] out` (default
//!    `dist`) as `<package name>.lua` — one file per entry, and the entry
//!    is single today.
//! 2. **Check first** — the same gate as `luabox build`: bundle refuses to
//!    emit while `luabox check` reports errors.
//! 3. **Bundle** via `luabox-bundle`: modules are lowered
//!    `edition → target` (one hoisted `__luabox_rt` prelude), statically
//!    resolved `require`s are rewritten onto the emitted shim, unreachable
//!    modules are tree-shaken, dynamic requires fail loudly. `--minify`
//!    mangles locals/labels (never property names); `--sourcemap` writes
//!    `<bundle>.map` alongside.
//! 4. `luabox unmap <bundle> [traceback…]` rewrites `bundle.lua:NN`
//!    references in a traceback (argument or stdin) back to
//!    `module.lua:NN` via that map.
//!
//! Profiles (`dev`/`release`, `---@luabox-assert` stripping) are a
//! follow-up — SPEC.md §7.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use luabox_bundle::{BundleMap, BundleRequest, unmap_traceback};
use luabox_diag::{Code, Diagnostic, Format, Label, Severity, Span, render};

use crate::check_cmd;

/// Execute `luabox bundle` from `cwd`.
pub fn run(cwd: &Path, minify: bool, sourcemap: bool) -> anyhow::Result<()> {
    let project = check_cmd::discover(cwd)?;

    let entry = project.root.join("src").join("main.lua");
    if !entry.is_file() {
        bail!(
            "`luabox bundle` needs an entry point: expected `src/main.lua` under the \
             project root (binary-project convention; a manifest entry field is planned)"
        );
    }

    let out_dir: PathBuf = project
        .out_dir
        .clone()
        .unwrap_or_else(|| project.root.join("dist"));
    let name = format!("{}.lua", package_name(&project.root));

    // Check gate, exactly as `luabox build`: refuse to emit on check
    // errors. The out dir is passed through so previously emitted output
    // is never checked as project source.
    if check_cmd::run_once(cwd, None, "human", Some(&out_dir)).is_err() {
        bail!("`luabox bundle` refuses to emit while `luabox check` reports errors");
    }

    let request = BundleRequest {
        root: &project.root,
        entry: &entry,
        edition: project.dialect,
        target: project.build_target,
        name: &name,
        minify,
        sourcemap,
    };
    let bundle = luabox_bundle::bundle(&request).map_err(|e| anyhow::anyhow!("{e}"))?;

    // Warn-tier lowering diagnostics render like `luabox build`'s; they
    // never block the bundle.
    if !bundle.warnings.is_empty() {
        let diags: Vec<Diagnostic> = bundle
            .warnings
            .iter()
            .map(|(file, d)| {
                let code: Code = d
                    .code
                    .parse()
                    .unwrap_or_else(|_| unreachable!("luabox-lower emits registered codes"));
                let range = usize::from(d.range.start())..usize::from(d.range.end());
                let diag = match d.severity {
                    luabox_lower::Severity::Error => Diagnostic::error(code, d.message.clone()),
                    luabox_lower::Severity::Warning => Diagnostic::warning(code, d.message.clone()),
                };
                diag.with_label(Label::primary(Span::new(file, range), "lowered here"))
            })
            .collect();
        let root = project.root.clone();
        let lookup = move |file: &str| fs::read_to_string(root.join(file)).ok();
        let rendered = render(&diags, Format::Human, &lookup);
        if !rendered.is_empty() {
            println!("{rendered}");
        }
        if diags.iter().any(|d| d.severity == Severity::Error) {
            bail!("bundle failed");
        }
    }

    fs::create_dir_all(&out_dir)
        .with_context(|| format!("cannot create `{}`", out_dir.display()))?;
    let out_path = out_dir.join(&name);
    fs::write(&out_path, &bundle.text)
        .with_context(|| format!("cannot write `{}`", out_path.display()))?;
    if let Some(map) = &bundle.map {
        let map_path = out_dir.join(format!("{name}.map"));
        fs::write(&map_path, map)
            .with_context(|| format!("cannot write `{}`", map_path.display()))?;
    }

    println!(
        "bundle: {} module(s) inlined into {} ({} -> {}){}{}",
        bundle.modules,
        check_cmd::display_rel(&out_path, &project.root),
        project.dialect.manifest_id(),
        project.build_target.manifest_id(),
        if minify { ", minified" } else { "" },
        if sourcemap { ", with sourcemap" } else { "" },
    );
    Ok(())
}

/// Execute `luabox unmap <bundle> [traceback…]` from `cwd`: rewrite
/// `bundle.lua:NN` references via the `.lua.map` next to the bundle. The
/// traceback comes from the arguments when present, stdin otherwise.
pub fn unmap(cwd: &Path, bundle: &Path, traceback: Option<&str>) -> anyhow::Result<()> {
    let bundle_path = if bundle.is_absolute() {
        bundle.to_path_buf()
    } else {
        cwd.join(bundle)
    };
    let map_path = PathBuf::from(format!("{}.map", bundle_path.display()));
    let map_text = fs::read_to_string(&map_path).with_context(|| {
        format!(
            "cannot read `{}` (bundle with `luabox bundle --sourcemap` to produce it)",
            map_path.display()
        )
    })?;
    let map = BundleMap::from_json(&map_text).map_err(|e| anyhow::anyhow!(e))?;

    let text = if let Some(text) = traceback {
        text.to_owned()
    } else {
        let mut buffer = String::new();
        std::io::stdin()
            .read_to_string(&mut buffer)
            .context("cannot read the traceback from stdin")?;
        buffer
    };

    // Every spelling a traceback may use for the bundle: the path as
    // given (both slash directions), its basename, and the map's own
    // record of the bundle name.
    let given = bundle.to_string_lossy();
    let mut names = vec![
        given.replace('\\', "/"),
        given.replace('/', "\\"),
        map.bundle.clone(),
    ];
    if let Some(base) = bundle.file_name() {
        names.push(base.to_string_lossy().into_owned());
    }

    print!("{}", unmap_traceback(&map, &names, &text));
    if !text.ends_with('\n') {
        println!();
    }
    Ok(())
}

/// The `[package] name` from the project manifest, or `bundle` for
/// manifest-less directories (mirrors `check`'s least-surprise default).
fn package_name(root: &Path) -> String {
    let manifest_path = root.join("luabox.toml");
    let Ok(text) = fs::read_to_string(&manifest_path) else {
        return "bundle".to_owned();
    };
    match luabox_resolve::manifest::Manifest::parse(&text) {
        Ok(manifest) => manifest.package.name,
        Err(_) => "bundle".to_owned(),
    }
}
