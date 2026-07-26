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
    // Pin the usage-line name: without this clap derives it from argv[0],
    // which renders as `luabox.exe` on Windows and splits help output (and
    // everything asserting on it) across platforms.
    bin_name = "luabox",
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

#[cfg(test)]
mod tests {
    // test code — panics document assumptions
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use clap::CommandFactory as _;

    /// Parse an argv (without the leading program name) into a subcommand.
    fn parse(args: &[&str]) -> Command {
        let command_line: Vec<&str> = std::iter::once("luabox")
            .chain(args.iter().copied())
            .collect();
        Cli::try_parse_from(command_line)
            .unwrap_or_else(|e| panic!("`luabox {}` should parse: {e}", args.join(" ")))
            .command
    }

    /// Parse an argv expected to be rejected, returning clap's error kind.
    fn reject(args: &[&str]) -> clap::error::ErrorKind {
        let command_line: Vec<&str> = std::iter::once("luabox")
            .chain(args.iter().copied())
            .collect();
        Cli::try_parse_from(command_line)
            .err()
            .unwrap_or_else(|| panic!("`luabox {}` should be rejected", args.join(" ")))
            .kind()
    }

    #[test]
    fn the_cli_definition_is_internally_consistent() {
        // clap's own debug assertions catch conflicting/duplicated argument
        // definitions that only surface at runtime otherwise.
        Cli::command().debug_assert();
    }

    #[test]
    fn a_subcommand_is_required() {
        // Bare `luabox` prints help rather than doing anything.
        assert_eq!(
            reject(&[]),
            clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand,
            "bare `luabox` must not be a no-op"
        );
    }

    #[test]
    fn an_unknown_subcommand_is_rejected() {
        assert_eq!(
            reject(&["frobnicate"]),
            clap::error::ErrorKind::InvalidSubcommand
        );
    }

    // -- init / new --------------------------------------------------------

    #[test]
    fn init_defaults_to_a_binary_project_on_the_current_edition() {
        let Command::Init { lib, bin, edition } = parse(&["init"]) else {
            panic!("expected Init");
        };
        assert!(!lib);
        assert!(!bin);
        assert_eq!(edition, "5.4");
    }

    #[test]
    fn init_accepts_lib_bin_and_an_explicit_edition() {
        let Command::Init { lib, edition, .. } = parse(&["init", "--lib", "--edition", "luajit"])
        else {
            panic!("expected Init");
        };
        assert!(lib);
        assert_eq!(edition, "luajit");

        let Command::Init { bin, .. } = parse(&["init", "--bin"]) else {
            panic!("expected Init");
        };
        assert!(bin);
    }

    #[test]
    fn init_rejects_lib_and_bin_together() {
        assert_eq!(
            reject(&["init", "--lib", "--bin"]),
            clap::error::ErrorKind::ArgumentConflict
        );
    }

    #[test]
    fn new_requires_a_project_name() {
        assert_eq!(
            reject(&["new"]),
            clap::error::ErrorKind::MissingRequiredArgument
        );
    }

    #[test]
    fn new_takes_a_name_plus_the_same_flags_as_init() {
        let Command::New {
            name, lib, edition, ..
        } = parse(&["new", "mypkg", "--lib", "--edition", "5.1"])
        else {
            panic!("expected New");
        };
        assert_eq!(name, "mypkg");
        assert!(lib);
        assert_eq!(edition, "5.1");
    }

    #[test]
    fn new_rejects_lib_and_bin_together() {
        assert_eq!(
            reject(&["new", "mypkg", "--lib", "--bin"]),
            clap::error::ErrorKind::ArgumentConflict
        );
    }

    // -- check / lint / fmt ------------------------------------------------

    #[test]
    fn check_defaults_to_the_human_format_with_no_target_and_no_watch() {
        let Command::Check {
            target,
            format,
            watch,
        } = parse(&["check"])
        else {
            panic!("expected Check");
        };
        assert_eq!(target, None);
        assert_eq!(format, "human");
        assert!(!watch);
    }

