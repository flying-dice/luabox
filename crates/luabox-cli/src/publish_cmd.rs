//! `luabox publish [--yank <version>]` — publish to the first-party
//! registry (SPEC.md §6, ticket #20).
//!
//! # Publish pipeline
//!
//! 1. **Gates.** `luabox check` must be green (errors block the publish);
//!    the test suite must pass — but when no Lua runtime can be resolved
//!    the tests are *skipped with a warning*, because publishing from
//!    machines without Lua must work (luabox is not a runtime). Annotation
//!    coverage of the public API is *advisory*: public functions in `src/`
//!    without `---@param`/`---@return` annotations are listed as a warning,
//!    never a failure (MVP; SPEC.md §6 wants this fatal eventually).
//! 2. **Pack.** The package tree — `luabox.toml`, `README*`, everything
//!    under `src/`, and any `.lb` shape modules — is staged, excluding
//!    build output (`[build] out`, default `dist/`), `lua_modules/`,
//!    `vendor/`, and dot-files/dirs.
//! 3. **Hash.** The staged tree is interned into the content-addressed
//!    store (`Store::put_tree`); its tree hash becomes the index line's
//!    `checksum` (`sha256:…`) — the exact value `luabox install` recomputes
//!    after extracting the artifact.
//! 4. **Push.** The tree is packed into `<version>.tar` with the `tar` CLI
//!    (the toolchain installer's approach — no archive crate) and handed to
//!    [`Registry::publish`], which stores the artifact and appends the
//!    index line, refusing a duplicate `name@version`.
//!
//! Registry dependencies only: a package whose `[dependencies]` contain
//! path/git/workspace entries is refused — consumers could never resolve
//! them from a registry. Version-req dev-dependencies are recorded on the
//! index line with `dev: true` (they never affect consumers); non-registry
//! dev-dependencies are skipped with a note.
//!
//! `--yank <version>` flips the version's `yanked` flag in the index —
//! crates.io semantics: hidden from new resolutions, restorable from
//! existing lockfiles, never deleted.
//!
//! The registry must be *writable*, i.e. a directory or `file://` root;
//! `https://` registries are read-only in this MVP. Artifact signing
//! (sigstore) is out of scope for the MVP — integrity rests on the tree
//! hash recorded in the index.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow, bail};
use luabox_resolve::manifest::Dependency;
use luabox_resolve::{IndexDep, IndexEntry, REGISTRY_ENV, Registry};
use luabox_store::Store;
use luabox_test::run_suite;
use luabox_test::runner::SuiteOptions;
use luabox_test::runtime::resolve_default;

use crate::deps_cmd::{self, Project};

/// Execute `luabox publish` (or, with `yank: Some(version)`, flip that
/// version's yanked flag instead of publishing).
pub fn run(cwd: &Path, yank: Option<&str>) -> anyhow::Result<()> {
    let project = deps_cmd::discover(cwd)?;
    let registry = deps_cmd::registry_from_env()?.ok_or_else(|| {
        anyhow!(
            "`luabox publish` needs a registry: set {REGISTRY_ENV} to a writable \
             registry root (a directory or file:// URL). There is no hosted \
             default registry yet (SPEC.md §6)"
        )
    })?;
    if !registry.is_writable() {
        bail!(
            "cannot publish to `{}`: https registries are read-only in this MVP — \
             point {REGISTRY_ENV} at the registry's filesystem root (or file:// \
             URL) to publish",
            registry.location()
        );
    }

    let name = project.manifest.package.name.clone();
    if let Some(version) = yank {
        return run_yank(&registry, &name, version);
    }
    publish(&project, &registry, &name)
}

