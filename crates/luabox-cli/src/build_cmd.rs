//! `luabox build` — the single tsc/esbuild-style emit command
//! (SPEC.md §2.1, §4, §7, §18 P3; flying-dice/luabox#4). `luabox bundle`
//! no longer exists: its per-mode packaging lives here, driven by
//! `[build]` config (`bundle`, `mode`, `entry`, `outfile`, `sourcemap`,
//! `minify`) with CLI flags overriding every field.
//!
//! Pipeline:
//!
//! 1. **Discover** the project (nearest `luabox.toml`) and resolve the
//!    effective build config: flags override `[build]`, which defaults the
//!    target to the edition and the out dir to `dist`.
//! 2. **Check first** — the same per-file gate as `luabox check` (parse +
//!    *edition* dialect legality + typecheck). Build refuses to emit while
//!    check reports errors. Target-dialect legality is deliberately *not*
//!    part of this gate: constructs illegal on the target are exactly what
//!    lowering exists to handle.
//! 3. **Emit**, one of two shapes:
//!    - **Tree mode** (`bundle = false`, `mode = plain`): every `.lua` file
//!      is lowered `edition → target` and written under `out`, mirroring the
//!      source layout. `edition == target` copies byte-identical. `*.d.lua`
//!      analyser-only surfaces are skipped. No require-graph work; no
//!      implicit cleaning of stale `out` files (tsc/esbuild semantics).
//!    - **Bundle mode** (`bundle = true`, or any non-`plain` `mode`): each
//!      entry point's static `require` graph is inlined into one
//!      target-lowered file (reusing `luabox-bundle`), then packaged per
//!      `mode` (`plain` → `.lua`, `love` → `.love` zip, `nvim-plugin` →
//!      runtimepath tree). `sourcemap` writes a `.map` beside each bundle
//!      for `luabox unmap`; `minify` mangles locals.
//!
//! Output rules (flying-dice/luabox#4, enforced below):
//! `entry` defaults to `["src/main.lua"]`; bundling with a missing entry is
//! a clear error, never a guess; `outfile` is valid only with exactly one
//! entry and never with a non-`plain` mode; multi-entry bundle names derive
//! from entry basenames under `out`.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use luabox_bundle::{BundleMap, BundleRequest, unmap_traceback};
use luabox_diag::{Code, Diagnostic, Format, Label, Span};
use luabox_lower::LowerDiagnostic;
use luabox_resolve::manifest::{Build, DEFAULT_ENTRY, Manifest};
use luabox_syntax::{Dialect, lua};
use rayon::prelude::*;

use crate::check_cmd;
use crate::modes;

/// CLI overrides for `luabox build`; each field, when set, wins over the
/// corresponding `[build]` config value.
pub struct BuildOptions {
    /// `--target`: dialect to lower to.
    pub target: Option<String>,
    /// `--out`: output directory.
    pub out: Option<PathBuf>,
    /// `--outfile`: single-entry bundle output path.
    pub outfile: Option<PathBuf>,
    /// `--entry` (repeatable): bundle entry points.
    pub entry: Vec<PathBuf>,
    /// `--bundle` / `--no-bundle`: `Some(true)`/`Some(false)`; `None` defers
    /// to `[build] bundle`.
    pub bundle: Option<bool>,
    /// `--sourcemap`: presence ORs with `[build] sourcemap`.
    pub sourcemap: bool,
    /// `--minify`: presence ORs with `[build] minify`.
    pub minify: bool,
    /// `--mode`: embedding mode override.
    pub mode: Option<String>,
}