    #[test]
    fn check_accepts_a_target_a_format_and_watch() {
        let Command::Check {
            target,
            format,
            watch,
        } = parse(&["check", "--target", "5.1", "--format", "json", "--watch"])
        else {
            panic!("expected Check");
        };
        assert_eq!(target.as_deref(), Some("5.1"));
        assert_eq!(format, "json");
        assert!(watch);
    }

    #[test]
    fn check_format_and_target_require_values() {
        assert_eq!(
            reject(&["check", "--format"]),
            clap::error::ErrorKind::InvalidValue
        );
        assert_eq!(
            reject(&["check", "--target"]),
            clap::error::ErrorKind::InvalidValue
        );
    }

    #[test]
    fn lint_takes_an_optional_fix_flag() {
        let Command::Lint { fix } = parse(&["lint"]) else {
            panic!("expected Lint");
        };
        assert!(!fix);

        let Command::Lint { fix } = parse(&["lint", "--fix"]) else {
            panic!("expected Lint");
        };
        assert!(fix);
    }

    #[test]
    fn fmt_composes_check_and_watch() {
        let Command::Fmt { check, watch } = parse(&["fmt"]) else {
            panic!("expected Fmt");
        };
        assert!(!check);
        assert!(!watch);

        let Command::Fmt { check, watch } = parse(&["fmt", "--check", "--watch"]) else {
            panic!("expected Fmt");
        };
        assert!(check);
        assert!(watch);
    }

    // -- build -------------------------------------------------------------

    #[test]
    fn build_defaults_every_knob_to_the_manifest() {
        let Command::Build {
            target,
            out,
            outfile,
            entry,
            bundle,
            no_bundle,
            sourcemap,
            minify,
            mode,
        } = parse(&["build"])
        else {
            panic!("expected Build");
        };
        assert_eq!(target, None);
        assert_eq!(out, None);
        assert_eq!(outfile, None);
        assert!(entry.is_empty());
        assert!(!bundle);
        assert!(!no_bundle);
        assert!(!sourcemap);
        assert!(!minify);
        assert_eq!(mode, None);
    }

    #[test]
    fn build_entry_is_repeatable_and_order_preserving() {
        let Command::Build { entry, .. } = parse(&[
            "build",
            "--entry",
            "src/cli.lua",
            "--entry",
            "src/worker.lua",
        ]) else {
            panic!("expected Build");
        };
        assert_eq!(
            entry,
            vec![
                PathBuf::from("src/cli.lua"),
                PathBuf::from("src/worker.lua")
            ]
        );
    }

    #[test]
    fn build_accepts_every_override_flag() {
        let Command::Build {
            target,
            out,
            outfile,
            sourcemap,
            minify,
            mode,
            ..
        } = parse(&[
            "build",
            "--target",
            "5.1",
            "--out",
            "build",
            "--outfile",
            "app.lua",
            "--sourcemap",
            "--minify",
            "--mode",
            "love",
        ])
        else {
            panic!("expected Build");
        };
        assert_eq!(target.as_deref(), Some("5.1"));
        assert_eq!(out, Some(PathBuf::from("build")));
        assert_eq!(outfile, Some(PathBuf::from("app.lua")));
        assert!(sourcemap);
        assert!(minify);
        assert_eq!(mode.as_deref(), Some("love"));
    }

    #[test]
    fn build_rejects_bundle_and_no_bundle_together() {
        assert_eq!(
            reject(&["build", "--bundle", "--no-bundle"]),
            clap::error::ErrorKind::ArgumentConflict
        );
    }

    #[test]
    fn build_accepts_either_bundle_flag_on_its_own() {
        let Command::Build {
            bundle, no_bundle, ..
        } = parse(&["build", "--bundle"])
        else {
            panic!("expected Build");
        };
        assert!(bundle);
        assert!(!no_bundle);

        let Command::Build {
            bundle, no_bundle, ..
        } = parse(&["build", "--no-bundle"])
        else {
            panic!("expected Build");
        };
        assert!(!bundle);
        assert!(no_bundle);
    }

    // -- doc / lsp / upgrade / explain -------------------------------------

