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
use luabox_manifest::layout::{self, DefFiles};
use luabox_manifest::model::{Build, BundleMode, DEFAULT_OUT};
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
    /// `--mode`: embedding mode override. Already a valid mode by the time it
    /// gets here — clap rejects anything else (`crate::ModeArg`).
    pub mode: Option<BundleMode>,
}

/// Execute `luabox build` from `cwd`.
#[allow(
    clippy::too_many_lines,
    reason = "the effective-config resolution + output-rule validation reads as one linear pipeline"
)]
pub fn run(cwd: &Path, opts: &BuildOptions) -> anyhow::Result<()> {
    // One discovery for the whole command (CC-M11): `[build]`, the package
    // name and the description all ride on the `Project`, so nothing below
    // re-reads `luabox.toml`.
    let mut project = check_cmd::discover(cwd)?;
    let build_cfg: &Build = &project.build;

    let edition = project.dialect;
    let target = match opts.target.as_deref() {
        Some(id) => crate::dialect::parse("target", id)?,
        None => project.build_target,
    };
    let out_dir: PathBuf = match opts.out.as_deref() {
        Some(dir) if dir.is_absolute() => dir.to_path_buf(),
        Some(dir) => cwd.join(dir),
        None => project
            .out_dir
            .clone()
            .unwrap_or_else(|| project.root.join(DEFAULT_OUT)),
    };

    // Effective config: flags override `[build]`.
    let mode = opts.mode.unwrap_or(build_cfg.mode);
    let bundle_flag = opts.bundle.unwrap_or(build_cfg.bundle);
    // A non-`plain` mode packages a bundle, so it implies bundling even when
    // `bundle` is left false (the LÖVE / Neovim examples set only `mode`).
    let do_bundle = bundle_flag || mode != BundleMode::Plain;
    let sourcemap = opts.sourcemap || build_cfg.sourcemap;
    let minify = opts.minify || build_cfg.minify;
    let entry_specs: Vec<String> = if opts.entry.is_empty() {
        build_cfg.entry.clone()
    } else {
        opts.entry
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    };
    let outfile: Option<PathBuf> = opts
        .outfile
        .clone()
        .or_else(|| build_cfg.outfile.clone().map(PathBuf::from));

    // The chosen out dir is what the check gate and the file walk must skip,
    // even when `--out` overrides the manifest's.
    project.out_dir = Some(out_dir.clone());

    if !do_bundle {
        // Tree mode ignores the bundle-only knobs (`entry`, `outfile`,
        // `sourcemap`, `minify`) — there is no require graph to walk.
        check_gate(&project)?;
        return emit_tree(&project, &out_dir, edition, target);
    }

    // Bundle mode: the output-naming rules.
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
    if outfile.is_some() && mode != BundleMode::Plain {
        bail!(
            "`outfile` conflicts with `mode = \"{mode}\"`: that mode dictates its own output \
             layout (a `.love` archive / a Neovim plugin tree), so an output filename is \
             meaningless — drop one of them"
        );
    }
    if mode != BundleMode::Plain && entry_specs.len() != 1 {
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
                layout::display_rel(path, &project.root)
            );
        }
    }

    // Check gate, exactly as tree mode: refuse to emit on check errors.
    check_gate(&project)?;

    fs::create_dir_all(&out_dir)
        .with_context(|| format!("cannot create `{}`", out_dir.display()))?;

    // `[package] name` is optional (the rockspec is the package manifest,
    // SPEC.md §6), so a nameless project bundles as `bundle`.
    let package_name = if project.name.is_empty() {
        NAMELESS_BUNDLE
    } else {
        project.name.as_str()
    };
    let ctx = EmitCtx {
        root: &project.root,
        out_dir: &out_dir,
        edition,
        target,
        minify,
        sourcemap,
        package_name,
        description: project.description.as_deref(),
        entry: &entries[0],
    };

    // Exhaustive: a new `BundleMode` variant is a compile error here rather
    // than silently taking the plain arm (CC-M12).
    match mode {
        BundleMode::Love => emit_love(&ctx),
        BundleMode::NvimPlugin => emit_nvim(&ctx),
        BundleMode::Plain => emit_plain(
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

/// The bundle name a project with no `[package] name` gets.
const NAMELESS_BUNDLE: &str = "bundle";

/// `luabox build` runs `luabox check` first and refuses to emit while it
/// reports errors — the same gate for both emit shapes.
fn check_gate(project: &check_cmd::Project) -> anyhow::Result<()> {
    if check_cmd::run_once(project, None, Format::Human).is_err() {
        bail!("`luabox build` refuses to emit while `luabox check` reports errors");
    }
    Ok(())
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
fn emit_tree(
    project: &check_cmd::Project,
    out_dir: &Path,
    edition: Dialect,
    target: Dialect,
) -> anyhow::Result<()> {
    // `project.out_dir` is already the *chosen* out dir (`run` sets it before
    // the check gate), so the walk skips previous build output even when
    // `--out` overrides the manifest — without a second discovery.
    let lua_files =
        layout::collect_lua_files(&project.root, project.out_dir.as_deref(), DefFiles::Exclude)?;

    let results: Vec<anyhow::Result<Vec<Diagnostic>>> = lua_files
        .par_iter()
        .map(|path| {
            let rel = layout::display_rel(path, &project.root);
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
    let out_display = layout::display_rel(out_dir, &project.root);
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
                let code = Code::new(finding.code);
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
            let code = Code::new(d.code);
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
            layout::display_rel(&out_path, root),
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
        layout::display_rel(&love_path, ctx.root),
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
        layout::display_rel(&plugin_root, ctx.root),
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
            let code = Code::new(d.code);
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

#[cfg(test)]
mod tests {
    // test code — panics document assumptions
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::testutil::{project, read, write};

    /// `BuildOptions` with every override unset — the shape `luabox build`
    /// with no flags hands to [`run`].
    fn opts() -> BuildOptions {
        BuildOptions {
            target: None,
            out: None,
            outfile: None,
            entry: Vec::new(),
            bundle: None,
            sourcemap: false,
            minify: false,
            mode: None,
        }
    }

    // -- tree mode ---------------------------------------------------------

    #[test]
    fn tree_mode_copies_byte_identical_when_the_edition_equals_the_target() {
        let tmp = project("5.1", "\n[build]\ntarget = \"5.1\"\nout = \"dist\"\n");
        let source = "local x   =   1 -- odd spacing survives a copy\nprint(x)\n";
        write(tmp.path(), "src/main.lua", source);

        run(tmp.path(), &opts()).expect("build succeeds");
        assert_eq!(read(tmp.path(), "dist/src/main.lua"), source);
    }

    #[test]
    fn tree_mode_lowers_every_file_from_the_edition_to_the_target() {
        let tmp = project("5.4", "\n[build]\ntarget = \"5.1\"\nout = \"dist\"\n");
        write(
            tmp.path(),
            "src/main.lua",
            "local i = 0\n::top::\ni = i + 1\nif i < 3 then goto top end\nprint(i)\n",
        );

        run(tmp.path(), &opts()).expect("build succeeds");
        let emitted = read(tmp.path(), "dist/src/main.lua");
        assert!(!emitted.contains("goto"), "{emitted}");
        assert!(emitted.contains("repeat"), "{emitted}");
    }

    #[test]
    fn tree_mode_mirrors_the_source_layout_under_the_out_directory() {
        let tmp = project("5.4", "\n[build]\nout = \"dist\"\n");
        write(tmp.path(), "src/a.lua", "return 1\n");
        write(tmp.path(), "src/deep/b.lua", "return 2\n");
        write(tmp.path(), "top.lua", "return 3\n");

        run(tmp.path(), &opts()).expect("build succeeds");
        assert_eq!(read(tmp.path(), "dist/src/a.lua"), "return 1\n");
        assert_eq!(read(tmp.path(), "dist/src/deep/b.lua"), "return 2\n");
        assert_eq!(read(tmp.path(), "dist/top.lua"), "return 3\n");
    }

    #[test]
    fn tree_mode_skips_definition_files() {
        let tmp = project("5.4", "\n[build]\nout = \"dist\"\n");
        write(tmp.path(), "src/main.lua", "return 0\n");
        write(tmp.path(), "defs/love.d.lua", "---@meta\n");

        run(tmp.path(), &opts()).expect("build succeeds");
        assert!(!tmp.path().join("dist").join("defs").exists());
    }

    #[test]
    fn tree_mode_does_not_re_emit_its_own_previous_output() {
        let tmp = project("5.4", "\n[build]\nout = \"dist\"\n");
        write(tmp.path(), "src/main.lua", "return 0\n");

        run(tmp.path(), &opts()).expect("first build");
        run(tmp.path(), &opts()).expect("second build");
        // `dist/dist/...` would mean the walk re-consumed the output tree.
        assert!(!tmp.path().join("dist").join("dist").exists());
    }

    #[test]
    fn build_refuses_to_emit_while_check_reports_errors() {
        let tmp = project(
            "5.4",
            "\n[build]\ntarget = \"5.1\"\nout = \"dist\"\n[types]\nstrict = true\n",
        );
        write(
            tmp.path(),
            "src/main.lua",
            "---@param n number\nlocal function double(n)\n  return n * 2\nend\ndouble(\"nope\")\n",
        );

        let error = run(tmp.path(), &opts()).unwrap_err().to_string();
        assert!(
            error.contains("refuses to emit while `luabox check` reports errors"),
            "{error}"
        );
        assert!(
            !tmp.path()
                .join("dist")
                .join("src")
                .join("main.lua")
                .exists()
        );
    }

    #[test]
    fn an_irreducible_construct_is_a_hard_build_error_that_emits_nothing() {
        let tmp = project("5.4", "\n[build]\ntarget = \"5.1\"\nout = \"dist\"\n");
        write(
            tmp.path(),
            "src/main.lua",
            "while true do\n  goto out\nend\n::out::\nprint(\"after\")\n",
        );

        let error = run(tmp.path(), &opts()).unwrap_err().to_string();
        assert!(error.contains("build failed with"), "{error}");
        assert!(
            !tmp.path()
                .join("dist")
                .join("src")
                .join("main.lua")
                .exists()
        );
    }

    #[test]
    fn the_out_flag_overrides_the_manifest_out_directory() {
        let tmp = project("5.4", "\n[build]\nout = \"dist\"\n");
        write(tmp.path(), "src/main.lua", "return 0\n");

        let options = BuildOptions {
            out: Some(PathBuf::from("elsewhere")),
            ..opts()
        };
        run(tmp.path(), &options).expect("build succeeds");
        assert!(
            tmp.path()
                .join("elsewhere")
                .join("src")
                .join("main.lua")
                .is_file()
        );
        assert!(!tmp.path().join("dist").exists());
    }

    #[test]
    fn an_absolute_out_flag_is_used_verbatim() {
        let tmp = project("5.4", "");
        write(tmp.path(), "src/main.lua", "return 0\n");
        let out = tempfile::tempdir().expect("tempdir");

        let options = BuildOptions {
            out: Some(out.path().to_path_buf()),
            ..opts()
        };
        run(tmp.path(), &options).expect("build succeeds");
        assert!(out.path().join("src").join("main.lua").is_file());
    }

    #[test]
    fn the_target_flag_overrides_the_manifest_build_target() {
        let tmp = project("5.4", "\n[build]\ntarget = \"5.4\"\nout = \"dist\"\n");
        write(
            tmp.path(),
            "src/main.lua",
            "local i = 0\n::top::\ni = i + 1\nif i < 3 then goto top end\n",
        );

        let options = BuildOptions {
            target: Some("5.1".to_owned()),
            ..opts()
        };
        run(tmp.path(), &options).expect("build succeeds");
        assert!(!read(tmp.path(), "dist/src/main.lua").contains("goto"));
    }

    #[test]
    fn an_unknown_target_flag_is_rejected_listing_the_valid_dialects() {
        let tmp = project("5.4", "");
        write(tmp.path(), "src/main.lua", "return 0\n");
        let options = BuildOptions {
            target: Some("5.9".to_owned()),
            ..opts()
        };
        let error = run(tmp.path(), &options).unwrap_err().to_string();
        assert!(error.contains("unknown target `5.9`"), "{error}");
        assert!(error.contains("5.1, 5.2, 5.3, 5.4, luajit"), "{error}");
    }

    #[test]
    fn a_manifest_less_project_builds_with_the_default_config() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "src/main.lua", "return 0\n");
        run(tmp.path(), &opts()).expect("build succeeds");
        // `dist` is the manifest-less default out directory.
        assert!(
            tmp.path()
                .join("dist")
                .join("src")
                .join("main.lua")
                .is_file()
        );
    }

    // -- bundle mode, plain ------------------------------------------------

    #[test]
    fn bundle_mode_inlines_the_require_graph_into_one_file_named_from_the_entry() {
        let tmp = project("5.4", "\n[build]\nout = \"dist\"\nbundle = true\n");
        write(
            tmp.path(),
            "src/util.lua",
            "local M = {}\nfunction M.double(n) return n * 2 end\nreturn M\n",
        );
        write(
            tmp.path(),
            "src/main.lua",
            "local util = require(\"src.util\")\nprint(util.double(2))\n",
        );

        run(tmp.path(), &opts()).expect("build succeeds");
        let bundle = read(tmp.path(), "dist/main.lua");
        assert!(bundle.contains("M.double"), "{bundle}");
        // Tree mode did not also run.
        assert!(!tmp.path().join("dist").join("src").exists());
    }

    #[test]
    fn the_bundle_flag_overrides_a_manifest_that_says_otherwise() {
        let tmp = project("5.4", "\n[build]\nout = \"dist\"\n");
        write(tmp.path(), "src/main.lua", "return 0\n");

        let options = BuildOptions {
            bundle: Some(true),
            ..opts()
        };
        run(tmp.path(), &options).expect("build succeeds");
        assert!(tmp.path().join("dist").join("main.lua").is_file());
    }

    #[test]
    fn no_bundle_forces_tree_mode_even_when_the_manifest_asks_for_a_bundle() {
        let tmp = project("5.4", "\n[build]\nout = \"dist\"\nbundle = true\n");
        write(tmp.path(), "src/main.lua", "return 0\n");

        let options = BuildOptions {
            bundle: Some(false),
            ..opts()
        };
        run(tmp.path(), &options).expect("build succeeds");
        assert!(
            tmp.path()
                .join("dist")
                .join("src")
                .join("main.lua")
                .is_file()
        );
    }

    #[test]
    fn outfile_places_the_single_bundle_at_the_given_path() {
        let tmp = project("5.4", "\n[build]\nout = \"dist\"\nbundle = true\n");
        write(tmp.path(), "src/main.lua", "return 0\n");

        let options = BuildOptions {
            outfile: Some(PathBuf::from("out/app.lua")),
            ..opts()
        };
        run(tmp.path(), &options).expect("build succeeds");
        assert!(tmp.path().join("out").join("app.lua").is_file());
    }

    #[test]
    fn an_absolute_outfile_is_used_verbatim() {
        let tmp = project("5.4", "\n[build]\nbundle = true\n");
        write(tmp.path(), "src/main.lua", "return 0\n");
        let dest = tempfile::tempdir().expect("tempdir");
        let target = dest.path().join("nested").join("app.lua");

        let options = BuildOptions {
            outfile: Some(target.clone()),
            ..opts()
        };
        run(tmp.path(), &options).expect("build succeeds");
        assert!(target.is_file());
    }

    #[test]
    fn multiple_entries_each_produce_a_bundle_named_from_its_basename() {
        let tmp = project("5.4", "\n[build]\nout = \"dist\"\nbundle = true\n");
        write(tmp.path(), "src/cli.lua", "return 1\n");
        write(tmp.path(), "src/worker.lua", "return 2\n");

        let options = BuildOptions {
            entry: vec![
                PathBuf::from("src/cli.lua"),
                PathBuf::from("src/worker.lua"),
            ],
            ..opts()
        };
        run(tmp.path(), &options).expect("build succeeds");
        assert!(tmp.path().join("dist").join("cli.lua").is_file());
        assert!(tmp.path().join("dist").join("worker.lua").is_file());
    }

    #[test]
    fn sourcemap_writes_a_map_beside_each_bundle() {
        let tmp = project("5.4", "\n[build]\nout = \"dist\"\nbundle = true\n");
        write(tmp.path(), "src/util.lua", "return {}\n");
        write(
            tmp.path(),
            "src/main.lua",
            "local util = require(\"src.util\")\nreturn util\n",
        );

        let options = BuildOptions {
            sourcemap: true,
            ..opts()
        };
        run(tmp.path(), &options).expect("build succeeds");
        assert!(tmp.path().join("dist").join("main.lua.map").is_file());
    }

    #[test]
    fn minify_produces_a_smaller_bundle_than_the_unminified_one() {
        let tmp = project("5.4", "\n[build]\nout = \"dist\"\nbundle = true\n");
        write(
            tmp.path(),
            "src/main.lua",
            "local a_very_long_local_name = 1\nlocal another_long_name = 2\n\
             print(a_very_long_local_name + another_long_name)\n",
        );

        run(tmp.path(), &opts()).expect("plain build");
        let plain = read(tmp.path(), "dist/main.lua");

        let options = BuildOptions {
            minify: true,
            ..opts()
        };
        run(tmp.path(), &options).expect("minified build");
        let minified = read(tmp.path(), "dist/main.lua");
        assert!(minified.len() < plain.len(), "{minified}");
    }

    // -- bundle mode, output-rule validation -------------------------------

    #[test]
    fn bundling_without_any_entry_point_is_a_clear_error() {
        let tmp = project("5.4", "\n[build]\nbundle = true\nentry = []\n");
        write(tmp.path(), "src/main.lua", "return 0\n");
        let error = run(tmp.path(), &opts()).unwrap_err().to_string();
        assert!(
            error.contains("cannot bundle without an entry point"),
            "{error}"
        );
        assert!(error.contains("set `bundle = false`"), "{error}");
    }

    #[test]
    fn outfile_with_more_than_one_entry_is_rejected() {
        let tmp = project("5.4", "\n[build]\nbundle = true\n");
        write(tmp.path(), "src/a.lua", "return 1\n");
        write(tmp.path(), "src/b.lua", "return 2\n");

        let options = BuildOptions {
            outfile: Some(PathBuf::from("app.lua")),
            entry: vec![PathBuf::from("src/a.lua"), PathBuf::from("src/b.lua")],
            ..opts()
        };
        let error = run(tmp.path(), &options).unwrap_err().to_string();
        assert!(
            error.contains("`outfile` is valid only with exactly one entry point"),
            "{error}"
        );
        assert!(error.contains("but 2 are configured"), "{error}");
    }

    #[test]
    fn outfile_conflicts_with_a_mode_that_dictates_its_own_layout() {
        let tmp = project("5.4", "");
        write(tmp.path(), "src/main.lua", "return 0\n");

        let options = BuildOptions {
            outfile: Some(PathBuf::from("app.lua")),
            mode: Some(BundleMode::NvimPlugin),
            ..opts()
        };
        let error = run(tmp.path(), &options).unwrap_err().to_string();
        assert!(
            error.contains("`outfile` conflicts with `mode ="),
            "{error}"
        );
    }

    #[test]
    fn a_packaging_mode_rejects_more_than_one_entry_point() {
        let tmp = project("5.4", "");
        write(tmp.path(), "src/a.lua", "return 1\n");
        write(tmp.path(), "src/b.lua", "return 2\n");

        let options = BuildOptions {
            mode: Some(BundleMode::NvimPlugin),
            entry: vec![PathBuf::from("src/a.lua"), PathBuf::from("src/b.lua")],
            ..opts()
        };
        let error = run(tmp.path(), &options).unwrap_err().to_string();
        assert!(error.contains("packages a single entry point"), "{error}");
        assert!(error.contains("but 2 are configured"), "{error}");
    }

    #[test]
    fn a_missing_entry_point_names_the_path_it_looked_for() {
        let tmp = project("5.4", "\n[build]\nbundle = true\n");
        write(tmp.path(), "src/other.lua", "return 0\n");

        let error = run(tmp.path(), &opts()).unwrap_err().to_string();
        assert!(
            error.contains("bundle entry `src/main.lua` was not found"),
            "{error}"
        );
        assert!(error.contains("--entry"), "{error}");
    }

    #[test]
    fn bundle_mode_also_refuses_to_emit_while_check_reports_errors() {
        let tmp = project("5.4", "\n[build]\nbundle = true\nout = \"dist\"\n");
        write(tmp.path(), "src/main.lua", "local x = \n");
        let error = run(tmp.path(), &opts()).unwrap_err().to_string();
        assert!(
            error.contains("refuses to emit while `luabox check` reports errors"),
            "{error}"
        );
        assert!(!tmp.path().join("dist").join("main.lua").exists());
    }

    // -- packaging modes ---------------------------------------------------

    #[test]
    fn nvim_plugin_mode_writes_the_runtimepath_layout_under_the_package_name() {
        let tmp = project(
            "5.4",
            "description = \"a neovim plugin\"\n\n[build]\nout = \"dist\"\nmode = \"nvim-plugin\"\n",
        );
        write(
            tmp.path(),
            "src/main.lua",
            "return { setup = function() end }\n",
        );

        run(tmp.path(), &opts()).expect("build succeeds");
        let root = tmp.path().join("dist").join("fixture");
        assert!(root.join("lua").join("fixture").join("init.lua").is_file());
        assert!(root.join("plugin").join("fixture.lua").is_file());
        let doc = fs::read_to_string(root.join("doc").join("fixture.txt")).expect("doc stub");
        assert!(doc.contains("a neovim plugin"), "{doc}");
    }

    #[test]
    fn nvim_plugin_mode_writes_the_sourcemap_beside_init_lua() {
        let tmp = project("5.4", "\n[build]\nout = \"dist\"\nmode = \"nvim-plugin\"\n");
        write(tmp.path(), "src/util.lua", "return {}\n");
        write(
            tmp.path(),
            "src/main.lua",
            "local util = require(\"src.util\")\nreturn util\n",
        );

        let options = BuildOptions {
            sourcemap: true,
            ..opts()
        };
        run(tmp.path(), &options).expect("build succeeds");
        assert!(
            tmp.path()
                .join("dist")
                .join("fixture")
                .join("lua")
                .join("fixture")
                .join("init.lua.map")
                .is_file()
        );
    }

    #[test]
    fn a_packaging_mode_implies_bundling_even_with_bundle_left_false() {
        let tmp = project(
            "5.4",
            "\n[build]\nout = \"dist\"\nbundle = false\nmode = \"nvim-plugin\"\n",
        );
        write(tmp.path(), "src/main.lua", "return 0\n");
        run(tmp.path(), &opts()).expect("build succeeds");
        assert!(tmp.path().join("dist").join("fixture").is_dir());
        // Tree mode never ran.
        assert!(!tmp.path().join("dist").join("src").exists());
    }

    #[test]
    fn love_mode_packages_the_bundle_into_a_love_archive() {
        let tmp = project("5.4", "\n[build]\nout = \"dist\"\nmode = \"love\"\n");
        write(
            tmp.path(),
            "src/main.lua",
            "function love.draw() end\nreturn 0\n",
        );
        write(tmp.path(), "src/conf.lua", "return 0\n");
        write(tmp.path(), "assets/sprite.txt", "pixels\n");

        run(tmp.path(), &opts()).expect("build succeeds");
        let archive = tmp.path().join("dist").join("fixture.love");
        assert!(archive.is_file(), "expected `{}`", archive.display());
        // A zip archive always starts with the local file header magic.
        let bytes = fs::read(&archive).expect("read archive");
        assert_eq!(&bytes[..2], b"PK", "not a zip archive");
    }

    // -- effective configuration -------------------------------------------

    #[test]
    fn a_manifest_less_project_builds_with_the_manifest_s_own_defaults() {
        // `build` no longer keeps its own copy of the `[build]` fallback:
        // discovery hands it `Build::defaults`, the same constructor
        // `Manifest::parse` uses when a manifest has no `[build]` table, so
        // the two cannot drift (CC-M11).
        let tmp = tempfile::tempdir().expect("tempdir");
        let build = check_cmd::discover(tmp.path()).expect("discovers").build;
        assert_eq!(
            build,
            Build::defaults(luabox_manifest::model::DialectId::Lua54)
        );
        assert_eq!(build.out, "dist");
        assert_eq!(build.mode, BundleMode::Plain);
    }

    #[test]
    fn a_relative_entry_spec_resolves_against_the_project_root() {
        let root = Path::new("/proj");
        assert_eq!(
            resolve_entry(root, "src/main.lua"),
            root.join("src/main.lua")
        );
    }

    #[test]
    fn an_absolute_entry_spec_is_taken_as_is() {
        let absolute = if cfg!(windows) {
            "C:\\elsewhere\\main.lua"
        } else {
            "/elsewhere/main.lua"
        };
        assert_eq!(
            resolve_entry(Path::new("/proj"), absolute),
            PathBuf::from(absolute)
        );
    }

    #[test]
    fn a_malformed_manifest_fails_the_build_rather_than_silently_defaulting() {
        // `build` had a second manifest read (`read_manifest`) that swallowed
        // a parse failure and fell back to the `[build]` defaults; discovery
        // already reported it first, so deleting that read cannot change what
        // a user sees here — this pins that it did not.
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", "= = =\n");
        let error = run(tmp.path(), &opts()).unwrap_err().to_string();
        assert!(error.starts_with("invalid `"), "{error}");
    }

    #[test]
    fn discovery_is_the_only_manifest_read_on_the_build_path() {
        let tmp = project("5.3", "\n[build]\nmode = \"love\"\n");
        let project = check_cmd::discover(tmp.path()).expect("discovers");
        assert_eq!(project.name, "fixture");
        assert_eq!(project.dialect, Dialect::Lua53);
        assert_eq!(project.build.mode, BundleMode::Love);
    }

    #[test]
    fn a_manifest_less_bundle_is_named_bundle_in_packaging_modes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "src/main.lua", "return 0\n");

        let options = BuildOptions {
            mode: Some(BundleMode::NvimPlugin),
            ..opts()
        };
        run(tmp.path(), &options).expect("build succeeds");
        assert!(tmp.path().join("dist").join("bundle").is_dir());
    }

    // -- lowering ----------------------------------------------------------

    #[test]
    fn lowering_a_file_to_its_own_dialect_is_a_verbatim_copy_with_no_diagnostics() {
        let source = "local x   =  1 -- kept\n";
        let (output, diags) = lower_one(source, "src/main.lua", Dialect::Lua54, Dialect::Lua54);
        assert_eq!(output.as_deref(), Some(source));
        assert!(diags.is_empty());
    }

    #[test]
    fn lowering_rewrites_constructs_the_target_does_not_have() {
        let (output, _diags) = lower_one(
            "local mask = 5\nprint(mask & 3)\n",
            "src/main.lua",
            Dialect::Lua54,
            Dialect::Lua51,
        );
        let text = output.expect("lowers");
        assert!(text.contains("__luabox_rt.band"), "{text}");
        assert!(!text.contains(" & "), "{text}");
    }

    #[test]
    fn an_irreducible_construct_yields_no_output_and_an_error_diagnostic() {
        let (output, diags) = lower_one(
            "while true do\n  goto out\nend\n::out::\n",
            "src/main.lua",
            Dialect::Lua54,
            Dialect::Lua51,
        );
        assert!(output.is_none());
        assert!(
            diags
                .iter()
                .any(|d| d.severity == luabox_diag::Severity::Error),
            "{diags:?}"
        );
        // Every diagnostic is anchored at the file it came from.
        for diag in &diags {
            assert_eq!(
                diag.primary_label().map(|l| l.span.file.as_str()),
                Some("src/main.lua")
            );
        }
    }

    /// Lua 5.3+ floor division: lowering it to 5.1 is legal but warns about
    /// the integer-semantics divergence, so it is the fixture for the
    /// warn-tier path through both emit shapes.
    const FLOOR_DIV: &str = "local x = 7 // 2\nprint(x)\n";

    #[test]
    fn a_warn_tier_lowering_diagnostic_does_not_block_tree_mode_emit() {
        let tmp = project("5.4", "\n[build]\ntarget = \"5.1\"\nout = \"dist\"\n");
        write(tmp.path(), "src/main.lua", FLOOR_DIV);

        run(tmp.path(), &opts()).expect("a warning must not fail the build");
        let emitted = read(tmp.path(), "dist/src/main.lua");
        assert!(emitted.contains("math.floor"), "{emitted}");
    }

    #[test]
    fn a_warn_tier_lowering_diagnostic_does_not_block_a_bundle() {
        let tmp = project(
            "5.4",
            "\n[build]\ntarget = \"5.1\"\nout = \"dist\"\nbundle = true\n",
        );
        write(tmp.path(), "src/main.lua", FLOOR_DIV);

        run(tmp.path(), &opts()).expect("a warning must not fail the bundle");
        assert!(read(tmp.path(), "dist/main.lua").contains("math.floor"));
    }

    #[test]
    fn a_warn_tier_lowering_diagnostic_maps_to_a_warning_not_an_error() {
        let lowered = luabox_lower::lower(FLOOR_DIV, Dialect::Lua54, Dialect::Lua51)
            .expect("floor division lowers");
        // One warning, about the one `//` in the fixture.
        assert_eq!(lowered.warnings.len(), 1, "{:?}", lowered.warnings);
        let diags = to_diagnostics(&lowered.warnings, "src/main.lua");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, luabox_diag::Severity::Warning);
        assert_eq!(diags[0].code, Code::new(lowered.warnings[0].code));
        assert_eq!(diags[0].message, lowered.warnings[0].message);
    }

    #[test]
    fn a_lowering_warning_is_reported_alongside_the_emitted_file() {
        let (output, diags) = lower_one(FLOOR_DIV, "src/main.lua", Dialect::Lua54, Dialect::Lua51);
        // Output *and* diagnostics: warnings never suppress the emit, and the
        // emitted text is the lowered form rather than the input echoed back.
        let output = output.expect("a warn-tier lowering still emits");
        assert!(output.contains("math.floor"), "{output}");
        assert!(!output.contains("//"), "{output}");
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].severity, luabox_diag::Severity::Warning);
        assert_eq!(
            diags[0].primary_label().map(|l| l.span.file.as_str()),
            Some("src/main.lua")
        );
    }

    #[test]
    fn minify_composes_with_the_packaging_modes() {
        let tmp = project("5.4", "\n[build]\nout = \"dist\"\nmode = \"nvim-plugin\"\n");
        write(
            tmp.path(),
            "src/main.lua",
            "local a_very_long_local_name = 1\nreturn a_very_long_local_name\n",
        );

        run(tmp.path(), &opts()).expect("plain build");
        let plain = read(tmp.path(), "dist/fixture/lua/fixture/init.lua");

        let options = BuildOptions {
            minify: true,
            ..opts()
        };
        run(tmp.path(), &options).expect("minified build");
        let minified = read(tmp.path(), "dist/fixture/lua/fixture/init.lua");
        assert!(minified.len() < plain.len(), "{minified}");
    }

    #[test]
    fn an_empty_package_name_falls_back_to_bundle() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "luabox.toml",
            "[package]\nname = \"\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n[build]\nout = \"dist\"\nmode = \"nvim-plugin\"\n",
        );
        write(tmp.path(), "src/main.lua", "return 0\n");

        run(tmp.path(), &opts()).expect("build succeeds");
        assert!(tmp.path().join("dist").join("bundle").is_dir());
    }

    #[test]
    fn lower_diagnostics_carry_the_file_relative_span_and_severity() {
        let lowered = luabox_lower::lower(
            "while true do\n  goto out\nend\n::out::\n",
            Dialect::Lua54,
            Dialect::Lua51,
        );
        let raw = lowered.expect_err("this program cannot be lowered");
        let diags = to_diagnostics(&raw, "src/main.lua");
        assert_eq!(diags.len(), raw.len());
        for diag in &diags {
            let label = diag.primary_label().expect("has a primary label");
            assert_eq!(label.span.file, "src/main.lua");
            assert_eq!(label.message, "lowered here");
        }
    }

    // -- unmap -------------------------------------------------------------

    /// Build a bundle with `--sourcemap` and return the project tempdir.
    fn project_with_sourcemapped_bundle() -> tempfile::TempDir {
        let tmp = project("5.4", "\n[build]\nout = \"dist\"\nbundle = true\n");
        write(
            tmp.path(),
            "src/util.lua",
            "local M = {}\nfunction M.boom() error(\"nope\") end\nreturn M\n",
        );
        write(
            tmp.path(),
            "src/main.lua",
            "local util = require(\"src.util\")\nutil.boom()\n",
        );
        let options = BuildOptions {
            sourcemap: true,
            ..opts()
        };
        run(tmp.path(), &options).expect("build succeeds");
        tmp
    }

    #[test]
    fn unmap_decodes_a_traceback_against_the_map_beside_the_bundle() {
        let tmp = project_with_sourcemapped_bundle();
        unmap(
            tmp.path(),
            Path::new("dist/main.lua"),
            Some("dist/main.lua:12: something went wrong"),
        )
        .expect("unmap succeeds");
    }

    #[test]
    fn unmap_accepts_an_absolute_bundle_path() {
        let tmp = project_with_sourcemapped_bundle();
        let bundle = tmp.path().join("dist").join("main.lua");
        unmap(tmp.path(), &bundle, Some("main.lua:3: boom")).expect("unmap succeeds");
    }

    #[test]
    fn unmap_handles_a_traceback_without_a_trailing_newline() {
        let tmp = project_with_sourcemapped_bundle();
        unmap(
            tmp.path(),
            Path::new("dist/main.lua"),
            Some("main.lua:1: x"),
        )
        .expect("unmap succeeds");
        unmap(
            tmp.path(),
            Path::new("dist/main.lua"),
            Some("main.lua:1: x\n"),
        )
        .expect("unmap succeeds");
    }

    #[test]
    fn unmap_without_a_map_tells_the_user_how_to_produce_one() {
        let tmp = project("5.4", "\n[build]\nout = \"dist\"\nbundle = true\n");
        write(tmp.path(), "src/main.lua", "return 0\n");
        run(tmp.path(), &opts()).expect("build without a sourcemap");

        let error = unmap(tmp.path(), Path::new("dist/main.lua"), Some("boom")).unwrap_err();
        let rendered = format!("{error:#}");
        assert!(rendered.contains("cannot read"), "{rendered}");
        assert!(rendered.contains("--sourcemap"), "{rendered}");
    }

    #[test]
    fn unmap_rejects_a_map_that_is_not_valid_json() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "dist/main.lua", "return 0\n");
        write(tmp.path(), "dist/main.lua.map", "not json at all");

        // The map was found and rejected as JSON — a different failure from
        // the missing-map one above, and it says so.
        let error = unmap(tmp.path(), Path::new("dist/main.lua"), Some("boom")).unwrap_err();
        let rendered = format!("{error:#}");
        assert!(rendered.contains("invalid .lua.map"), "{rendered}");
        assert!(!rendered.contains("--sourcemap"), "{rendered}");
    }
}
