//! `luabox run <script|task> [args...]` — SPEC.md §4, §5.
//!
//! Resolution order (SPEC.md §4: "`luabox run` resolves package tasks then
//! `$PATH`"; the script-path step is the obvious middle ground and is
//! spelled out explicitly here):
//!
//! 1. **`[tasks]`** — if `luabox.toml` (nearest one walking up from `cwd`)
//!    has a `[tasks]` entry named `script`, run it. A [`TaskValue::Single`]
//!    is one command; a [`TaskValue::Multiple`] runs each command in order
//!    and **stops at the first failure**, propagating its exit code.
//! 2. **Script path** — else, if `script` names an existing `.lua` file
//!    (relative to `cwd`, or absolute), it is run as
//!    `<runtime> <script> <args...>`, where `<runtime>` is resolved from the
//!    manifest `edition` via [`crate::runtime::resolve_default`] (honoring the
//!    project pin, `LUABOX_LUA`, managed toolchains, then `PATH`).
//!    A bare script with no manifest in scope is fine — only tasks require
//!    one (an empty `[tasks]` table just means step 1 never matches).
//! 3. **`$PATH` fallback** — else `script` is probed as a bare executable
//!    name. This fallback is **kept with a purpose** (#3): it is not a worse
//!    `foo`, it is `npm run` resolving `node_modules/.bin` first. When the
//!    project pins a toolchain, its bin directories (the toolchain root and the
//!    provisioned `luarocks/`) are probed **before** the system `PATH`, so
//!    `luabox run luarocks -- install <rock>` hits the pinned, correctly-wired
//!    luarocks rather than whatever is on the system. The resolved executable
//!    is run directly with `args` as its argv (no shell). If none of the three
//!    resolve, the error lists the project's available task names.
//!
//! Extra `args` (everything after `script` on the command line) pass
//! through in all three cases. For a task, they're appended — shell-quoted
//! — to *every* command the task runs (documented behavior: a task is one
//! named unit of work, so args apply to the whole unit, not just its last
//! step).
//!
//! ## Pinned-toolchain environment (#3)
//!
//! nvm-style, **every** child `luabox run` spawns — task shells, scripts, and
//! `$PATH` executables — inherits the pinned toolchain's wiring:
//!
//! - **`PATH`** is prefixed with the toolchain's bin directories, so a
//!   `[tasks]` entry invoking bare `lua`/`luarocks` hits the pinned pair.
//! - **`LUAROCKS_CONFIG`** points at a generated config selecting the toolchain
//!   interpreter and a project-local `lua_modules` rock tree — environment
//!   injection is the portable mechanism luarocks honors everywhere (see
//!   [`write_luarocks_config`]). Generated only when the toolchain provisioned
//!   luarocks; regenerated per `run` invocation, never frozen into the install.
//!
//! With no pin in scope this wiring is empty and behavior is unchanged.
//!
//! ## Shell handling
//!
//! Task command strings are **not** split naively on whitespace — SPEC.md
//! doesn't mandate a shell grammar, and least-surprise (matching `npm run`)
//! is to hand the string to the platform shell verbatim: `cmd /C <command>`
//! on Windows, `sh -c <command>` elsewhere. That's the only place a shell is
//! involved; script and `$PATH`-executable invocations spawn the target
//! directly with `args` as literal argv entries, no shell parsing.
//!
//! ## Exit-code fidelity
//!
//! `anyhow::bail!` always maps to process exit code 1, which would silently
//! discard a child's real exit code (e.g. a task using specific codes to
//! signal different failures). So a *resolution* failure (bad manifest,
//! unknown name, runtime not found, failed to spawn) is a normal `Err` —
//! but once a child process actually runs, its exit code is propagated
//! faithfully via `std::process::exit`, not `anyhow::bail!`. See
//! [`RunOutcome`].
//!
//! ## Recursion guard
//!
//! A task can invoke `luabox run` on itself, directly or transitively
//! (`luabox.toml` is user-authored and mistakes happen). Every spawned
//! child has `LUABOX_RUN_DEPTH` set to one more than this process read on
//! entry; once that counter reaches [`MAX_RUN_DEPTH`], `luabox run` refuses
//! to spawn anything further and reports the loop instead of hanging.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use anyhow::{Context, bail};
use luabox_resolve::manifest::TaskValue;

