//! `luabox` — the unified Lua toolchain (SPEC.md §4).
//!
//! Thin frontend over the bounded-context crates: owns UX, argument parsing,
//! and diagnostic rendering; none of the domain logic.

mod build_cmd;
mod check_cmd;
mod dialect;
mod doc_cmd;
mod fmt_cmd;
mod lint_cmd;
mod lsp_cmd;
mod modes;
mod project;
mod scaffold;
#[cfg(test)]
mod testutil;
mod upgrade_cmd;
mod watch;

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::bail;
use clap::{Parser, Subcommand, ValueEnum};
use luabox_diag::Format;
use luabox_manifest::model::BundleMode;

/// `--format`: the closed set of diagnostic renderings (SPEC.md §14).
///
/// A CLI-side mirror of [`luabox_diag::Format`], not that type itself: clap's
/// `ValueEnum` owns the spellings and the "possible values" help/completions,
/// while `luabox-diag` stays free of a `clap` dependency. Hand-rolled
/// `parse_format` string matching died with it (CC-M12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum FormatArg {
    Human,
    Json,
    Sarif,
    Github,
    Gitlab,
}

impl From<FormatArg> for Format {
    fn from(arg: FormatArg) -> Self {
        match arg {
            FormatArg::Human => Format::Human,
            FormatArg::Json => Format::Json,
            FormatArg::Sarif => Format::Sarif,
            FormatArg::Github => Format::GithubActions,
            FormatArg::Gitlab => Format::GitlabCodeQuality,
        }
    }
}

/// `--mode`: the closed set of bundler embedding modes (SPEC.md §7).
///
/// The same CLI-side-mirror trick as [`FormatArg`]: clap validates the flag,
/// and this maps onto the manifest's [`BundleMode`] so a flag-supplied mode
/// and a `[build] mode` are the same value by the time `build_cmd` sees them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ModeArg {
    Plain,
    Love,
    #[value(name = "nvim-plugin")]
    NvimPlugin,
}

impl From<ModeArg> for BundleMode {
    fn from(arg: ModeArg) -> Self {
        match arg {
            ModeArg::Plain => BundleMode::Plain,
            ModeArg::Love => BundleMode::Love,
            ModeArg::NvimPlugin => BundleMode::NvimPlugin,
        }
    }
}

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
        /// Scaffold a binary/script project — the default, so passing it
        /// only makes that explicit
        #[arg(long)]
        bin: bool,
        /// Dialect you write: 5.1, 5.2, 5.3, 5.4, luajit
        #[arg(long, default_value = "5.4")]
        edition: String,
    },
    /// Scaffold a new project in a new directory
    New {
        name: String,
        /// Scaffold a library (default is a binary/script project)
        #[arg(long, conflicts_with = "bin")]
        lib: bool,
        /// Scaffold a binary/script project — the default, so passing it
        /// only makes that explicit
        #[arg(long)]
        bin: bool,
        /// Dialect you write: 5.1, 5.2, 5.3, 5.4, luajit
        #[arg(long, default_value = "5.4")]
        edition: String,
    },
    /// Typecheck the project
    Check {
        /// Also validate dialect legality against a ship target
        #[arg(long)]
        target: Option<String>,
        /// Output format
        #[arg(long, value_enum, default_value_t = FormatArg::Human)]
        format: FormatArg,
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
        /// Embedding mode (default: `[build] mode`, else plain); overrides
        /// `[build] mode`
        #[arg(long, value_enum)]
        mode: Option<ModeArg>,
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
    /// Explain a diagnostic code (e.g. LB0300)
    Explain { code: String },
    /// Print the JSON Schema (draft 2020-12) for `luabox.toml`, for editors,
    /// validators and LLM coding assistants
    Schema,
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

/// Exit codes are part of the CLI contract (SPEC.md §14): 0 on success, 1
/// when a command ran and failed, 2 when clap rejects the invocation — that
/// last path is clap's own, taken inside `Cli::parse` before `run` is
/// reached, and is untouched here.
///
/// `main` returns [`ExitCode`] rather than `anyhow::Result<()>` deliberately.
/// The `Result` return renders the error with `anyhow`'s `Debug`, which
/// appends a captured stack backtrace to every failure whenever
/// `RUST_BACKTRACE` is set in the user's environment — and release builds are
/// stripped (`Cargo.toml`'s `[profile.release] strip = true`), so those frames
/// arrive as pages of `<unknown>` telling the user nothing about their
/// manifest or sources. Rendering the chain here keeps the diagnostic and
/// drops the noise.
fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprint!("{}", render_error(&error));
            ExitCode::FAILURE
        }
    }
}