/// `luabox publish --yank <version>`: crates.io rule — hide, never delete.
fn run_yank(registry: &Registry, name: &str, version: &str) -> anyhow::Result<()> {
    let changed = registry
        .set_yanked(name, version, true)
        .map_err(|e| anyhow!("{e}"))?;
    if changed {
        println!(
            "yanked `{name}@{version}`: new resolutions will skip it; projects \
             whose luabox.lock already pins it can still restore it"
        );
    } else {
        println!("`{name}@{version}` is already yanked");
    }
    Ok(())
}

/// The full gate + pack + push pipeline.
fn publish(project: &Project, registry: &Registry, name: &str) -> anyhow::Result<()> {
    let version = &project.manifest.package.version;

    // Refuse early (before the slow gates) if this version already exists —
    // and refuse dependencies a registry consumer could never resolve.
    if let Some(entries) = registry.load_entries(name).map_err(|e| anyhow!("{e}"))?
        && entries.iter().any(|e| &e.version == version)
    {
        bail!(
            "`{name}@{version}` is already published; registry versions are \
             immutable — bump the version, or yank it with \
             `luabox publish --yank {version}`"
        );
    }
    let deps = index_deps(project)?;

    // Gate 1: `luabox check` must be green.
    println!("publish: running `luabox check`");
    crate::check_cmd::run_once(&project.root, None, "human", None).map_err(|_| {
        anyhow!("publish blocked: `luabox check` reported errors (fix them and retry)")
    })?;

    // Gate 2: tests must pass — skipped with a warning when no Lua runtime
    // is available (publishing from runtime-less machines must work).
    run_test_gate(project)?;

    // Gate 3 (advisory): annotation coverage of the public API.
    report_annotation_coverage(&project.root);

    // Pack the tree, intern it (tree hash = index checksum), tar it, push.
    let staging = tempfile::tempdir().context("cannot create a temp dir for packaging")?;
    let tree_dir = staging.path().join("tree");
    let files = stage_package_tree(project, &tree_dir)?;
    if files == 0 {
        bail!("nothing to publish: no `src/` sources, `.lb` modules, or README found");
    }

    let store = Store::open(deps_cmd::store_root()?);
    let tree = store
        .put_tree(&tree_dir)
        .context("hashing the package tree")?;
    store
        .write_package_manifest(name, version, &tree)
        .context("indexing the package in the local store")?;
    let checksum = format!("sha256:{}", tree.tree_hash);

    let artifact = staging.path().join("package.tar");
    deps_cmd::create_tar(&tree_dir, &artifact)?;

    let entry = IndexEntry {
        name: name.to_owned(),
        version: version.clone(),
        deps,
        lua_versions: project.manifest.package.lua_versions.clone(),
        checksum: checksum.clone(),
        yanked: false,
    };
    registry
        .publish(&entry, &artifact)
        .map_err(|e| anyhow!("{e}"))?;

    println!(
        "published `{name}@{version}` to `{}` ({files} file(s), {checksum})",
        registry.location()
    );
    println!("note: artifact signing (sigstore) is not part of this MVP (SPEC.md §6)");
    Ok(())
}

/// `[dependencies]` as index entries. Path/git/workspace dependencies are
/// refused — a registry consumer cannot resolve them. Version-req
/// dev-dependencies are recorded with `dev: true`; other dev-dependency
/// kinds are skipped with a note (they never affect consumers).
fn index_deps(project: &Project) -> anyhow::Result<Vec<IndexDep>> {
    let mut deps = Vec::new();
    for (dep_name, dep) in &project.manifest.dependencies {
        match dep {
            Dependency::Version(req) => deps.push(IndexDep {
                name: dep_name.clone(),
                req: req.clone(),
                dev: false,
            }),
            _ => bail!(
                "cannot publish: dependency `{dep_name}` is a path/git/workspace \
                 dependency, which registry consumers cannot resolve — publish \
                 it to the registry first and depend on it by version"
            ),
        }
    }
    for (dep_name, dep) in &project.manifest.dev_dependencies {
        match dep {
            Dependency::Version(req) => deps.push(IndexDep {
                name: dep_name.clone(),
                req: req.clone(),
                dev: true,
            }),
            _ => println!(
                "note: dev-dependency `{dep_name}` is not a registry dependency; \
                 leaving it out of the index (dev-dependencies never affect \
                 consumers)"
            ),
        }
    }
    Ok(deps)
}

