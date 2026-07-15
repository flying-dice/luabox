//! `differ` — the SPEC.md §16.1/§16.2 differential-execution harness.
//!
//! For every corpus program (a `.lua` file with a `-- DIFFER:` header) and
//! every target dialect it names, the harness:
//!
//! 1. runs the **original** program on its `from`-dialect runtime,
//! 2. lowers it `from → to` with [`luabox_lower`] (linked by path — a lowering
//!    change is exercised the moment this tool is rebuilt),
//! 3. runs the **lowered** output on the `to`-dialect runtime,
//! 4. compares stdout (exact), exit code, and error class (see [`compare`]).
//!
//! Runtimes are resolved by candidate binary names on `PATH` (a small
//! self-contained probe, [`candidate_names`] + [`find_on_path`]). A pair whose
//! source **or** target runtime is
//! absent is **skipped with a note** — never a failure — so the harness runs
//! partially on a machine with only some Lua versions installed (the full
//! matrix runs in CI). The process exits non-zero if any pair **mismatched**
//! or failed to lower.
//!
//! Not a workspace member (root `Cargo.toml` excludes `tools/*`): a build-time
//! test tool, not something the shipped `luabox` binary depends on.
//!
//! Usage:
//!   differ --corpus <dir> [--filter <substr>] [--timeout <secs>] [--verbose]

mod compare;
mod corpus;
mod exec;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use luabox_syntax::Dialect;

use compare::{Axis, Verdict};

/// Candidate interpreter binary names for a dialect's manifest id, in
/// priority order (self-contained copy of the CLI's PATH-probe rules — this
/// tool is not a workspace member and cannot depend on the `luabox` binary).
fn candidate_names(edition: &str) -> Vec<String> {
    let names: &[&str] = match edition {
        "5.1" => &["lua5.1", "lua51", "lua"],
        "5.2" => &["lua5.2", "lua52", "lua"],
        "5.3" => &["lua5.3", "lua53", "lua"],
        "5.4" => &["lua5.4", "lua54", "lua"],
        "luajit" => &["luajit"],
        _ => &["lua"],
    };
    names.iter().map(|s| (*s).to_string()).collect()
}

/// Resolve a bare interpreter `name` on `PATH`, honoring `PATHEXT` on Windows.
/// Returns the resolved path if a matching executable exists.
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    let exts = path_exts();
    for dir in std::env::split_paths(&path_var) {
        for ext in &exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Executable extensions to try for a bare name: `[""]` on Unix; the
/// `PATHEXT` list (lower-cased, plus the empty extension) on Windows.
fn path_exts() -> Vec<String> {
    if cfg!(windows) {
        let raw = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.BAT;.CMD;.COM".to_string());
        let mut exts = vec![String::new()];
        for ext in raw.split(';') {
            let ext = ext.trim();
            if !ext.is_empty() {
                exts.push(ext.to_ascii_lowercase());
            }
        }
        exts
    } else {
        vec![String::new()]
    }
}

/// One resolved run configuration.
struct Args {
    corpus: PathBuf,
    filter: Option<String>,
    timeout: Duration,
    verbose: bool,
    /// Also write every lowered output to this directory (inspection /
    /// debugging aid; happens even for pairs later skipped for a missing
    /// runtime, so lowered text can be examined and run by hand).
    emit: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut corpus = PathBuf::from("corpus/differ");
    let mut filter = None;
    let mut timeout = Duration::from_secs(10);
    let mut verbose = false;
    let mut emit = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--corpus" => {
                corpus = PathBuf::from(args.next().ok_or("--corpus requires a value")?);
            }
            "--filter" => {
                filter = Some(args.next().ok_or("--filter requires a value")?);
            }
            "--emit" => {
                emit = Some(PathBuf::from(args.next().ok_or("--emit requires a value")?));
            }
            "--timeout" => {
                let secs: u64 = args
                    .next()
                    .ok_or("--timeout requires a value")?
                    .parse()
                    .map_err(|_| "--timeout must be whole seconds".to_string())?;
                timeout = Duration::from_secs(secs);
            }
            "--verbose" | "-v" => verbose = true,
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument `{other}` (see --help)")),
        }
    }

    Ok(Args {
        corpus,
        filter,
        timeout,
        verbose,
        emit,
    })
}