use crate::runtime::{
    self, RuntimeSpec, find_on_path, luarocks_bin, resolve_default, toolchain_bin_dirs,
    toolchain_interpreter,
};

/// Nesting cap for `LUABOX_RUN_DEPTH` (see the module doc's "Recursion
/// guard" section). 32 is generous for any legitimate task graph while
/// still failing fast on a self-invoking task.
const MAX_RUN_DEPTH: u32 = 32;

/// Execute `luabox run <script> [args...]` from `cwd`. Returns `Err` only
/// for resolution failures (bad manifest, unknown name, recursion-guard
/// trip, failure to spawn at all); once a child process runs, its exit
/// status is propagated via `std::process::exit` from within this function
/// (see the module doc's "Exit-code fidelity" section), so a successful
/// `Ok(())` return means the child also exited 0.
pub fn run(cwd: &Path, script: &str, args: &[String]) -> anyhow::Result<()> {
    let child_depth = next_run_depth(std::env::var("LUABOX_RUN_DEPTH").ok().as_deref())?;

    let project = discover(cwd)?;
    let resolution = resolve(&project.tasks, cwd, script, Path::is_file);

    // The pinned toolchain (if any) drives both bare-executable resolution and
    // the environment every spawned child inherits — nvm-style (#3).
    let pinned = runtime::pinned_toolchain_dir(&project.root);
    let env = ToolchainEnv::for_toolchain(pinned.as_deref(), &project.root);

    let outcome = match resolution {
        Resolution::Task(task) => run_task(cwd, task, args, child_depth, &env)?,
        Resolution::Script(path) => {
            let runtime = resolve_default(&project.edition, &project.root)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            run_script(cwd, &runtime, &path, args, child_depth, &env)?
        }
        Resolution::PathExecutable => match resolve_executable(pinned.as_deref(), script) {
            Some(resolved) => run_path_executable(cwd, &resolved, args, child_depth, &env)?,
            None => return Err(unknown_name_error(script, &project.tasks)),
        },
    };

    match outcome {
        RunOutcome::Success => Ok(()),
        RunOutcome::Failed(code) => std::process::exit(code),
    }
}

/// How `name` resolves, in priority order. Borrows the matched
/// [`TaskValue`] from the caller's manifest rather than cloning it.
#[derive(Debug, PartialEq, Eq)]
enum Resolution<'a> {
    Task(&'a TaskValue),
    Script(PathBuf),
    PathExecutable,
}

/// The pure decision at the heart of `luabox run`: given the manifest's
/// `[tasks]` (empty when there is no manifest in scope), `cwd`, and the
/// argument `name`, decide which of the three resolution kinds applies.
///
/// `script_exists` is injected rather than calling `Path::is_file`
/// directly so this stays a pure function of its inputs — the acceptance
/// tests exercise real file existence end-to-end, while the unit tests
/// below assert the decision itself with no I/O.
fn resolve<'a>(
    tasks: &'a BTreeMap<String, TaskValue>,
    cwd: &Path,
    name: &str,
    script_exists: impl Fn(&Path) -> bool,
) -> Resolution<'a> {
    if let Some(task) = tasks.get(name) {
        return Resolution::Task(task);
    }
    let looks_like_a_lua_script = Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("lua"));
    if looks_like_a_lua_script {
        let candidate = resolve_script_path(cwd, name);
        if script_exists(&candidate) {
            return Resolution::Script(candidate);
        }
    }
    Resolution::PathExecutable
}