/// Execute `luabox build` from `cwd`.
#[allow(
    clippy::too_many_lines,
    reason = "the effective-config resolution + output-rule validation reads as one linear pipeline"
)]
pub fn run(cwd: &Path, opts: &BuildOptions) -> anyhow::Result<()> {
    // Validate a `--mode` override as early as possible — a typo shouldn't
    // wait through discovery/check to be reported.
    if let Some(m) = &opts.mode {
        modes::validate(m)?;
    }

    let project = check_cmd::discover(cwd)?;
    let manifest = read_manifest(&project.root);
    let build_cfg: Build = manifest
        .as_ref()
        .map_or_else(default_build, |m| m.build.clone());

    let edition = project.dialect;
    let target = match opts.target.as_deref() {
        Some(id) => match Dialect::from_manifest_id(id) {
            Some(dialect) => dialect,
            None => bail!(
                "unknown target `{id}`; expected one of: 5.1, 5.2, 5.3, 5.4, luajit \
                 (see `luabox explain LB1001`)"
            ),
        },
        None => project.build_target,
    };
    let out_dir: PathBuf = match opts.out.as_deref() {
        Some(dir) if dir.is_absolute() => dir.to_path_buf(),
        Some(dir) => cwd.join(dir),
        None => project
            .out_dir
            .clone()
            .unwrap_or_else(|| project.root.join("dist")),
    };

    // Effective config: flags override `[build]`.
    let mode = opts.mode.clone().unwrap_or(build_cfg.mode);
    let bundle_flag = opts.bundle.unwrap_or(build_cfg.bundle);
    // A non-`plain` mode packages a bundle, so it implies bundling even when
    // `bundle` is left false (the LÖVE / Neovim examples set only `mode`).
    let do_bundle = bundle_flag || mode != "plain";
    let sourcemap = opts.sourcemap || build_cfg.sourcemap;
    let minify = opts.minify || build_cfg.minify;

    if !do_bundle {
        // Tree mode ignores the bundle-only knobs (`entry`, `outfile`,
        // `sourcemap`, `minify`) — there is no require graph to walk.
        if check_cmd::run_once(cwd, None, "human", Some(&out_dir)).is_err() {
            bail!("`luabox build` refuses to emit while `luabox check` reports errors");
        }
        return emit_tree(cwd, &out_dir, edition, target);
    }

    // Bundle mode: resolve entries and the output-naming rules.
    let entry_specs: Vec<String> = if opts.entry.is_empty() {
        build_cfg.entry
    } else {
        opts.entry
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    };
    let outfile: Option<PathBuf> = opts
        .outfile
        .clone()
        .or_else(|| build_cfg.outfile.map(PathBuf::from));

    if entry_specs.is_empty() {
        bail!(
            "`luabox build` cannot bundle without an entry point: `[build] entry` is empty \
             (a library project has nothing to bundle — set `bundle = false`, or add an entry)"
        );
    }
    if outfile.is_some() && entry_specs.len() != 1 {
        bail!(
            "`outfile` is valid only with exactly one entry point, but {} are configured \
             — drop `outfile` and each bundle is named from its entry's basename under `out`",
            entry_specs.len()
        );
    }
    if outfile.is_some() && mode != "plain" {
        bail!(
            "`outfile` conflicts with `mode = \"{mode}\"`: that mode dictates its own output \
             layout (a `.love` archive / a Neovim plugin tree), so an output filename is \
             meaningless — drop one of them"
        );
    }
    if mode != "plain" && entry_specs.len() != 1 {
        bail!(
            "`mode = \"{mode}\"` packages a single entry point, but {} are configured",
            entry_specs.len()
        );
    }

    let entries: Vec<PathBuf> = entry_specs
        .iter()
        .map(|spec| resolve_entry(&project.root, spec))
        .collect();
    for (spec, path) in entry_specs.iter().zip(&entries) {
        if !path.is_file() {
            bail!(
                "bundle entry `{spec}` was not found at `{}` — set `[build] entry` (or pass \
                 `--entry`) to your actual entry point(s)",
                crate::project::display_rel(path, &project.root)
            );
        }
    }

    // Check gate, exactly as tree mode: refuse to emit on check errors.
    if check_cmd::run_once(cwd, None, "human", Some(&out_dir)).is_err() {
        bail!("`luabox build` refuses to emit while `luabox check` reports errors");
    }

    fs::create_dir_all(&out_dir)
        .with_context(|| format!("cannot create `{}`", out_dir.display()))?;

    let package_name = manifest
        .as_ref()
        .map_or_else(|| "bundle".to_owned(), |m| m.package.name.clone());
    let package_name = if package_name.is_empty() {
        "bundle".to_owned()
    } else {
        package_name
    };
    let description = manifest
        .as_ref()
        .and_then(|m| m.package.description.clone());

    match mode.as_str() {
        "love" => emit_love(&EmitCtx {
            root: &project.root,
            out_dir: &out_dir,
            edition,
            target,
            minify,
            sourcemap,
            package_name: &package_name,
            description: description.as_deref(),
            entry: &entries[0],
        }),
        "nvim-plugin" => emit_nvim(&EmitCtx {
            root: &project.root,
            out_dir: &out_dir,
            edition,
            target,
            minify,
            sourcemap,
            package_name: &package_name,
            description: description.as_deref(),
            entry: &entries[0],
        }),
        _ => emit_plain(
            &project.root,
            &out_dir,
            edition,
            target,
            minify,
            sourcemap,
            &entries,
            outfile.as_deref(),
        ),
    }
}

