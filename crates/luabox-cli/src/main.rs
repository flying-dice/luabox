//! `luabox` — the unified Lua toolchain (SPEC.md §4).
//!
//! Thin frontend over the bounded-context crates: owns UX, argument parsing,
//! and diagnostic rendering; none of the domain logic.

mod auth_cmd;
mod build_cmd;
mod bundle_cmd;
mod check_cmd;
mod deps_cmd;
mod doc_cmd;
mod fmt_cmd;
mod github;
mod keychain;
mod lint_cmd;
mod lsp_cmd;
mod modes;
mod outdated_cmd;
mod project;
mod run_cmd;
mod runtime;
mod scaffold;
mod search_cmd;
mod toolchain_cmd;
mod upgrade_cmd;
mod watch;

use std::path::PathBuf;

use anyhow::bail;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "luabox",
    version,
    about = "Unified Lua toolchain: package manager, typechecker, linter, formatter, bundler, LSP. Not a runtime."
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
        /// Add as a path dependency rooted at this directory
        #[arg(long, conflicts_with_all = ["git", "url"])]
        path: Option<String>,
        /// Add as a git dependency at this URL
        #[arg(long, conflicts_with = "url")]
        git: Option<String>,
        /// Add as an http(s) tarball dependency (its sha256 is captured now)
        #[arg(long)]
        url: Option<String>,
        /// Pin the git dependency to a commit
        #[arg(long, requires = "git", conflicts_with_all = ["tag", "branch"])]
        rev: Option<String>,
        /// Pin the git dependency to a tag
        #[arg(long, requires = "git", conflicts_with = "branch")]
        tag: Option<String>,
        /// Track a branch of the git dependency
        #[arg(long, requires = "git")]
        branch: Option<String>,
    },
    /// Remove a dependency from luabox.toml
    Remove { package: String },
    /// Search luarocks.org (the registry) for rocks by name
    Search {
        /// Optional terms, matched as a case-insensitive substring of rock names
        query: Option<String>,
        /// Output format: text (default) or json
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Report dependencies behind their latest version (registry rocks vs.
    /// luarocks.org; git deps vs. their repo's latest GitHub release)
    Outdated {
        /// Output format: text (default) or json
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Sign in to GitHub via the browser (OAuth device flow); stores the token
    /// encrypted in the OS keychain
    Login {
        /// Output format: text (default) or json (newline-delimited events)
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Delete the stored GitHub token from the OS keychain
    Logout,
    /// Show the signed-in GitHub identity, if any
    Whoami {
        /// Output format: text (default) or json
        #[arg(long, default_value = "text")]
        format: String,
    },
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
        /// Embedding mode: plain (default), love, nvim-plugin; overrides
        /// `[build] mode`
        #[arg(long)]
        mode: Option<String>,
    },
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
    /// Start the language server (stdio)
    Lsp {
        /// Accepted for editor compatibility; stdio is the only transport.
        #[arg(long)]
        stdio: bool,
    },
    /// Manage Lua runtimes (install, pin, list)
    Toolchain {
        #[command(subcommand)]
        action: Option<ToolchainAction>,
    },
    /// Vendor dependencies into the source tree
    Vendor,
    /// Replace this binary with a GitHub release build (default: latest)
    Upgrade {
        /// Release version to install (e.g. 0.1.0 or v0.1.0); default: latest
        version: Option<String>,
    },
    /// Explain a diagnostic code (e.g. LB0421)
    Explain { code: String },
    /// Rewrite bundle line references in a traceback via its .lua.map
    Unmap {
        /// Path to the bundle; the map is read from `<bundle>.map`
        bundle: PathBuf,
        /// Traceback text (joined with spaces); read from stdin when omitted
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        traceback: Vec<String>,
    },
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
        Command::Add {
            package,
            dev,
            path,
            git,
            url,
            rev,
            tag,
            branch,
        } => deps_cmd::add(
            &std::env::current_dir()?,
            &deps_cmd::AddOptions {
                package,
                dev,
                path,
                git,
                url,
                rev,
                tag,
                branch,
            },
        ),
        Command::Remove { package } => deps_cmd::remove(&std::env::current_dir()?, &package),
        Command::Search { query, format } => search_cmd::run(query.as_deref(), &format),
        Command::Outdated { format } => outdated_cmd::run(&std::env::current_dir()?, &format),
        Command::Login { format } => auth_cmd::login(&format),
        Command::Logout => auth_cmd::logout(),
        Command::Whoami { format } => auth_cmd::whoami(&format),
        Command::Install => deps_cmd::install(&std::env::current_dir()?),
        Command::Update { package } => {
            deps_cmd::update(&std::env::current_dir()?, package.as_deref())
        }
        Command::Check {
            target,
            format,
            watch,
        } => check_cmd::run(&std::env::current_dir()?, target.as_deref(), &format, watch),
        Command::Lint { fix } => lint_cmd::run(&std::env::current_dir()?, fix),
        Command::Fmt { check, watch } => fmt_cmd::run(&std::env::current_dir()?, check, watch),
        Command::Build { target, out } => {
            build_cmd::run(&std::env::current_dir()?, target.as_deref(), out.as_deref())
        }
        Command::Bundle {
            minify,
            sourcemap,
            mode,
        } => bundle_cmd::run(
            &std::env::current_dir()?,
            minify,
            sourcemap,
            mode.as_deref(),
        ),
        Command::Run { script, args } => run_cmd::run(&std::env::current_dir()?, &script, &args),
        Command::Doc { open } => doc_cmd::run(&std::env::current_dir()?, open),
        Command::Lsp { .. } => lsp_cmd::run(),
        Command::Toolchain { action } => {
            let cwd = std::env::current_dir()?;
            match action {
                Some(ToolchainAction::Install { version }) => {
                    toolchain_cmd::install(&cwd, &version)
                }
                Some(ToolchainAction::Pin { version }) => toolchain_cmd::pin(&cwd, &version),
                Some(ToolchainAction::List) | None => toolchain_cmd::list(&cwd),
            }
        }
        Command::Upgrade { version } => upgrade_cmd::run(version),
        Command::Vendor => deps_cmd::vendor(&std::env::current_dir()?),
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
            bundle_cmd::unmap(&std::env::current_dir()?, &bundle, text.as_deref())
        }
    }
}