/// The error chain as `anyhow`'s `Debug` renders it — top-level message, then
/// a `Caused by:` block (numbered once there is more than one source) — minus
/// the backtrace.
fn render_error(error: &anyhow::Error) -> String {
    let mut out = format!("Error: {error}\n");
    let sources: Vec<String> = error.chain().skip(1).map(ToString::to_string).collect();
    if sources.is_empty() {
        return out;
    }
    out.push_str("\nCaused by:\n");
    let numbered = sources.len() > 1;
    for (index, source) in sources.iter().enumerate() {
        let lead = if numbered {
            format!("{index:>4}: ")
        } else {
            "    ".to_owned()
        };
        let continuation = " ".repeat(lead.len());
        for (line_number, line) in source.lines().enumerate() {
            let prefix = if line_number == 0 {
                &lead
            } else {
                &continuation
            };
            let _ = writeln!(out, "{prefix}{line}");
        }
    }
    out
}

// A pure one-arm-per-subcommand dispatcher: length tracks the CLI surface,
// not complexity.
#[allow(clippy::too_many_lines)]
fn run(command: Command) -> anyhow::Result<()> {
    match command {
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
        } => check_cmd::run(
            &std::env::current_dir()?,
            target.as_deref(),
            format.into(),
            watch,
        ),
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
                    mode: mode.map(Into::into),
                },
            )
        }
        Command::Doc { open } => doc_cmd::run(&std::env::current_dir()?, open),
        Command::Lsp { .. } => lsp_cmd::run(),
        Command::Upgrade { version } => upgrade_cmd::run(version),
        Command::Explain { code } => {
            let parsed: luabox_diag::Code = code.parse().map_err(|_| {
                anyhow::anyhow!("`{code}` is not a valid diagnostic code; codes look like LB0300")
            })?;
            match luabox_diag::explain(&parsed) {
                Some(entry) => {
                    println!("{}: {}\n\n{}", entry.code, entry.title, entry.explain);
                    Ok(())
                }
                None => bail!("no such diagnostic code `{parsed}`; codes look like LB0300"),
            }
        }
        // No project, no filesystem, no flags: the schema is embedded in the
        // binary, so this is a `cat` of a compile-time constant. That is the
        // point — `luabox schema > luabox.schema.json` has to work anywhere,
        // including in a directory that has no manifest to describe yet.
        Command::Schema => {
            println!("{}", luabox_manifest::schema::json_schema().trim_end());
            Ok(())
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

    // -- error rendering ---------------------------------------------------

    #[test]
    fn a_bare_error_renders_as_one_error_line() {
        let rendered = render_error(&anyhow::anyhow!("no such diagnostic code `LB9999`"));
        assert_eq!(rendered, "Error: no such diagnostic code `LB9999`\n");
    }

    #[test]
    fn a_single_source_renders_an_indented_caused_by_block() {
        let error = anyhow::anyhow!("no such file").context("cannot read `luabox.toml`");
        assert_eq!(
            render_error(&error),
            "Error: cannot read `luabox.toml`\n\nCaused by:\n    no such file\n"
        );
    }

    #[test]
    fn several_sources_are_numbered_outermost_first() {
        let error = anyhow::anyhow!("connection refused")
            .context("downloading SHA256SUMS")
            .context("upgrading to v0.2.0");
        assert_eq!(
            render_error(&error),
            "Error: upgrading to v0.2.0\n\nCaused by:\n   0: downloading SHA256SUMS\n   1: connection refused\n"
        );
    }

    #[test]
    fn a_multi_line_source_keeps_its_lines_under_the_same_indent() {
        let error = anyhow::anyhow!("first\nsecond").context("invalid `luabox.toml`");
        assert_eq!(
            render_error(&error),
            "Error: invalid `luabox.toml`\n\nCaused by:\n    first\n    second\n"
        );
    }

    #[test]
    fn the_rendering_never_carries_a_backtrace() {
        // The whole point of rendering the chain by hand: `anyhow`'s `Debug`
        // appends a `Stack backtrace:` dump whenever `RUST_BACKTRACE` is set,
        // and a stripped release build renders those frames as `<unknown>`.
        let error = anyhow::anyhow!("boom").context("while doing the thing");
        let rendered = render_error(&error);
        assert!(!rendered.contains("Stack backtrace"), "{rendered}");
        assert!(!rendered.contains("<unknown>"), "{rendered}");
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

    #[test]
    fn the_subcommand_surface_is_exactly_these_twelve() {
        // The v1 scope cut (DIRECTION.md, 2026-07-26) made luabox a pure
        // static toolchain: no package manager, no registry client, no
        // interpreter. This is the whole surface — adding a command (even a
        // `hide`-ed one, which `get_subcommands` still reports) must be a
        // deliberate edit here, not a quiet regrowth of a deleted verb.
        let mut actual: Vec<String> = Cli::command()
            .get_subcommands()
            .map(|sub| sub.get_name().to_string())
            .collect();
        actual.sort();

        let mut expected = [
            "build", "check", "doc", "explain", "fmt", "init", "lint", "lsp", "new", "schema",
            "unmap", "upgrade",
        ];
        expected.sort_unstable();

        assert_eq!(actual, expected, "the CLI subcommand surface changed");
        assert_eq!(actual.len(), 12);

        // ...and every one of them is documented. This used to be a second
        // test with its own copy of the list above, which asserted that each
        // name was *present* — something the exact-set comparison already
        // covers — and then looped for `about`. Only the loop was load-bearing
        // (LG-N5), so it lives here, next to the set it is about.
        for sub in Cli::command().get_subcommands() {
            assert!(
                sub.get_about().is_some(),
                "`{}` has no help text",
                sub.get_name()
            );
        }
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
        assert_eq!(format, FormatArg::Human);
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
        assert_eq!(format, FormatArg::Json);
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
        assert_eq!(mode, Some(ModeArg::Love));
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
        let Command::Explain { code } = parse(&["explain", "LB0300"]) else {
            panic!("expected Explain");
        };
        assert_eq!(code, "LB0300");
    }

    // -- schema ------------------------------------------------------------

    #[test]
    fn schema_takes_no_arguments_at_all() {
        assert!(matches!(parse(&["schema"]), Command::Schema));
        assert_eq!(
            reject(&["schema", "--format", "json"]),
            clap::error::ErrorKind::UnknownArgument,
            "`schema` has no flags — it prints one document"
        );
    }

    #[test]
    fn schema_help_says_what_the_document_is_for() {
        // The help line is the discovery path: someone scanning `--help` for
        // a way to point their editor or an LLM at the manifest contract has
        // to recognise this command as it.
        let about = Cli::command()
            .find_subcommand("schema")
            .expect("subcommand exists")
            .get_about()
            .map(ToString::to_string)
            .unwrap_or_default();
        assert!(about.contains("JSON Schema"), "{about}");
        assert!(about.contains("luabox.toml"), "{about}");
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
    fn the_bin_flag_documents_itself_as_the_explicit_default() {
        // `--bin` and no flag at all scaffold the same project, so its help
        // has to say so — otherwise it reads as the opposite of `--lib`, i.e.
        // as something you have to pass (LG-N11). Both scaffolding commands
        // carry the same flag, so both are pinned.
        for name in ["init", "new"] {
            let sub = Cli::command()
                .find_subcommand(name)
                .expect("subcommand exists")
                .clone();
            let help = sub
                .get_arguments()
                .find(|a| a.get_id() == "bin")
                .and_then(clap::Arg::get_help)
                .map_or_else(
                    || panic!("`{name} --bin` has help text"),
                    ToString::to_string,
                );
            assert!(help.contains("the default"), "{name}: {help}");
        }
    }
}