fn print_help() {
    println!(
        "differ — differential execution of lowered output vs source (SPEC.md §16.1/§16.2)\n\n\
         USAGE:\n    differ --corpus <dir> [--filter <substr>] [--timeout <secs>] [--verbose]\n\n\
         OPTIONS:\n    \
         --corpus <dir>     directory of annotated .lua corpus files (default: corpus/differ)\n    \
         --filter <substr>  only run corpus files whose name contains <substr>\n    \
         --timeout <secs>   per-run wall-clock timeout, catches infinite loops (default: 10)\n    \
         --emit <dir>       also write every lowered output there (inspection aid)\n    \
         --verbose, -v      print per-pair detail and lowering polyfill/warning notes\n\n\
         EXIT: non-zero if any pair MISMATCHed or failed to lower; SKIPs never fail."
    );
}

/// The outcome of one `(file, target)` pair.
enum Outcome {
    Match,
    Mismatch(Vec<Axis>),
    LowerError(String),
    Skipped(String),
    ExecError(String),
}

impl Outcome {
    fn tag(&self) -> &'static str {
        match self {
            Outcome::Match => "PASS",
            Outcome::Mismatch(_) => "MISMATCH",
            Outcome::LowerError(_) => "LOWER-ERR",
            Outcome::Skipped(_) => "SKIP",
            Outcome::ExecError(_) => "EXEC-ERR",
        }
    }

    fn detail(&self) -> String {
        match self {
            Outcome::Match => String::new(),
            Outcome::Mismatch(axes) => axes
                .iter()
                .map(|a| a.label())
                .collect::<Vec<_>>()
                .join(", "),
            Outcome::LowerError(m) | Outcome::Skipped(m) | Outcome::ExecError(m) => m.clone(),
        }
    }

    /// Only mismatches, lowering failures, and exec errors fail the run.
    /// A skip is a documented gap, never a failure.
    fn is_failure(&self) -> bool {
        matches!(
            self,
            Outcome::Mismatch(_) | Outcome::LowerError(_) | Outcome::ExecError(_)
        )
    }
}

/// A row of the summary table.
struct Row {
    file: String,
    pair: String,
    outcome: Outcome,
}

/// Resolves and memoizes an interpreter path per dialect from `PATH`.
///
/// Every candidate is **version-verified** before being accepted: the
/// candidate-name lists include a generic `lua` fallback, and a machine whose
/// bare `lua` is 5.1 must not have it masquerade as the 5.3 runtime — running
/// source on the wrong interpreter would produce false mismatches (or, worse,
/// false matches). LuaJIT reports `_VERSION == "Lua 5.1"`, so it is told apart
/// from real 5.1 by its `jit` global.
struct Runtimes {
    cache: BTreeMap<Dialect, Option<String>>,
}

impl Runtimes {
    fn new() -> Self {
        Self {
            cache: BTreeMap::new(),
        }
    }

    /// The resolved-and-verified interpreter path for `dialect`, or `None` if
    /// no candidate binary on `PATH` reports the right version.
    fn resolve(&mut self, dialect: Dialect) -> Option<String> {
        if let Some(hit) = self.cache.get(&dialect) {
            return hit.clone();
        }
        let label = dialect.manifest_id();
        let resolved = candidate_names(label)
            .into_iter()
            .filter_map(|name| find_on_path(&name))
            .map(|p| p.to_string_lossy().into_owned())
            .find(|p| verify_version(p, dialect));
        self.cache.insert(dialect, resolved.clone());
        resolved
    }
}