    #[test]
    fn doc_takes_an_optional_open_flag() {
        let Command::Doc { open } = parse(&["doc"]) else {
            panic!("expected Doc");
        };
        assert!(!open);

        let Command::Doc { open } = parse(&["doc", "--open"]) else {
            panic!("expected Doc");
        };
        assert!(open);
    }

    #[test]
    fn lsp_accepts_the_editor_compatibility_stdio_flag() {
        let Command::Lsp { stdio } = parse(&["lsp"]) else {
            panic!("expected Lsp");
        };
        assert!(!stdio);

        let Command::Lsp { stdio } = parse(&["lsp", "--stdio"]) else {
            panic!("expected Lsp");
        };
        assert!(stdio);
    }

    #[test]
    fn upgrade_takes_an_optional_version() {
        let Command::Upgrade { version } = parse(&["upgrade"]) else {
            panic!("expected Upgrade");
        };
        assert_eq!(version, None);

        let Command::Upgrade { version } = parse(&["upgrade", "v0.1.0"]) else {
            panic!("expected Upgrade");
        };
        assert_eq!(version.as_deref(), Some("v0.1.0"));
    }

    #[test]
    fn explain_requires_a_diagnostic_code() {
        assert_eq!(
            reject(&["explain"]),
            clap::error::ErrorKind::MissingRequiredArgument
        );
        let Command::Explain { code } = parse(&["explain", "LB0421"]) else {
            panic!("expected Explain");
        };
        assert_eq!(code, "LB0421");
    }

    // -- unmap -------------------------------------------------------------

    #[test]
    fn unmap_requires_a_bundle_path_and_takes_the_traceback_from_stdin_by_default() {
        assert_eq!(
            reject(&["unmap"]),
            clap::error::ErrorKind::MissingRequiredArgument
        );
        let Command::Unmap { bundle, traceback } = parse(&["unmap", "dist/app.lua"]) else {
            panic!("expected Unmap");
        };
        assert_eq!(bundle, PathBuf::from("dist/app.lua"));
        assert!(traceback.is_empty());
    }

    #[test]
    fn unmap_collects_a_trailing_traceback_including_hyphen_leading_words() {
        // A Lua traceback is free text: `--` and `-e` inside it must not be
        // mistaken for flags (`trailing_var_arg` + `allow_hyphen_values`).
        let Command::Unmap { bundle, traceback } = parse(&[
            "unmap",
            "dist/app.lua",
            "stack",
            "traceback:",
            "-- in",
            "function",
        ]) else {
            panic!("expected Unmap");
        };
        assert_eq!(bundle, PathBuf::from("dist/app.lua"));
        assert_eq!(
            traceback,
            vec![
                "stack".to_owned(),
                "traceback:".to_owned(),
                "-- in".to_owned(),
                "function".to_owned()
            ]
        );
        // This is exactly what `main` joins back into one traceback string.
        assert_eq!(traceback.join(" "), "stack traceback: -- in function");
    }

    // -- help / version ----------------------------------------------------

    #[test]
    fn the_binary_reports_a_version() {
        assert_eq!(
            reject(&["--version"]),
            clap::error::ErrorKind::DisplayVersion
        );
    }

    #[test]
    fn help_describes_the_toolchain_and_its_lua_modules_contract() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("typechecker"), "{help}");
        assert!(help.contains("lua_modules/"), "{help}");
        assert!(help.contains("never runs Lua"), "{help}");
    }

    #[test]
    fn every_subcommand_is_reachable_and_documented() {
        let command = Cli::command();
        let names: Vec<&str> = command
            .get_subcommands()
            .map(clap::Command::get_name)
            .collect();
        for expected in [
            "init", "new", "check", "lint", "fmt", "build", "doc", "lsp", "upgrade", "explain",
            "unmap",
        ] {
            assert!(
                names.contains(&expected),
                "`{expected}` is missing: {names:?}"
            );
        }
        for sub in command.get_subcommands() {
            assert!(
                sub.get_about().is_some(),
                "`{}` has no help text",
                sub.get_name()
            );
        }
    }
}
