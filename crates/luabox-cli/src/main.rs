//! `luabox` — the unified Lua toolchain (SPEC.md §4).
//!
//! Thin frontend over the bounded-context crates: owns UX, argument parsing,
//! and diagnostic rendering; none of the domain logic.

mod check_cmd;
mod fmt_cmd;
mod scaffold;

use std::path::PathBuf;

use anyhow::bail;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "luabox",
    version,
    about = "Unified Lua toolchain: package manager, typechecker, linter, formatter, bundler, test runner, LSP. Not a runtime."
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
    /// Add a dependency to luabox.toml
    Add {
        /// Package spec: name[@version]
        package: String,
        /// Add to [dev-dependencies]
        #[arg(long)]
        dev: bool,
    },
    /// Remove a dependency from luabox.toml
    Remove { package: String },
    /// Resolve and fetch dependencies (lockfile-driven)
    Install,
    /// Update dependencies within manifest constraints
    Update { package: Option<String> },
    /// Typecheck the project
    Check {
        /// Also validate dialect legality against a ship target
        #[arg(long)]
        target: Option<String>,
        /// Output format: human, json, sarif, github, gitlab
        #[arg(long, default_value = "human")]
        format: String,
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
    },
    /// Lower to the configured target and emit
    Build {
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Emit a single-file bundle per entry point
    Bundle {
        #[arg(long)]
        minify: bool,
        #[arg(long)]
        sourcemap: bool,
    },
    /// Run tests on the configured runtime(s)
    Test {
        pattern: Option<String>,
        #[arg(long)]
        watch: bool,
        #[arg(long)]
        coverage: bool,
    },
    /// Run statistical benchmarks across runtimes
    Bench,
    /// Run a script or a [tasks] entry via the configured runtime
    Run {
        script: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Generate documentation from annotations
    Doc {
        #[arg(long)]
        open: bool,
    },
    /// Publish the package to the registry
    Publish,
    /// Start the language server (stdio)
    Lsp,
    /// Manage Lua runtimes (install, pin, list)
    Toolchain {
        #[command(subcommand)]
        action: Option<ToolchainAction>,
    },
    /// Vendor dependencies into the source tree
    Vendor,
    /// Check dependencies against the advisory database
    Audit,
    /// Explain a diagnostic code (e.g. LB0421)
    Explain { code: String },
}

#[derive(Subcommand)]
enum ToolchainAction {
    /// Install a runtime (e.g. 5.4.6, luajit-2.1)
    Install { version: String },
    /// Pin the project runtime
    Pin { version: String },
    /// List installed runtimes
    List,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init { lib, edition, .. } => {
            scaffold::init(&std::env::current_dir()?, lib, &edition)
        }
        Command::New {
            name, lib, edition, ..
        } => scaffold::new(&std::env::current_dir()?, &name, lib, &edition),
        Command::Add { .. } => unimplemented("add", "P2"),
        Command::Remove { .. } => unimplemented("remove", "P2"),
        Command::Install => unimplemented("install", "P2"),
        Command::Update { .. } => unimplemented("update", "P2"),
        Command::Check { target, format } => {
            check_cmd::run(&std::env::current_dir()?, target.as_deref(), &format)
        }
        Command::Lint { .. } => unimplemented("lint", "P1"),
        Command::Fmt { check } => fmt_cmd::run(&std::env::current_dir()?, check),
        Command::Build { .. } => unimplemented("build", "P3"),
        Command::Bundle { .. } => unimplemented("bundle", "P3"),
        Command::Test { .. } => unimplemented("test", "P4"),
        Command::Bench => unimplemented("bench", "P4"),
        Command::Run { .. } => unimplemented("run", "P4"),
        Command::Doc { .. } => unimplemented("doc", "P5"),
        Command::Publish => unimplemented("publish", "P2"),
        Command::Lsp => unimplemented("lsp", "P1"),
        Command::Toolchain { .. } => unimplemented("toolchain", "P4"),
        Command::Vendor => unimplemented("vendor", "P2"),
        Command::Audit => unimplemented("audit", "P5"),
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
    }
}

fn unimplemented(command: &str, phase: &str) -> anyhow::Result<()> {
    bail!("`luabox {command}` is not implemented yet — planned for {phase} (see SPEC.md §18)")
}