/// True when `program` really is an interpreter for `dialect`: it must print
/// the matching `_VERSION` (suffixed `+jit` when the LuaJIT-only `jit` global
/// is present) for a tiny `-e` probe.
fn verify_version(program: &str, dialect: Dialect) -> bool {
    let probe = r#"io.write(_VERSION .. (jit and "+jit" or ""))"#;
    let Ok(out) = std::process::Command::new(program)
        .args(["-e", probe])
        .stdin(std::process::Stdio::null())
        .output()
    else {
        return false;
    };
    let banner = String::from_utf8_lossy(&out.stdout);
    let expected = match dialect {
        Dialect::Lua51 => "Lua 5.1",
        Dialect::Lua52 => "Lua 5.2",
        Dialect::Lua53 => "Lua 5.3",
        Dialect::Lua54 => "Lua 5.4",
        Dialect::LuaJit => "Lua 5.1+jit",
    };
    banner.trim() == expected
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(err) => {
            eprintln!("differ: error: {err}");
            return ExitCode::FAILURE;
        }
    };

    match run(&args) {
        Ok(false) => ExitCode::SUCCESS,
        Ok(true) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("differ: error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Returns `Ok(true)` when at least one pair failed (mismatch / lower-error).
fn run(args: &Args) -> Result<bool, String> {
    let files = corpus::discover(&args.corpus)
        .map_err(|e| format!("reading corpus dir {}: {e}", args.corpus.display()))?;
    if files.is_empty() {
        return Err(format!("no .lua files under {}", args.corpus.display()));
    }

    let tmp = TempDir::new().map_err(|e| format!("creating temp dir: {e}"))?;
    let mut runtimes = Runtimes::new();

    // Startup banner: which runtimes resolved (helps read the SKIP notes).
    print_runtime_banner(&mut runtimes);

    let mut rows: Vec<Row> = Vec::new();

    for file in &files {
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<file>")
            .to_string();
        if let Some(f) = &args.filter
            && !name.contains(f)
        {
            continue;
        }

        let source = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                rows.push(Row {
                    file: name.clone(),
                    pair: "-".into(),
                    outcome: Outcome::ExecError(format!("read failed: {e}")),
                });
                continue;
            }
        };

        let header = match corpus::parse_header(&source) {
            Ok(h) => h,
            Err(e) => {
                rows.push(Row {
                    file: name.clone(),
                    pair: "-".into(),
                    outcome: Outcome::ExecError(format!("bad header: {e}")),
                });
                continue;
            }
        };

        for &target in &header.targets {
            let pair = format!("{}->{}", header.from.manifest_id(), target.manifest_id());
            let outcome = run_pair(
                &source,
                file,
                header.from,
                target,
                &mut runtimes,
                tmp.path(),
                &name,
                args,
            );
            rows.push(Row {
                file: name.clone(),
                pair,
                outcome,
            });
        }
    }

    print_summary(&rows, args.verbose);
    Ok(rows.iter().any(|r| r.outcome.is_failure()))
}

/// Execute one `(file, target)` differential pair.
#[allow(clippy::too_many_arguments)]
fn run_pair(
    source: &str,
    file: &Path,
    from: Dialect,
    to: Dialect,
    runtimes: &mut Runtimes,
    tmp: &Path,
    name: &str,
    args: &Args,
) -> Outcome {
    // Lower first, before resolving runtimes: a corpus file that cannot be
    // lowered cleanly is a failure *regardless* of which interpreters happen to
    // be installed, so this axis is validated even on a machine that must skip
    // the actual execution (e.g. only Lua 5.1 present locally).
    let lowered = match luabox_lower::lower(source, from, to) {
        Ok(l) => l,
        Err(diags) => {
            let codes: Vec<&str> = diags.iter().map(|d| d.code).collect();
            return Outcome::LowerError(format!("lowering failed: {}", codes.join(", ")));
        }
    };

    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("chunk");
    let emitted_name = format!("{stem}.to-{}.lua", to.manifest_id());

    // --emit: persist the lowered text even when the pair is about to be
    // skipped, so it can be inspected and run by hand.
    if let Some(dir) = &args.emit {
        if let Err(e) = std::fs::create_dir_all(dir) {
            return Outcome::ExecError(format!("creating --emit dir: {e}"));
        }
        if let Err(e) = std::fs::write(dir.join(&emitted_name), &lowered.text) {
            return Outcome::ExecError(format!("writing --emit file: {e}"));
        }
    }

    let Some(from_rt) = runtimes.resolve(from) else {
        return Outcome::Skipped(format!(
            "no verified {} runtime on PATH",
            from.manifest_id()
        ));
    };
    let Some(to_rt) = runtimes.resolve(to) else {
        return Outcome::Skipped(format!("no verified {} runtime on PATH", to.manifest_id()));
    };
    if args.verbose && (!lowered.polyfills.is_empty() || !lowered.warnings.is_empty()) {
        let warns: Vec<&str> = lowered.warnings.iter().map(|w| w.code).collect();
        eprintln!(
            "  {name} {}->{}: polyfills=[{}] warnings=[{}]",
            from.manifest_id(),
            to.manifest_id(),
            lowered.polyfills.join(", "),
            warns.join(", ")
        );
    }

    // Write the lowered output to a uniquely-named temp file, keeping the
    // stem so its chunk name reads naturally in any error output.
    let lowered_path = tmp.join(&emitted_name);
    if let Err(e) = std::fs::write(&lowered_path, &lowered.text) {
        return Outcome::ExecError(format!("writing lowered temp: {e}"));
    }

    let orig = match exec::run(&from_rt, file, args.timeout) {
        Ok(r) => r,
        Err(e) => return Outcome::ExecError(format!("running source on {from_rt}: {e}")),
    };
    let low = match exec::run(&to_rt, &lowered_path, args.timeout) {
        Ok(r) => r,
        Err(e) => return Outcome::ExecError(format!("running lowered on {to_rt}: {e}")),
    };

    match compare::compare(&orig, &low, &from_rt, &to_rt) {
        Verdict::Match => Outcome::Match,
        Verdict::Mismatch(axes) => {
            if args.verbose {
                report_mismatch(name, from, to, &orig, &low);
            }
            Outcome::Mismatch(axes)
        }
    }
}