/// The `[build]` defaults for a manifest-less project (mirrors
/// `Manifest::parse`'s `[build]` fallback).
fn default_build() -> Build {
    Build {
        target: String::new(),
        out: "dist".to_owned(),
        mode: "plain".to_owned(),
        entry: vec![DEFAULT_ENTRY.to_owned()],
        outfile: None,
        bundle: false,
        sourcemap: false,
        minify: false,
    }
}

/// Resolve an entry spec (from `[build] entry` or `--entry`) against the
/// project root; an absolute path is taken as-is.
fn resolve_entry(root: &Path, spec: &str) -> PathBuf {
    let path = Path::new(spec);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

// ---------------------------------------------------------------------------
// Tree mode
// ---------------------------------------------------------------------------

/// Lower every project `.lua` file `edition → target` and write it under
/// `out_dir`, mirroring the source layout (SPEC.md §2.1). The check gate has
/// already passed by the time this runs.
fn emit_tree(cwd: &Path, out_dir: &Path, edition: Dialect, target: Dialect) -> anyhow::Result<()> {
    // Re-discover with the chosen out dir so the file walk skips previous
    // build output even when `--out` overrides the manifest.
    let mut project = check_cmd::discover(cwd)?;
    project.out_dir = Some(out_dir.to_path_buf());
    let lua_files =
        crate::project::collect_lua_files(&project.root, project.out_dir.as_deref(), true)?;

    let results: Vec<anyhow::Result<Vec<Diagnostic>>> = lua_files
        .par_iter()
        .map(|path| {
            let rel = crate::project::display_rel(path, &project.root);
            let source =
                fs::read_to_string(path).with_context(|| format!("cannot read `{rel}`"))?;
            let (output, diags) = lower_one(&source, &rel, edition, target);
            if let Some(output) = output {
                let dest = out_dir.join(path.strip_prefix(&project.root).unwrap_or(path));
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)
                        .with_context(|| format!("cannot create `{}`", parent.display()))?;
                }
                fs::write(&dest, output)
                    .with_context(|| format!("cannot write `{}`", dest.display()))?;
            }
            Ok(diags)
        })
        .collect();

    let mut diags = Vec::new();
    for result in results {
        diags.extend(result?);
    }
    let errors = crate::project::render_diagnostics(&diags, Format::Human, &project.root).errors;
    if errors > 0 {
        bail!("build failed with {errors} error(s)");
    }
    let out_display = crate::project::display_rel(out_dir, &project.root);
    println!(
        "build: {} files emitted to {} ({} -> {})",
        lua_files.len(),
        out_display,
        edition.manifest_id(),
        target.manifest_id(),
    );
    Ok(())
}

/// Lower one file. Returns the output bytes (when emittable) plus the
/// diagnostics to render. `edition == target` is a byte-identical copy —
/// no lowering, no reprint.
fn lower_one(
    source: &str,
    rel: &str,
    edition: Dialect,
    target: Dialect,
) -> (Option<String>, Vec<Diagnostic>) {
    if edition == target {
        return (Some(source.to_owned()), Vec::new());
    }
    match luabox_lower::lower(source, edition, target) {
        Err(lower_diags) => (None, to_diagnostics(&lower_diags, rel)),
        Ok(lowered) => {
            let mut diags = to_diagnostics(&lowered.warnings, rel);
            // Residual validation: the output must be legal under the
            // target. Anything left over has no lowering rule — fail loudly
            // instead of emitting a file the target runtime rejects.
            let parse = lua::parse(&lowered.text, target);
            let mut residual = false;
            for err in parse.errors() {
                residual = true;
                diags.push(
                    Diagnostic::error(
                        Code::new(1),
                        format!(
                            "lowered output does not parse under the target: {}",
                            err.message
                        ),
                    )
                    .with_note(format!("in the lowered output of `{rel}`")),
                );
            }
            for finding in lua::validate::validate(&parse, target) {
                residual = true;
                let code: Code = finding
                    .code
                    .parse()
                    .unwrap_or_else(|_| unreachable!("validator emits registered codes"));
                diags.push(Diagnostic::error(code, finding.message).with_note(format!(
                    "this construct has no lowering rule for target {}; it remains in the \
                         lowered output of `{rel}`",
                    target.manifest_id()
                )));
            }
            if residual {
                (None, diags)
            } else {
                (Some(lowered.text), diags)
            }
        }
    }
}