/// The test gate: run the suite when a runtime exists, warn-and-skip when
/// none does.
fn run_test_gate(project: &Project) -> anyhow::Result<()> {
    let out_dir = project.root.join(&project.manifest.build.out);
    let files = luabox_test::discover(&project.root, Some(out_dir.as_path()));
    if files.is_empty() {
        println!("publish: no test files found; skipping the test gate");
        return Ok(());
    }
    let runtime = match resolve_default(&project.manifest.package.edition, &project.root) {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!(
                "warning: skipping the test gate — no Lua runtime found ({e}); \
                 install one with `luabox toolchain install` to run tests before \
                 publishing"
            );
            return Ok(());
        }
    };
    println!("publish: running `luabox test`");
    let opts = SuiteOptions {
        files: &files,
        pattern: None,
        root: &project.root,
    };
    let report = run_suite(&runtime, &opts);
    let (text, summary) = luabox_test::render(&[report], false);
    print!("{text}");
    if summary.failed > 0 {
        bail!(
            "publish blocked: {} test(s) failed ({} passed)",
            summary.failed,
            summary.passed
        );
    }
    Ok(())
}

// --- packaging ---------------------------------------------------------------

/// Copy the publishable tree into `dest`: `luabox.toml`, root `README*`,
/// everything under `src/`, and any `.lb` shape modules — excluding the
/// build output dir, `lua_modules/`, `vendor/`, and dot-files/dirs.
/// Returns the number of files staged.
fn stage_package_tree(project: &Project, dest: &Path) -> anyhow::Result<usize> {
    let out_dir_name = project.manifest.build.out.clone();
    let mut staged = 0usize;
    let mut stack = vec![PathBuf::new()];
    while let Some(rel_dir) = stack.pop() {
        let abs_dir = project.root.join(&rel_dir);
        let mut entries: Vec<_> = fs::read_dir(&abs_dir)
            .with_context(|| format!("cannot read `{}`", abs_dir.display()))?
            .collect::<Result<_, _>>()
            .with_context(|| format!("cannot read `{}`", abs_dir.display()))?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let rel = rel_dir.join(&name);
            if entry.path().is_dir() {
                let excluded = rel_dir.as_os_str().is_empty()
                    && (name == out_dir_name || name == "lua_modules" || name == "vendor");
                if !excluded {
                    stack.push(rel);
                }
                continue;
            }
            if should_publish_file(&rel, &name) {
                let target = dest.join(&rel);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)
                        .with_context(|| format!("cannot create `{}`", parent.display()))?;
                }
                fs::copy(entry.path(), &target)
                    .with_context(|| format!("cannot stage `{}`", rel.display()))?;
                staged += 1;
            }
        }
    }
    Ok(staged)
}

/// Whether a (non-excluded) file belongs in the published tree.
fn should_publish_file(rel: &Path, name: &str) -> bool {
    let top_level = rel.components().count() == 1;
    let is_lb = rel
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("lb"));
    (top_level && (name == "luabox.toml" || name.starts_with("README")))
        || rel.starts_with("src")
        || is_lb
}

// --- annotation coverage (advisory gate) --------------------------------------

/// A public function found in `src/` without annotations.
struct UndocumentedFn {
    file: String,
    line: usize,
    name: String,
}