/// Dump the diverging streams for a mismatch (verbose mode) — enough to see
/// *why* without re-running by hand.
fn report_mismatch(
    name: &str,
    from: Dialect,
    to: Dialect,
    orig: &exec::ExecResult,
    low: &exec::ExecResult,
) {
    eprintln!(
        "--- MISMATCH {name} {}->{} ---",
        from.manifest_id(),
        to.manifest_id()
    );
    eprintln!(
        "  source  exit={:?} timeout={} stdout={:?}",
        orig.code, orig.timed_out, orig.stdout
    );
    eprintln!(
        "  lowered exit={:?} timeout={} stdout={:?}",
        low.code, low.timed_out, low.stdout
    );
    if orig.failed() || low.failed() {
        eprintln!("  source  stderr={:?}", orig.stderr);
        eprintln!("  lowered stderr={:?}", low.stderr);
    }
}

fn print_runtime_banner(runtimes: &mut Runtimes) {
    let mut found = Vec::new();
    let mut missing = Vec::new();
    for d in Dialect::ALL {
        if runtimes.resolve(d).is_some() {
            found.push(d.manifest_id());
        } else {
            missing.push(d.manifest_id());
        }
    }
    println!(
        "differ: runtimes found (version-verified): [{}]",
        found.join(", ")
    );
    if !missing.is_empty() {
        println!(
            "differ: runtimes missing (pairs needing these are SKIPPED): [{}]",
            missing.join(", ")
        );
    }
    println!();
}

fn print_summary(rows: &[Row], verbose: bool) {
    let file_w = rows.iter().map(|r| r.file.len()).max().unwrap_or(4).max(4);
    let pair_w = rows.iter().map(|r| r.pair.len()).max().unwrap_or(4).max(4);

    println!(
        "{:<file_w$}  {:<pair_w$}  {:<9}  DETAIL",
        "FILE",
        "PAIR",
        "RESULT",
        file_w = file_w,
        pair_w = pair_w
    );
    for r in rows {
        let detail = r.outcome.detail();
        // Keep passes quiet unless verbose; always show non-passes.
        if verbose || !matches!(r.outcome, Outcome::Match) {
            println!(
                "{:<file_w$}  {:<pair_w$}  {:<9}  {}",
                r.file,
                r.pair,
                r.outcome.tag(),
                detail,
                file_w = file_w,
                pair_w = pair_w
            );
        }
    }

    let mut pass = 0;
    let mut mismatch = 0;
    let mut lower_err = 0;
    let mut skip = 0;
    let mut exec_err = 0;
    for r in rows {
        match r.outcome {
            Outcome::Match => pass += 1,
            Outcome::Mismatch(_) => mismatch += 1,
            Outcome::LowerError(_) => lower_err += 1,
            Outcome::Skipped(_) => skip += 1,
            Outcome::ExecError(_) => exec_err += 1,
        }
    }

    println!();
    println!(
        "differ: {} pairs — {pass} passed, {mismatch} mismatched, {lower_err} lower-errors, \
         {exec_err} exec-errors, {skip} skipped",
        rows.len()
    );
    if mismatch + lower_err + exec_err == 0 {
        println!("differ: OK (no mismatches; {skip} pairs skipped for missing runtimes)");
    } else {
        println!("differ: FAILED");
    }
}

/// A self-cleaning temp directory (no external `tempfile` dep for the shipped
/// tool — one unique dir under the system temp root, removed on drop).
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> std::io::Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("differ-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