/// Map `luabox-lower`'s plain-code diagnostics onto rendered ones.
fn to_diagnostics(lower_diags: &[LowerDiagnostic], rel: &str) -> Vec<Diagnostic> {
    lower_diags
        .iter()
        .map(|d| {
            let code: Code = d
                .code
                .parse()
                .unwrap_or_else(|_| unreachable!("luabox-lower emits registered codes"));
            let range = usize::from(d.range.start())..usize::from(d.range.end());
            let diag = match d.severity {
                luabox_lower::Severity::Error => Diagnostic::error(code, d.message.clone()),
                luabox_lower::Severity::Warning => Diagnostic::warning(code, d.message.clone()),
            };
            diag.with_label(Label::primary(Span::new(rel, range), "lowered here"))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Bundle mode
// ---------------------------------------------------------------------------

/// Everything a `love`/`nvim-plugin` embed needs (grouped to stay under
/// clippy's argument-count comfort zone).
struct EmitCtx<'a> {
    root: &'a Path,
    out_dir: &'a Path,
    edition: Dialect,
    target: Dialect,
    minify: bool,
    sourcemap: bool,
    package_name: &'a str,
    description: Option<&'a str>,
    entry: &'a Path,
}

/// Bundle one entry file plus its static require graph.
fn bundle_one(
    root: &Path,
    entry: &Path,
    name: &str,
    edition: Dialect,
    target: Dialect,
    minify: bool,
    sourcemap: bool,
) -> anyhow::Result<luabox_bundle::Bundle> {
    let request = BundleRequest {
        root,
        entry,
        edition,
        target,
        name,
        minify,
        sourcemap,
    };
    let bundle = luabox_bundle::bundle(&request).map_err(|e| anyhow::anyhow!("{e}"))?;
    render_warnings(&bundle, root)?;
    Ok(bundle)
}

/// Plain mode: one `.lua` (plus optional `.map`) per entry. A single entry
/// may override its path with `outfile`; otherwise each bundle is named from
/// its entry basename under `out_dir` (esbuild semantics).
#[allow(
    clippy::too_many_arguments,
    reason = "the plain-emit loop threads the effective config"
)]
fn emit_plain(
    root: &Path,
    out_dir: &Path,
    edition: Dialect,
    target: Dialect,
    minify: bool,
    sourcemap: bool,
    entries: &[PathBuf],
    outfile: Option<&Path>,
) -> anyhow::Result<()> {
    for entry in entries {
        let out_path = match outfile {
            Some(of) if of.is_absolute() => of.to_path_buf(),
            Some(of) => root.join(of),
            None => {
                let base = entry
                    .file_name()
                    .unwrap_or_else(|| std::ffi::OsStr::new("bundle.lua"));
                out_dir.join(base)
            }
        };
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("cannot create `{}`", parent.display()))?;
        }
        let name = out_path.file_name().map_or_else(
            || "bundle.lua".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        let bundle = bundle_one(root, entry, &name, edition, target, minify, sourcemap)?;
        fs::write(&out_path, &bundle.text)
            .with_context(|| format!("cannot write `{}`", out_path.display()))?;
        if let Some(map) = &bundle.map {
            let map_path = PathBuf::from(format!("{}.map", out_path.display()));
            fs::write(&map_path, map)
                .with_context(|| format!("cannot write `{}`", map_path.display()))?;
        }
        println!(
            "build: {} module(s) inlined into {} ({} -> {}){}{}",
            bundle.modules,
            crate::project::display_rel(&out_path, root),
            edition.manifest_id(),
            target.manifest_id(),
            if minify { ", minified" } else { "" },
            if sourcemap { ", with sourcemap" } else { "" },
        );
    }
    Ok(())
}