/// List public functions in `src/**/*.lua` whose leading comment block has
/// neither `---@param` nor `---@return`, and print them as a warning. A
/// *public* function here is a non-`local` `function` statement
/// (`function M.foo(...)`, `function M:bar()`, `function baz()`) — a
/// deliberately simple, text-level heuristic: this gate is advisory in the
/// MVP and must not require the type checker.
fn report_annotation_coverage(root: &Path) {
    let mut findings = Vec::new();
    collect_undocumented(&root.join("src"), root, &mut findings);
    if findings.is_empty() {
        return;
    }
    eprintln!(
        "warning: {} public function(s) lack ---@param/---@return annotations \
         (annotation coverage is advisory in this MVP; publishing anyway):",
        findings.len()
    );
    for finding in &findings {
        eprintln!(
            "  {}:{}: `{}` is undocumented",
            finding.file, finding.line, finding.name
        );
    }
}

/// Recursively scan `dir` for `.lua` sources and collect undocumented
/// public functions.
fn collect_undocumented(dir: &Path, root: &Path, findings: &mut Vec<UndocumentedFn>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_undocumented(&path, root, findings);
        } else if std::path::Path::new(&name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("lua"))
            && !name.ends_with(".d.lua")
        {
            let Ok(source) = fs::read_to_string(&path) else {
                continue;
            };
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            scan_file_annotations(&source, &rel, findings);
        }
    }
}

/// Line-level scan of one file: for each public `function` statement, check
/// the contiguous comment block directly above it for `@param`/`@return`.
fn scan_file_annotations(source: &str, rel: &str, findings: &mut Vec<UndocumentedFn>) {
    let lines: Vec<&str> = source.lines().collect();
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        // Only lines *beginning* with `function ` count as public API:
        // `local function …` and `x = function(…)` don't match the prefix.
        let Some(rest) = trimmed.strip_prefix("function ") else {
            continue;
        };
        let fn_name: String = rest
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '(')
            .collect();
        if fn_name.is_empty() {
            continue;
        }
        // Walk the contiguous comment block directly above.
        let mut has_annotations = false;
        let mut cursor = idx;
        while cursor > 0 {
            cursor -= 1;
            let above = lines[cursor].trim_start();
            if !above.starts_with("--") {
                break;
            }
            if above.contains("@param") || above.contains("@return") {
                has_annotations = true;
                break;
            }
        }
        if !has_annotations {
            findings.push(UndocumentedFn {
                file: rel.to_owned(),
                line: idx + 1,
                name: fn_name,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{UndocumentedFn, scan_file_annotations, should_publish_file};
    use std::path::Path;

    fn scan(source: &str) -> Vec<String> {
        let mut findings: Vec<UndocumentedFn> = Vec::new();
        scan_file_annotations(source, "src/m.lua", &mut findings);
        findings.into_iter().map(|f| f.name).collect()
    }

    #[test]
    fn annotated_public_functions_pass() {
        let src = "\
local M = {}

---Adds two numbers.
---@param a number
---@param b number
---@return number
function M.add(a, b)
  return a + b
end

return M
";
        assert!(scan(src).is_empty());
    }

    #[test]
    fn undocumented_public_function_is_flagged() {
        let src = "local M = {}\nfunction M.add(a, b)\n  return a + b\nend\nreturn M\n";
        assert_eq!(scan(src), vec!["M.add".to_owned()]);
    }

    #[test]
    fn local_functions_are_not_public_api() {
        let src = "local function helper(x)\n  return x\nend\n";
        assert!(scan(src).is_empty());
    }

    #[test]
    fn publish_file_selection() {
        assert!(should_publish_file(Path::new("luabox.toml"), "luabox.toml"));
        assert!(should_publish_file(Path::new("README.md"), "README.md"));
        assert!(should_publish_file(
            Path::new("src/deep/init.lua"),
            "init.lua"
        ));
        assert!(should_publish_file(
            Path::new("shapes/geometry.lb"),
            "geometry.lb"
        ));
        assert!(!should_publish_file(
            Path::new("scripts/build.lua"),
            "build.lua"
        ));
        assert!(!should_publish_file(
            Path::new("docs/README.md"),
            "README.md"
        ));
    }
}
