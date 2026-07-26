//! `luabox` — the unified Lua toolchain (SPEC.md §4).
//!
//! Thin frontend over the bounded-context crates: owns UX, argument parsing,
//! and diagnostic rendering; none of the domain logic.

mod build_cmd;
mod check_cmd;
mod doc_cmd;
mod fmt_cmd;
mod lint_cmd;
mod lsp_cmd;
mod modes;
mod project;
mod scaffold;
mod upgrade_cmd;
mod watch;

use std::path::PathBuf;

use anyhow::bail;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "luabox",
    version,
    about = "Unified static Lua toolchain: typechecker, linter, formatter, bundler, LSP. \
             Consumes a `lua_modules/` rock tree — it never fetches one, and never runs Lua."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scaffold a project in the current directory
    Init {
        /// Scaffold a library (default is a binary/script project)
        #[arg(long, conflicts_with = "bin")]
        lib: bool,
        /// Scaffold a binary/script project
        #[arg(long)]
        bin: bool,
        /// Dialect you write: 5.1, 5.2, 5.3, 5.4, luajit
        #[arg(long, default_value = "5.4")]
        edition: String,
    },
    /// Scaffold a new project in a new directory
    New {
        name: String,
        #[arg(long, conflicts_with = "bin")]
        lib: bool,
        #[arg(long)]
        bin: bool,
        #[arg(long, default_value = "5.4")]
        edition: String,
    },
    /// Typecheck the project
    Check {
        /// Also validate dialect legality against a ship target
        #[arg(long)]
        target: Option<String>,
        /// Output format: human, json, sarif, github, gitlab
        #[arg(long, default_value = "human")]
        format: String,
        /// Rerun on every source/manifest change until interrupted (Ctrl-C);
        /// a failing run is reported but does not stop watching
        #[arg(long)]
        watch: bool,
    },
    /// Lint the project
    Lint {
        /// Apply machine-applicable fixes
        #[arg(long)]
        fix: bool,
    },
    /// Format Lua sources canonically
    Fmt {
        /// Fail (without writing) if any file is not already formatted
        #[arg(long)]
        check: bool,
        /// Rerun on every source/manifest change until interrupted (Ctrl-C);
        /// a failing run is reported but does not stop watching
        #[arg(long)]
        watch: bool,
    },
    /// Lower to the configured target and emit — tsc/esbuild-style, driven
    /// by `[build]` config (flags override every field)
    Build {
        /// Dialect to lower to (default: `[build] target`, else the edition)
        #[arg(long)]
        target: Option<String>,
        /// Output directory for tree-mode emit and multi-entry bundles
        #[arg(long)]
        out: Option<PathBuf>,
        /// Single-entry bundle output path (illegal with multiple entries)
        #[arg(long)]
        outfile: Option<PathBuf>,
        /// Bundle entry point (repeatable); overrides `[build] entry`
        #[arg(long = "entry")]
        entry: Vec<PathBuf>,
        /// Emit a single-file bundle per entry (overrides `[build] bundle`)
        #[arg(long, conflicts_with = "no_bundle")]
        bundle: bool,
        /// Force tree-mode emit even if `[build] bundle = true`
        #[arg(long = "no-bundle")]
        no_bundle: bool,
        /// Emit a `.map` beside each bundle for `luabox unmap`
        #[arg(long)]
        sourcemap: bool,
        /// Mangle locals/whitespace in each bundle
        #[arg(long)]
        minify: bool,
        /// Embedding mode: plain (default), love, nvim-plugin; overrides
        /// `[build] mode`
        #[arg(long)]
        mode: Option<String>,
    },
    /// Generate documentation from annotations
    Doc {
        #[arg(long)]
        open: bool,
    },
    /// Start the language server (stdio)
    Lsp {
        /// Accepted for editor compatibility; stdio is the only transport.
        #[arg(long)]
        stdio: bool,
    },
    /// Replace this binary with a GitHub release build (default: latest)
    Upgrade {
        /// Release version to install (e.g. 0.1.0 or v0.1.0); default: latest
        version: Option<String>,
    },
    /// Explain a diagnostic code (e.g. LB0421)
    Explain { code: String },
    /// Rewrite bundle line references in a traceback back to source, via the
    /// `<bundle>.map` emitted next to the bundle by `luabox build --sourcemap`
    Unmap {
        /// Path to the bundle; the map is read from `<bundle>.map` beside it
        bundle: PathBuf,
        /// Traceback text (joined with spaces); read from stdin when omitted
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        traceback: Vec<String>,
    },
}

// A pure one-arm-per-subcommand dispatcher: length tracks the CLI surface,
// not complexity.
#[allow(clippy::too_many_lines)]
fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init { lib, edition, .. } => {
            scaffold::init(&std::env::current_dir()?, lib, &edition)
        }
        Command::New {
            name, lib, edition, ..
        } => scaffold::new(&std::env::current_dir()?, &name, lib, &edition),
        Command::Check {
            target,
            format,
            watch,
        } => check_cmd::run(&std::env::current_dir()?, target.as_deref(), &format, watch),
        Command::Lint { fix } => lint_cmd::run(&std::env::current_dir()?, fix),
        Command::Fmt { check, watch } => fmt_cmd::run(&std::env::current_dir()?, check, watch),
        Command::Build {
            target,
            out,
            outfile,
            entry,
            bundle,
            no_bundle,
            sourcemap,
            minify,
            mode,
        } => {
            let bundle = if no_bundle {
                Some(false)
            } else if bundle {
                Some(true)
            } else {
                None
            };
            build_cmd::run(
                &std::env::current_dir()?,
                &build_cmd::BuildOptions {
                    target,
                    out,
                    outfile,
                    entry,
                    bundle,
                    sourcemap,
                    minify,
                    mode,
                },
            )
        }
        Command::Doc { open } => doc_cmd::run(&std::env::current_dir()?, open),
        Command::Lsp { .. } => lsp_cmd::run(),
        Command::Upgrade { version } => upgrade_cmd::run(version),
        Command::Explain { code } => {
            let parsed: luabox_diag::Code = code.parse().map_err(|_| {
                anyhow::anyhow!("`{code}` is not a valid diagnostic code; codes look like LB0421")
            })?;
            match luabox_diag::explain(&parsed) {
                Some(entry) => {
                    println!("{}: {}\n\n{}", entry.code, entry.title, entry.explain);
                    Ok(())
                }
                None => bail!("no such diagnostic code `{parsed}`; codes look like LB0421"),
            }
        }
        Command::Unmap { bundle, traceback } => {
            let text = if traceback.is_empty() {
                None
            } else {
                Some(traceback.join(" "))
            };
            build_cmd::unmap(&std::env::current_dir()?, &bundle, text.as_deref())
        }
    }
}