/// LÖVE mode: bundle the entry and package it as `<package>.love`. LÖVE
/// dictates the archive layout (entry is `main.lua` at the root), so the
/// bundle text is written unmodified as `main.lua`; `sourcemap` has nowhere
/// sensible to land in a `.love` and is dropped (documented follow-up).
fn emit_love(ctx: &EmitCtx<'_>) -> anyhow::Result<()> {
    let bundle = bundle_one(
        ctx.root,
        ctx.entry,
        "main.lua",
        ctx.edition,
        ctx.target,
        ctx.minify,
        false,
    )?;
    let love_path = modes::emit_love(
        ctx.root,
        ctx.out_dir,
        ctx.package_name,
        &bundle.text,
        ctx.edition,
        ctx.target,
    )?;
    println!(
        "build: {} module(s) inlined into {} ({} -> {}){}, packaged as a LÖVE .love archive",
        bundle.modules,
        crate::project::display_rel(&love_path, ctx.root),
        ctx.edition.manifest_id(),
        ctx.target.manifest_id(),
        if ctx.minify { ", minified" } else { "" },
    );
    Ok(())
}

/// Neovim plugin mode: bundle the entry into a runtimepath tree under
/// `<package>/`. A `.map` (when requested) lands beside `init.lua`.
fn emit_nvim(ctx: &EmitCtx<'_>) -> anyhow::Result<()> {
    let bundle = bundle_one(
        ctx.root,
        ctx.entry,
        "init.lua",
        ctx.edition,
        ctx.target,
        ctx.minify,
        ctx.sourcemap,
    )?;
    let plugin_root =
        modes::emit_nvim_plugin(ctx.out_dir, ctx.package_name, &bundle.text, ctx.description)?;
    if let Some(map) = &bundle.map {
        let map_path = plugin_root
            .join("lua")
            .join(ctx.package_name)
            .join("init.lua.map");
        fs::write(&map_path, map)
            .with_context(|| format!("cannot write `{}`", map_path.display()))?;
    }
    println!(
        "build: {} module(s) inlined into {} ({} -> {}){}{}, written as a Neovim plugin layout",
        bundle.modules,
        crate::project::display_rel(&plugin_root, ctx.root),
        ctx.edition.manifest_id(),
        ctx.target.manifest_id(),
        if ctx.minify { ", minified" } else { "" },
        if ctx.sourcemap {
            ", with sourcemap"
        } else {
            ""
        },
    );
    Ok(())
}

/// Render warn-tier lowering diagnostics like tree mode's (they never block
/// the bundle) and turn an error-tier one into a hard failure.
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
        bail!("build failed");
    }
    Ok(())
}

/// Parse the project's `luabox.toml`, when present and valid. `None` for
/// manifest-less directories or a manifest that fails to parse — the check
/// gate reports a parse failure loudly before it would matter here.
fn read_manifest(root: &Path) -> Option<Manifest> {
    let text = fs::read_to_string(root.join("luabox.toml")).ok()?;
    Manifest::parse(&text).ok()
}

// ---------------------------------------------------------------------------
// unmap — build's traceback decoder companion
// ---------------------------------------------------------------------------

/// Execute `luabox unmap <bundle> [traceback…]` from `cwd`: rewrite
/// `bundle.lua:NN` references in a traceback back to `module.lua:NN` via the
/// `<bundle>.map` emitted next to the bundle by `luabox build --sourcemap`.
/// The traceback comes from the arguments when present, stdin otherwise.
pub fn unmap(cwd: &Path, bundle: &Path, traceback: Option<&str>) -> anyhow::Result<()> {
    let bundle_path = if bundle.is_absolute() {
        bundle.to_path_buf()
    } else {
        cwd.join(bundle)
    };
    let map_path = PathBuf::from(format!("{}.map", bundle_path.display()));
    let map_text = fs::read_to_string(&map_path).with_context(|| {
        format!(
            "cannot read `{}` (build with `luabox build --sourcemap` to produce it)",
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

    // Every spelling a traceback may use for the bundle: the path as given
    // (both slash directions), its basename, and the map's own record of the
    // bundle name.
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