/// Resolve a bare executable `name` for the `$PATH` fallback, toolchain-first
/// (#3): a pinned toolchain's bin directories are probed before the system
/// `PATH` (the `node_modules/.bin`-first analog). `None` when nothing matches.
fn resolve_executable(pinned: Option<&Path>, name: &str) -> Option<PathBuf> {
    pinned
        .and_then(|dir| runtime::find_in_toolchain(dir, name))
        .or_else(|| find_on_path(name))
}

/// `name` resolved against `cwd` (absolute paths pass through untouched).
fn resolve_script_path(cwd: &Path, name: &str) -> PathBuf {
    let as_path = Path::new(name);
    if as_path.is_absolute() {
        as_path.to_path_buf()
    } else {
        cwd.join(as_path)
    }
}

/// The result of actually running a resolved child: either it exited 0, or
/// it didn't and here's the code to propagate (signals and other
/// code-less terminations on Unix are folded to `1`, matching the shell
/// convention).
enum RunOutcome {
    Success,
    Failed(i32),
}

impl From<ExitStatus> for RunOutcome {
    fn from(status: ExitStatus) -> Self {
        if status.success() {
            Self::Success
        } else {
            Self::Failed(status.code().unwrap_or(1))
        }
    }
}

/// The pinned toolchain's contribution to a spawned child's environment (#3):
/// a `PATH` prefix of the toolchain's bin directories, and a generated
/// `LUAROCKS_CONFIG` wiring luarocks to the toolchain interpreter and a
/// project-local tree. Empty (a no-op) when no toolchain is pinned.
#[derive(Default)]
struct ToolchainEnv {
    /// Directories prepended to the child's `PATH`, highest priority first.
    path_prefix: Vec<PathBuf>,
    /// Path to the generated `LUAROCKS_CONFIG`, if luarocks was provisioned.
    luarocks_config: Option<PathBuf>,
    /// Keeps the generated config file alive for the child's lifetime; dropped
    /// (and cleaned up) when this `ToolchainEnv` goes out of scope.
    _config_dir: Option<tempfile::TempDir>,
}

impl ToolchainEnv {
    /// Build the environment for `toolchain_dir` (the pinned, installed
    /// toolchain), rooted at the project `root` for the luarocks tree. `None`
    /// yields an empty, do-nothing environment.
    fn for_toolchain(toolchain_dir: Option<&Path>, root: &Path) -> Self {
        let Some(dir) = toolchain_dir else {
            return Self::default();
        };
        let path_prefix = toolchain_bin_dirs(dir)
            .into_iter()
            .filter(|d| d.is_dir())
            .collect();
        let (luarocks_config, config_dir) = match write_luarocks_config(dir, root) {
            Some((path, guard)) => (Some(path), Some(guard)),
            None => (None, None),
        };
        Self {
            path_prefix,
            luarocks_config,
            _config_dir: config_dir,
        }
    }

    /// Apply this environment to a command about to be spawned: prepend the
    /// toolchain bin directories to `PATH` and set `LUAROCKS_CONFIG`.
    fn apply(&self, cmd: &mut Command) {
        if !self.path_prefix.is_empty() {
            let existing = std::env::var_os("PATH").unwrap_or_default();
            let mut dirs = self.path_prefix.clone();
            dirs.extend(std::env::split_paths(&existing));
            if let Ok(joined) = std::env::join_paths(dirs) {
                cmd.env("PATH", joined);
            }
        }
        if let Some(config) = &self.luarocks_config {
            cmd.env("LUAROCKS_CONFIG", config);
        }
    }
}

