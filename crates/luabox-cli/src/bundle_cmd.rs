//! `luabox bundle [--minify] [--sourcemap] [--mode <mode>]` and `luabox
//! unmap` — single-file emit over the static require graph, in one of
//! several embedding modes (SPEC.md §7, §18 P3).
//!
//! Pipeline:
//!
//! 1. **Discover** the project (nearest `luabox.toml`). The entry point is
//!    `src/main.lua` by convention (binary projects; the manifest has no
//!    `main` field yet).
//! 2. **Check first** — the same gate as `luabox build`: bundle refuses to
//!    emit while `luabox check` reports errors.
//! 3. **Bundle** via `luabox-bundle`: modules are lowered
//!    `edition → target` (one hoisted `__luabox_rt` prelude), statically
//!    resolved `require`s are rewritten onto the emitted shim, unreachable
//!    modules are tree-shaken, dynamic requires fail loudly. `--minify`
//!    mangles locals/labels (never property names); `--sourcemap` writes
//!    a `.map` alongside.
//! 4. **Embed**, per `--mode` (or `[build] mode`; the flag overrides the
//!    manifest — see `crate::modes` for the full semantics of each):
//!    - `plain` (default): `<out>/<package name>.lua` — one file per
//!      entry, and the entry is single today.
//!    - `love`: `<out>/<package name>.love`, a LÖVE-loadable zip archive.
//!    - `nvim-plugin`: `<out>/<package name>/`, a Neovim runtimepath
//!      plugin layout.
//! 5. `luabox unmap <bundle> [traceback…]` rewrites `bundle.lua:NN`
//!    references in a traceback (argument or stdin) back to
//!    `module.lua:NN` via that map (`plain`-mode bundles only today).
//!
//! Profiles (`dev`/`release`, `---@luabox-assert` stripping) are a
//! follow-up — SPEC.md §7.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use luabox_bundle::{BundleMap, BundleRequest, unmap_traceback};
use luabox_diag::{Code, Diagnostic, Format, Label, Span};
use luabox_resolve::manifest::Manifest;

use crate::check_cmd;
use crate::modes;

/// Execute `luabox bundle` from `cwd`. `mode` is the `--mode` flag value,
/// when given; it overrides `[build] mode` (default `plain`) — see
/// `crate::modes`.
pub fn run(cwd: &Path, minify: bool, sourcemap: bool, mode: Option<&str>) -> anyhow::Result<()> {
    // Validate a `--mode` override as early as possible — a typo shouldn't
    // have to wait through discovery/check/bundle to be reported.
    if let Some(m) = mode {
        modes::validate(m)?;
    }

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
    let manifest = read_manifest(&project.root);
    let package_name = manifest
        .as_ref()
        .map_or_else(|| "bundle".to_owned(), |m| m.package.name.clone());
    // `--mode` overrides `[build] mode` (default `plain`, and already
    // validated by `Manifest::parse` when it comes from the manifest).
    let effective_mode = mode.map_or_else(
        || {
            manifest
                .as_ref()
                .map_or_else(|| "plain".to_owned(), |m| m.build.mode.clone())
        },
        str::to_owned,
    );
    let name = format!("{package_name}.lua");

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
    render_warnings(&bundle, &project.root)?;

    fs::create_dir_all(&out_dir)
        .with_context(|| format!("cannot create `{}`", out_dir.display()))?;

    let (destination, mode_note) = emit_by_mode(&EmitArgs {
        mode: &effective_mode,
        project: &project,
        out_dir: &out_dir,
        package_name: &package_name,
        name: &name,
        bundle: &bundle,
        description: manifest
            .as_ref()
            .and_then(|m| m.package.description.clone()),
    })?;

    println!(
        "bundle: {} module(s) inlined into {} ({} -> {}){}{}{}",
        bundle.modules,
        crate::project::display_rel(&destination, &project.root),
        project.dialect.manifest_id(),
        project.build_target.manifest_id(),
        if minify { ", minified" } else { "" },
        if sourcemap { ", with sourcemap" } else { "" },
        mode_note,
    );
    Ok(())
}

/// Render warn-tier lowering diagnostics like `luabox build`'s (they never
/// block the bundle) and turn an error-tier one into a hard failure.
fn render_warnings(bundle: &luabox_bundle::Bundle, root: &Path) -> anyhow::Result<()> {
    if bundle.warnings.is_empty() {
        return Ok(());
    }
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
    if crate::project::render_diagnostics(&diags, Format::Human, root).errors > 0 {
        bail!("bundle failed");
    }
    Ok(())
}

/// Inputs to [`emit_by_mode`] — grouped to keep the function under clap's
/// (and clippy's) argument-count comfort zone.
struct EmitArgs<'a> {
    mode: &'a str,
    project: &'a check_cmd::Project,
    out_dir: &'a Path,
    package_name: &'a str,
    /// The `<package name>.lua` filename `plain` mode writes.
    name: &'a str,
    bundle: &'a luabox_bundle::Bundle,
    description: Option<String>,
}

/// Write the bundle out per its embedding mode. Returns the primary
/// artifact path (a file for `plain`/`love`, a directory for
/// `nvim-plugin`) and a trailing note for the summary line.
fn emit_by_mode(args: &EmitArgs<'_>) -> anyhow::Result<(PathBuf, String)> {
    match args.mode {
        "love" => {
            let love_path = modes::emit_love(
                &args.project.root,
                args.out_dir,
                args.package_name,
                &args.bundle.text,
                args.project.dialect,
                args.project.build_target,
            )?;
            Ok((love_path, ", packaged as a LÖVE .love archive".to_owned()))
        }
        "nvim-plugin" => {
            let plugin_root = modes::emit_nvim_plugin(
                args.out_dir,
                args.package_name,
                &args.bundle.text,
                args.description.as_deref(),
            )?;
            if let Some(map) = &args.bundle.map {
                let map_path = plugin_root
                    .join("lua")
                    .join(args.package_name)
                    .join("init.lua.map");
                fs::write(&map_path, map)
                    .with_context(|| format!("cannot write `{}`", map_path.display()))?;
            }
            Ok((
                plugin_root,
                ", written as a Neovim plugin layout".to_owned(),
            ))
        }
        _ => {
            let out_path = args.out_dir.join(args.name);
            fs::write(&out_path, &args.bundle.text)
                .with_context(|| format!("cannot write `{}`", out_path.display()))?;
            if let Some(map) = &args.bundle.map {
                let map_path = args.out_dir.join(format!("{}.map", args.name));
                fs::write(&map_path, map)
                    .with_context(|| format!("cannot write `{}`", map_path.display()))?;
            }
            Ok((out_path, String::new()))
        }
    }
}

/// Parse the project's `luabox.toml`, when present and valid. `None` for
/// manifest-less directories (mirrors `check_cmd::discover`'s
/// least-surprise default) or a manifest that fails to parse — the check
/// gate above already reports that loudly before this would matter.
fn read_manifest(root: &Path) -> Option<Manifest> {
    let manifest_path = root.join("luabox.toml");
    let text = fs::read_to_string(&manifest_path).ok()?;
    Manifest::parse(&text).ok()
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
