//! `luabox build [--target <t>] [--out dir]` — lower + emit (SPEC.md §2.1,
//! §4, §18 P3).
//!
//! Pipeline:
//!
//! 1. **Discover** the project (nearest `luabox.toml`). The target dialect
//!    comes from `--target`, else `[build] target` (which defaults to the
//!    edition); the out directory from `--out`, else `[build] out`
//!    (default `dist`).
//! 2. **Check first** — the same per-file pipeline as `luabox check`
//!    (parse + *edition* dialect legality + typecheck). Build refuses to
//!    emit anything while check reports errors. Target-dialect legality is
//!    deliberately *not* part of this gate: constructs illegal on the
//!    target are exactly what lowering exists to handle.
//! 3. **Lower** each `.lua` file `edition → target` via `luabox-lower`.
//!    When `edition == target` the file is copied byte-identical (invariant
//!    covered by `features/emit/build.feature`). Hard `LB06xx` diagnostics
//!    fail the build; warn-tier ones are rendered and the build proceeds.
//! 4. **Residual validation** — every lowered output is reparsed and
//!    dialect-validated under the *target*; any residue (e.g. hex floats
//!    targeting 5.1, which have no lowering rule) fails the build rather
//!    than shipping a file the target runtime would reject.
//! 5. **Emit** to the out directory preserving relative paths. `.luab` shape
//!    files never reach the output (SHAPES.md §1 invariant 3 — they are
//!    not even candidates: only the `.lua` file list is emitted), and
//!    `*.d.lua` definition files are analyser-only surfaces, also skipped.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use luabox_diag::{Code, Diagnostic, Format, Label, Severity, Span, render};
use luabox_lower::LowerDiagnostic;
use luabox_syntax::{Dialect, lua};
use rayon::prelude::*;

use crate::check_cmd;

/// Execute `luabox build` from `cwd`.
pub fn run(cwd: &Path, target: Option<&str>, out: Option<&Path>) -> anyhow::Result<()> {
    let project = check_cmd::discover(cwd)?;

    let target = match target {
        Some(id) => match Dialect::from_manifest_id(id) {
            Some(dialect) => dialect,
            None => bail!(
                "unknown target `{id}`; expected one of: 5.1, 5.2, 5.3, 5.4, luajit \
                 (see `luabox explain LB1001`)"
            ),
        },
        None => project.build_target,
    };
    let out_dir: PathBuf = match out {
        Some(dir) if dir.is_absolute() => dir.to_path_buf(),
        Some(dir) => cwd.join(dir),
        None => project
            .out_dir
            .clone()
            .unwrap_or_else(|| project.root.join("dist")),
    };

    // Check gate: build refuses on check errors. `run_once` renders the
    // diagnostics itself, so the user sees exactly what `luabox check`
    // would print. The chosen out dir is passed through so previously
    // emitted output is never checked as project source.
    if check_cmd::run_once(cwd, None, "human", Some(&out_dir)).is_err() {
        bail!("`luabox build` refuses to emit while `luabox check` reports errors");
    }

    // Re-discover with the chosen out dir so the file walk skips previous
    // build output even when `--out` overrides the manifest.
    let mut project = check_cmd::discover(cwd)?;
    project.out_dir = Some(out_dir.clone());
    let (lua_files, _lb_files) = check_cmd::collect_files(&project)?;

    let edition = project.dialect;
    let results: Vec<anyhow::Result<Vec<Diagnostic>>> = lua_files
        .par_iter()
        .map(|path| {
            let rel = check_cmd::display_rel(path, &project.root);
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
    let root = project.root.clone();
    let lookup = move |file: &str| fs::read_to_string(root.join(file)).ok();
    let rendered = render(&diags, Format::Human, &lookup);
    if !rendered.is_empty() {
        println!("{rendered}");
    }

    let errors = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    if errors > 0 {
        bail!("build failed with {errors} error(s)");
    }
    let out_display = check_cmd::display_rel(&out_dir, &project.root);
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
/// no lowering, no reprint (SHAPES.md §1 invariant 1 keeps this
/// shape-blind by construction: `.luab` files play no part here).
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