/// Generate a `LUAROCKS_CONFIG` file wiring the toolchain's luarocks to its own
/// interpreter and a project-local `lua_modules` tree, returning the file path
/// and the temp dir that owns it. Returns `None` when the toolchain provisioned
/// no luarocks (nothing to configure) or the file can't be written.
///
/// luarocks reads the Lua file named by `LUAROCKS_CONFIG` and honors
/// `lua_interpreter` / `variables.LUA_*` (which interpreter to build rocks for)
/// and `rocks_trees` (where rocks land — here a single project-local tree, the
/// `--tree lua_modules` semantics). Paths are emitted with forward slashes so
/// the Lua string literals need no escaping on Windows.
fn write_luarocks_config(
    toolchain_dir: &Path,
    root: &Path,
) -> Option<(PathBuf, tempfile::TempDir)> {
    let interp = toolchain_interpreter(toolchain_dir)?;
    luarocks_bin(toolchain_dir)?; // only wire a config when luarocks exists
    let bindir = interp.parent().unwrap_or(toolchain_dir);
    let slashed = |p: &Path| p.to_string_lossy().replace('\\', "/");
    let tree = slashed(&root.join("lua_modules"));
    let contents = format!(
        "-- Generated by `luabox run` for the pinned toolchain (#3). Do not edit.\n\
         lua_interpreter = \"{interp_name}\"\n\
         variables = {{\n\
         \x20   LUA_DIR = \"{bindir}\",\n\
         \x20   LUA_BINDIR = \"{bindir}\",\n\
         }}\n\
         rocks_trees = {{\n\
         \x20   {{ name = \"project\", root = \"{tree}\" }},\n\
         }}\n",
        interp_name = interp
            .file_name()
            .map_or_else(|| "lua".into(), |n| n.to_string_lossy()),
        bindir = slashed(bindir),
    );
    let dir = tempfile::tempdir().ok()?;
    let path = dir.path().join("luarocks-config.lua");
    std::fs::write(&path, contents).ok()?;
    Some((path, dir))
}

/// Run a resolved `[tasks]` entry: one command, or a sequence that stops at
/// the first non-zero exit.
fn run_task(
    cwd: &Path,
    task: &TaskValue,
    args: &[String],
    run_depth: u32,
    env: &ToolchainEnv,
) -> anyhow::Result<RunOutcome> {
    match task {
        TaskValue::Single(command) => run_shell_command(cwd, command, args, run_depth, env),
        TaskValue::Multiple(commands) => {
            for command in commands {
                let outcome = run_shell_command(cwd, command, args, run_depth, env)?;
                if matches!(outcome, RunOutcome::Failed(_)) {
                    return Ok(outcome);
                }
            }
            Ok(RunOutcome::Success)
        }
    }
}

/// Run one task command string through the platform shell, with `args`
/// shell-quoted and appended. See the module doc's "Shell handling"
/// section for why this isn't a naive whitespace split.
fn run_shell_command(
    cwd: &Path,
    command: &str,
    args: &[String],
    run_depth: u32,
    env: &ToolchainEnv,
) -> anyhow::Result<RunOutcome> {
    let full_command = if args.is_empty() {
        command.to_string()
    } else {
        let quoted = args.iter().map(|a| quote_for_shell(a)).collect::<Vec<_>>();
        format!("{command} {}", quoted.join(" "))
    };

    let mut cmd = if cfg!(windows) {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(&full_command);
        cmd
    } else {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(&full_command);
        cmd
    };
    cmd.current_dir(cwd)
        .env("LUABOX_RUN_DEPTH", run_depth.to_string());
    env.apply(&mut cmd);
    let status = cmd
        .status()
        .with_context(|| format!("failed to spawn a shell for task command `{command}`"))?;
    Ok(status.into())
}

/// Quote `arg` for inclusion in a shell command line only if it needs it
/// (contains whitespace or a quote character); passed through verbatim
/// otherwise so the common case stays readable in error messages.
fn quote_for_shell(arg: &str) -> String {
    if arg.chars().all(|c| !c.is_whitespace() && c != '"') {
        return arg.to_string();
    }
    if cfg!(windows) {
        format!("\"{}\"", arg.replace('"', "\"\""))
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

/// Run a resolved `.lua` script as `<runtime> <script> <args...>` — no
/// shell, `args` passed through as literal argv entries.
fn run_script(
    cwd: &Path,
    runtime: &RuntimeSpec,
    script: &Path,
    args: &[String],
    run_depth: u32,
    env: &ToolchainEnv,
) -> anyhow::Result<RunOutcome> {
    let mut cmd = Command::new(&runtime.program);
    cmd.args(&runtime.args)
        .arg(script)
        .args(args)
        .current_dir(cwd)
        .env("LUABOX_RUN_DEPTH", run_depth.to_string());
    env.apply(&mut cmd);
    let status = cmd.status().with_context(|| {
        format!(
            "failed to spawn `{}` to run `{}`",
            runtime.program,
            script.display()
        )
    })?;
    Ok(status.into())
}

/// Run a resolved `$PATH` executable directly — no shell, `args` passed
/// through as literal argv entries.
fn run_path_executable(
    cwd: &Path,
    resolved: &Path,
    args: &[String],
    run_depth: u32,
    env: &ToolchainEnv,
) -> anyhow::Result<RunOutcome> {
    let mut cmd = Command::new(resolved);
    cmd.args(args)
        .current_dir(cwd)
        .env("LUABOX_RUN_DEPTH", run_depth.to_string());
    env.apply(&mut cmd);
    let status = cmd
        .status()
        .with_context(|| format!("failed to spawn `{}`", resolved.display()))?;
    Ok(status.into())
}

/// Parses the current `LUABOX_RUN_DEPTH` (as read from the process
/// environment by the caller) and returns the depth to hand to a spawned
/// child, or an error once nesting has reached [`MAX_RUN_DEPTH`]. Pure over
/// its input — read from the environment at the one call site — so the
/// boundary itself is unit-testable without mutating process state.
fn next_run_depth(current: Option<&str>) -> anyhow::Result<u32> {
    let depth: u32 = current.and_then(|v| v.parse().ok()).unwrap_or(0);
    if depth >= MAX_RUN_DEPTH {
        bail!(
            "`luabox run` nesting exceeded {MAX_RUN_DEPTH} levels (LUABOX_RUN_DEPTH); this \
             usually means a task invokes `luabox run` on itself, directly or transitively"
        );
    }
    Ok(depth + 1)
}

/// A clear error for a name that matched none of the three resolution
/// kinds, listing the project's available tasks (if any) so a typo is
/// obvious.
fn unknown_name_error(name: &str, tasks: &BTreeMap<String, TaskValue>) -> anyhow::Error {
    if tasks.is_empty() {
        anyhow::anyhow!(
            "no task, script, or PATH executable named `{name}` found (no [tasks] are defined, \
             and `{name}` is not an existing `.lua` file or an executable on PATH)"
        )
    } else {
        let names = tasks.keys().cloned().collect::<Vec<_>>().join(", ");
        anyhow::anyhow!(
            "no task, script, or PATH executable named `{name}` found; available tasks: {names}"
        )
    }
}

/// The bit of project state `luabox run` needs: the root (for the runtime
/// pin lookup — `resolve_default`'s `root` argument), the edition (for
/// script runtime resolution), and the `[tasks]` table (empty when there's
/// no manifest in scope — bare scripts and `$PATH` executables don't need
/// one).
struct Project {
    root: PathBuf,
    edition: String,
    tasks: BTreeMap<String, TaskValue>,
}

/// Find the project: nearest `luabox.toml` walking up from `cwd`, or a
/// manifest-less default rooted at `cwd` (edition 5.4, no tasks).
fn discover(cwd: &Path) -> anyhow::Result<Project> {
    match crate::project::discover_manifest(cwd)? {
        Some((root, manifest)) => Ok(Project {
            root,
            edition: manifest.package.edition.clone(),
            tasks: manifest.tasks.clone(),
        }),
        None => Ok(Project {
            root: cwd.to_path_buf(),
            edition: "5.4".to_string(),
            tasks: BTreeMap::new(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Resolution, TaskValue, next_run_depth, quote_for_shell, resolve, resolve_executable,
        resolve_script_path,
    };
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    fn tasks_with(name: &str, value: TaskValue) -> BTreeMap<String, TaskValue> {
        let mut tasks = BTreeMap::new();
        tasks.insert(name.to_string(), value);
        tasks
    }

    #[test]
    fn a_matching_task_wins_even_if_a_same_named_lua_file_exists() {
        let tasks = tasks_with("build.lua", TaskValue::Single("echo hi".to_string()));
        let cwd = Path::new("/proj");
        let resolution = resolve(&tasks, cwd, "build.lua", |_| true);
        assert_eq!(
            resolution,
            Resolution::Task(&TaskValue::Single("echo hi".to_string()))
        );
    }

    #[test]
    fn a_non_task_name_falls_through_to_script_when_the_file_exists() {
        let tasks = BTreeMap::new();
        let cwd = Path::new("/proj");
        let resolution = resolve(&tasks, cwd, "src/main.lua", |_| true);
        assert_eq!(
            resolution,
            Resolution::Script(PathBuf::from("/proj/src/main.lua"))
        );
    }

    #[test]
    fn a_non_lua_name_never_resolves_as_a_script() {
        let tasks = BTreeMap::new();
        let cwd = Path::new("/proj");
        // Even if a same-named file "exists" per the injected predicate, a
        // name without a `.lua` extension is never treated as a script.
        let resolution = resolve(&tasks, cwd, "make", |_| true);
        assert_eq!(resolution, Resolution::PathExecutable);
    }

    #[test]
    fn a_lua_name_that_does_not_exist_falls_through_to_path() {
        let tasks = BTreeMap::new();
        let cwd = Path::new("/proj");
        let resolution = resolve(&tasks, cwd, "missing.lua", |_| false);
        assert_eq!(resolution, Resolution::PathExecutable);
    }

    #[test]
    fn an_absolute_script_path_is_not_rejoined_to_cwd() {
        let abs = if cfg!(windows) {
            "C:/elsewhere/main.lua"
        } else {
            "/elsewhere/main.lua"
        };
        let resolved = resolve_script_path(Path::new("/proj"), abs);
        assert_eq!(resolved, PathBuf::from(abs));
    }

    #[test]
    fn no_env_var_starts_depth_at_one_for_the_child() {
        assert_eq!(next_run_depth(None).unwrap(), 1);
    }

    #[test]
    fn depth_increments_for_each_nesting_level() {
        assert_eq!(next_run_depth(Some("5")).unwrap(), 6);
    }

    #[test]
    fn depth_at_the_cap_is_rejected() {
        assert!(next_run_depth(Some("32")).is_err());
        assert!(next_run_depth(Some("100")).is_err());
    }

    #[test]
    fn a_garbage_depth_value_is_treated_as_the_start() {
        assert_eq!(next_run_depth(Some("not-a-number")).unwrap(), 1);
    }

    #[test]
    fn resolve_executable_prefers_the_pinned_toolchain() {
        // A pinned toolchain that carries `luarocks` in its `luarocks/` subdir
        // resolves it there — never consulting the system PATH (#3).
        let dir = tempfile::tempdir().unwrap();
        let lr = dir.path().join("luarocks");
        std::fs::create_dir_all(&lr).unwrap();
        let name = if cfg!(windows) {
            "luarocks.cmd"
        } else {
            "luarocks"
        };
        let shim = lr.join(name);
        std::fs::write(&shim, "").unwrap();

        let hit = resolve_executable(Some(dir.path()), "luarocks");
        assert_eq!(hit.as_deref(), Some(shim.as_path()));

        // A name the toolchain doesn't carry falls through to the PATH probe;
        // a name that cannot exist anywhere yields None.
        assert!(resolve_executable(Some(dir.path()), "luabox-zzz-no-such-exe").is_none());
    }

    #[test]
    fn plain_args_are_not_quoted() {
        assert_eq!(quote_for_shell("hello"), "hello");
        assert_eq!(quote_for_shell("--flag=value"), "--flag=value");
    }

    #[test]
    fn args_with_whitespace_are_quoted() {
        let quoted = quote_for_shell("has space");
        assert!(quoted.starts_with(if cfg!(windows) { '"' } else { '\'' }));
        assert!(quoted.contains("has space"));
    }
}
