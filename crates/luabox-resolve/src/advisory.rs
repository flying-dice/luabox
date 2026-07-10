//! The advisory database — `luabox audit`'s RUSTSEC-analog vulnerability
//! feed (SPEC.md §6, §14).
//!
//! # Format
//!
//! An advisory database is a **directory of TOML files**, one advisory per
//! file, found anywhere under the directory (subdirectories — e.g. grouped
//! by package name, RUSTSEC-style — are walked recursively; a flat directory
//! works just as well). Each file looks like:
//!
//! ```toml
//! id = "LBSEC-2026-0001"
//! package = "insecure-pkg"
//! severity = "high"                        # low | medium | high | critical
//! title = "Remote code execution via eval"
//! description = """
//! `insecure-pkg` evaluates untrusted input passed to `run()`, allowing
//! arbitrary code execution.
//! """
//! url = "https://example.com/advisories/LBSEC-2026-0001"   # optional
//! affected = ["<1.2.3"]                    # required, at least one entry
//! patched = [">=1.2.3"]                    # optional, default: none
//! withdrawn = "2026-03-01"                 # optional; presence retracts it
//! ```
//!
//! - `id` — `LBSEC-YYYY-NNNN` (a 4-digit year, a hyphen, then a numeric
//!   sequence). This crate does not check global uniqueness; that is the
//!   database's curation problem, not the loader's.
//! - `affected` / `patched` — arrays of [`VersionReq`] strings in Cargo's
//!   requirement grammar (the same grammar the resolver's PubGrub layer
//!   speaks — see `semver_ranges`, which is crate-private, so this module
//!   goes straight to the `semver` crate rather than reaching into it).
//!   Each array is an **OR** of its entries (any one matching is enough);
//!   each entry's own comma list is cargo's usual **AND** (e.g.
//!   `">=1.0.0, <1.2.3"`). `patched` is checked *after* `affected` and wins
//!   on overlap — see [`AdvisoryDb::check`].
//! - `withdrawn` — any non-empty string (a date is conventional, but its
//!   value is never parsed as one — only *presence* matters). A withdrawn
//!   advisory is loaded and validated like any other, but never produces a
//!   finding: this preserves history (and the id) instead of deleting it.
//!
//! # Location
//!
//! `luabox audit` (`crate::advisory` consumers, concretely `audit_cmd` in
//! the CLI) resolve the database directory from `LUABOX_ADVISORY_DB`, else
//! `~/.luabox/advisory-db`. **There is no hosted default feed yet** — no
//! advisory content ships with luabox and no network fetch happens. When
//! neither location exists, `luabox audit` prints an informational note and
//! exits `0` rather than failing.
//!
//! This is a deliberate choice, not a stopgap for a future hard requirement:
//! a security check that fails a build/CI pipeline merely because *no
//! database was ever configured* teaches teams to ignore or disable the
//! check, which is worse than not having it. Once the database is present,
//! findings are judged strictly (see severity/exit-code mapping on the CLI
//! side); only its total absence is silent.
//!
//! # Loading and validation
//!
//! [`AdvisoryDb::load`] never fails outright: each `*.toml` file is parsed
//! and validated independently, and a malformed one becomes an
//! [`AdvisoryError`] (message + offending path) rather than aborting the
//! load. One bad file should not blind a project to every other advisory in
//! the database — the caller decides how loud to be about the warnings
//! (`luabox audit` prints them and keeps going).

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use semver::{Version, VersionReq};
use toml_edit::{DocumentMut, Item};

use crate::lockfile::Lockfile;

/// How serious an advisory is. Maps to diagnostic severity on the CLI side:
/// [`AdvisorySeverity::Critical`]/[`AdvisorySeverity::High`] are hard
/// errors, [`AdvisorySeverity::Medium`]/[`AdvisorySeverity::Low`] are
/// warnings (SPEC.md §14 — `LB1100`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AdvisorySeverity {
    Low,
    Medium,
    High,
    Critical,
}

impl AdvisorySeverity {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }
}

impl fmt::Display for AdvisorySeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        })
    }
}

/// One parsed, validated advisory (one `*.toml` file in the database).
#[derive(Debug, Clone)]
pub struct Advisory {
    /// `LBSEC-YYYY-NNNN`.
    pub id: String,
    /// The exact package name this advisory targets (matched literally
    /// against `luabox.lock` entries — no glob/prefix matching).
    pub package: String,
    pub severity: AdvisorySeverity,
    pub title: String,
    pub description: String,
    pub url: Option<String>,
    /// Version requirements a locked version must match *any* of to be
    /// potentially affected. Never empty for a successfully parsed advisory.
    pub affected: Vec<VersionReq>,
    /// Version requirements that, if matched by *any* entry, exclude an
    /// otherwise-affected version (the fix landed there).
    pub patched: Vec<VersionReq>,
    /// Presence (any non-empty string) marks the advisory withdrawn: loaded
    /// and displayed by database tooling, but never produces a finding.
    pub withdrawn: Option<String>,
}

/// A DB entry that failed to load or validate. Carries the offending file so
/// `luabox audit` can report `warning: <path>: <message>` without failing
/// the rest of the audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvisoryError {
    pub path: PathBuf,
    pub message: String,
}

impl fmt::Display for AdvisoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

impl std::error::Error for AdvisoryError {}

/// A locked package matched against a database: `advisory` applies to
/// `package`'s locked `version`.
#[derive(Debug, Clone)]
pub struct Finding {
    pub package: String,
    pub version: Version,
    pub advisory: Advisory,
}

/// A loaded advisory database — the in-memory result of [`AdvisoryDb::load`].
#[derive(Debug, Clone, Default)]
pub struct AdvisoryDb {
    advisories: Vec<Advisory>,
}

impl AdvisoryDb {
    /// Loads every `*.toml` file found recursively under `dir`. Malformed
    /// files are collected as [`AdvisoryError`]s and skipped; a directory
    /// that cannot be read at all (or a subdirectory within it) simply
    /// yields no advisories from that location rather than failing — callers
    /// that need to distinguish "no database configured" from "database
    /// directory is unreadable" should check `dir` themselves before calling
    /// (`luabox audit` does, see `audit_cmd::run`).
    #[must_use]
    pub fn load(dir: &Path) -> (Self, Vec<AdvisoryError>) {
        let mut files = Vec::new();
        collect_toml_files(dir, &mut files);
        files.sort();

        let mut advisories = Vec::new();
        let mut errors = Vec::new();
        for path in files {
            match fs::read_to_string(&path) {
                Ok(text) => match parse_advisory(&text) {
                    Ok(advisory) => advisories.push(advisory),
                    Err(message) => errors.push(AdvisoryError { path, message }),
                },
                Err(e) => errors.push(AdvisoryError {
                    path,
                    message: format!("cannot read file: {e}"),
                }),
            }
        }
        (Self { advisories }, errors)
    }

    /// The number of successfully loaded advisories.
    #[must_use]
    pub fn len(&self) -> usize {
        self.advisories.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.advisories.is_empty()
    }

    /// Every successfully loaded advisory, in load order (sorted by file
    /// path — see [`AdvisoryDb::load`]).
    #[must_use]
    pub fn advisories(&self) -> &[Advisory] {
        &self.advisories
    }

    /// Matches every locked package in `lockfile` against the database.
    ///
    /// A `(package, advisory)` pair is a finding iff:
    ///
    /// 1. the advisory's `package` equals the locked package's name;
    /// 2. the advisory is not withdrawn;
    /// 3. the locked version matches *any* of `affected`'s requirements
    ///    (cargo/`semver`-crate matching semantics — pre-release versions
    ///    only match a requirement that explicitly opts into pre-releases
    ///    of that `major.minor.patch`, same as the resolver's own
    ///    requirement handling); and
    /// 4. the locked version matches *none* of `patched`'s requirements.
    ///
    /// Results are sorted by package name, then advisory id, for
    /// deterministic output.
    #[must_use]
    pub fn check(&self, lockfile: &Lockfile) -> Vec<Finding> {
        let mut findings: Vec<Finding> = lockfile
            .packages
            .iter()
            .flat_map(|package| {
                self.advisories
                    .iter()
                    .filter(|advisory| is_vulnerable(advisory, &package.name, &package.version))
                    .map(|advisory| Finding {
                        package: package.name.clone(),
                        version: package.version.clone(),
                        advisory: advisory.clone(),
                    })
            })
            .collect();
        findings.sort_by(|a, b| (&a.package, &a.advisory.id).cmp(&(&b.package, &b.advisory.id)));
        findings
    }
}

fn is_vulnerable(advisory: &Advisory, name: &str, version: &Version) -> bool {
    if advisory.package != name || advisory.withdrawn.is_some() {
        return false;
    }
    let affected = advisory.affected.iter().any(|req| req.matches(version));
    affected && !advisory.patched.iter().any(|req| req.matches(version))
}

/// Recursively collects every `*.toml` file under `dir`. Unreadable
/// directories (missing, permission-denied, ...) simply contribute nothing.
fn collect_toml_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_toml_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            out.push(path);
        }
    }
}

fn parse_advisory(text: &str) -> Result<Advisory, String> {
    let doc: DocumentMut = text.parse().map_err(|e| format!("invalid TOML: {e}"))?;

    let id = get_str(&doc, "id")?;
    validate_id(&id)?;
    let package = get_str(&doc, "package")?;
    if package.trim().is_empty() {
        return Err("`package` must not be empty".to_owned());
    }
    let severity_text = get_str(&doc, "severity")?;
    let severity = AdvisorySeverity::parse(&severity_text).ok_or_else(|| {
        format!("`severity` must be one of low, medium, high, critical (found `{severity_text}`)")
    })?;
    let title = get_str(&doc, "title")?;
    if title.trim().is_empty() {
        return Err("`title` must not be empty".to_owned());
    }
    let description = get_str(&doc, "description")?;
    if description.trim().is_empty() {
        return Err("`description` must not be empty".to_owned());
    }
    let url = get_opt_str(&doc, "url")?;
    let affected = get_req_array(&doc, "affected")?;
    if affected.is_empty() {
        return Err("`affected` must list at least one version requirement".to_owned());
    }
    let patched = get_req_array(&doc, "patched")?;
    let withdrawn = get_opt_str(&doc, "withdrawn")?.filter(|s| !s.trim().is_empty());

    Ok(Advisory {
        id,
        package,
        severity,
        title,
        description,
        url,
        affected,
        patched,
        withdrawn,
    })
}

/// `LBSEC-YYYY-NNNN`: a literal `LBSEC-` prefix, a 4-digit year, a hyphen,
/// then a non-empty numeric sequence.
fn validate_id(id: &str) -> Result<(), String> {
    let bad = || format!("`id` \"{id}\" must look like LBSEC-YYYY-NNNN");
    let rest = id.strip_prefix("LBSEC-").ok_or_else(bad)?;
    let (year, seq) = rest.split_once('-').ok_or_else(bad)?;
    let valid = year.len() == 4
        && year.bytes().all(|b| b.is_ascii_digit())
        && !seq.is_empty()
        && seq.bytes().all(|b| b.is_ascii_digit());
    if valid { Ok(()) } else { Err(bad()) }
}

fn get_str(doc: &DocumentMut, key: &str) -> Result<String, String> {
    doc.get(key)
        .and_then(Item::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("missing or non-string `{key}`"))
}

fn get_opt_str(doc: &DocumentMut, key: &str) -> Result<Option<String>, String> {
    match doc.get(key) {
        None => Ok(None),
        Some(item) => item
            .as_str()
            .map(|s| Some(s.to_owned()))
            .ok_or_else(|| format!("`{key}` must be a string")),
    }
}

fn get_req_array(doc: &DocumentMut, key: &str) -> Result<Vec<VersionReq>, String> {
    let Some(item) = doc.get(key) else {
        return Ok(Vec::new());
    };
    let array = item
        .as_array()
        .ok_or_else(|| format!("`{key}` must be an array of version requirements"))?;
    let mut out = Vec::with_capacity(array.len());
    for value in array {
        let text = value
            .as_str()
            .ok_or_else(|| format!("`{key}` entries must be strings"))?;
        let req = VersionReq::parse(text).map_err(|e| {
            format!("`{key}` entry `{text}` is not a valid version requirement: {e}")
        })?;
        out.push(req);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, content: &str) {
        fs::write(dir.join(name), content).expect("write advisory fixture");
    }

    fn advisory(affected: &[&str], patched: &[&str]) -> Advisory {
        Advisory {
            id: "LBSEC-2026-0001".to_owned(),
            package: "insecure-pkg".to_owned(),
            severity: AdvisorySeverity::High,
            title: "title".to_owned(),
            description: "description".to_owned(),
            url: None,
            affected: affected
                .iter()
                .map(|s| VersionReq::parse(s).unwrap())
                .collect(),
            patched: patched
                .iter()
                .map(|s| VersionReq::parse(s).unwrap())
                .collect(),
            withdrawn: None,
        }
    }

    fn v(s: &str) -> Version {
        Version::parse(s).expect("valid version")
    }

    // --- id / severity / structural validation ----------------------------

    #[test]
    fn valid_id_forms() {
        assert!(validate_id("LBSEC-2026-0001").is_ok());
        assert!(validate_id("LBSEC-2026-999999").is_ok());
    }

    #[test]
    fn rejects_malformed_ids() {
        for bad in [
            "LBSEC-26-0001",
            "RUSTSEC-2026-0001",
            "LBSEC-2026",
            "LBSEC-2026-",
            "LBSEC-abcd-0001",
            "lbsec-2026-0001",
            "",
        ] {
            assert!(validate_id(bad).is_err(), "should reject `{bad}`");
        }
    }

    const VALID: &str = "\
id = \"LBSEC-2026-0001\"
package = \"insecure-pkg\"
severity = \"high\"
title = \"Remote code execution\"
description = \"insecure-pkg evaluates untrusted input.\"
url = \"https://example.com/advisories/LBSEC-2026-0001\"
affected = [\"<1.2.3\"]
patched = [\">=1.2.3\"]
";

    #[test]
    fn parses_a_well_formed_advisory() {
        let advisory = parse_advisory(VALID).expect("parses");
        assert_eq!(advisory.id, "LBSEC-2026-0001");
        assert_eq!(advisory.package, "insecure-pkg");
        assert_eq!(advisory.severity, AdvisorySeverity::High);
        assert_eq!(
            advisory.url.as_deref(),
            Some("https://example.com/advisories/LBSEC-2026-0001")
        );
        assert_eq!(advisory.affected.len(), 1);
        assert_eq!(advisory.patched.len(), 1);
        assert!(advisory.withdrawn.is_none());
    }

    #[test]
    fn withdrawn_field_is_recorded() {
        let text = format!("{VALID}withdrawn = \"2026-03-01\"\n");
        let advisory = parse_advisory(&text).expect("parses");
        assert_eq!(advisory.withdrawn.as_deref(), Some("2026-03-01"));
    }

    #[test]
    fn rejects_missing_required_fields() {
        for key in [
            "id",
            "package",
            "severity",
            "title",
            "description",
            "affected",
        ] {
            let text: String = VALID
                .lines()
                .filter(|line| !line.starts_with(&format!("{key} ")))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                parse_advisory(&text).is_err(),
                "should reject a missing `{key}`"
            );
        }
    }

    #[test]
    fn rejects_bad_severity() {
        let text = VALID.replace("severity = \"high\"", "severity = \"extreme\"");
        let err = parse_advisory(&text).unwrap_err();
        assert!(err.contains("severity"), "{err}");
    }

    #[test]
    fn rejects_empty_affected() {
        let text = VALID.replace("affected = [\"<1.2.3\"]", "affected = []");
        assert!(parse_advisory(&text).is_err());
    }

    #[test]
    fn rejects_bad_version_requirement() {
        let text = VALID.replace("affected = [\"<1.2.3\"]", "affected = [\"not-a-req\"]");
        let err = parse_advisory(&text).unwrap_err();
        assert!(err.contains("affected"), "{err}");
    }

    #[test]
    fn rejects_invalid_toml() {
        assert!(parse_advisory("this is not toml [[[").is_err());
    }

    // --- range matching (table-driven) -------------------------------------

    #[test]
    fn affected_range_matches() {
        let cases: &[(&[&str], &[&str], &str, bool)] = &[
            // affected, patched, version, expect vulnerable
            (&["<1.2.3"], &[], "1.0.0", true),
            (&["<1.2.3"], &[], "1.2.3", false),
            (&["<1.2.3"], &[], "1.2.4", false),
            (&[">=1.0.0, <2.0.0"], &[], "1.5.0", true),
            (&[">=1.0.0, <2.0.0"], &[], "2.0.0", false),
            // OR across multiple affected entries
            (&["<1.0.0", ">=2.0.0, <2.0.5"], &[], "0.9.0", true),
            (&["<1.0.0", ">=2.0.0, <2.0.5"], &[], "2.0.1", true),
            (&["<1.0.0", ">=2.0.0, <2.0.5"], &[], "1.5.0", false),
            // patched exclusion overrides a broad affected range
            (&["<9.9.9"], &[">=1.2.3"], "1.5.0", false),
            (&["<9.9.9"], &[">=1.2.3"], "1.0.0", true),
            // OR across multiple patched entries
            (&["<9.9.9"], &["<1.0.0", ">=2.0.0"], "0.5.0", false),
            (&["<9.9.9"], &["<1.0.0", ">=2.0.0"], "2.5.0", false),
            (&["<9.9.9"], &["<1.0.0", ">=2.0.0"], "1.5.0", true),
        ];
        for (affected, patched, version, expect_vulnerable) in cases {
            let advisory = advisory(affected, patched);
            let actual = is_vulnerable(&advisory, "insecure-pkg", &v(version));
            assert_eq!(
                actual, *expect_vulnerable,
                "affected={affected:?} patched={patched:?} version={version}"
            );
        }
    }

    #[test]
    fn prerelease_needs_explicit_opt_in() {
        // A plain `<2.0.0` (implicitly non-pre-release) does not match an
        // interior pre-release, matching cargo/semver-crate semantics.
        let plain = advisory(&["<2.0.0"], &[]);
        assert!(!is_vulnerable(&plain, "insecure-pkg", &v("1.5.0-alpha")));
        // An explicit pre-release bound does opt in.
        let with_prerelease_bound = advisory(&[">=1.0.0-alpha, <1.0.0"], &[]);
        assert!(is_vulnerable(
            &with_prerelease_bound,
            "insecure-pkg",
            &v("1.0.0-alpha.1")
        ));
    }

    #[test]
    fn wrong_package_name_never_matches() {
        let advisory = advisory(&["<9.9.9"], &[]);
        assert!(!is_vulnerable(&advisory, "other-pkg", &v("1.0.0")));
    }

    #[test]
    fn withdrawn_advisory_is_skipped() {
        let mut advisory = advisory(&["<9.9.9"], &[]);
        advisory.withdrawn = Some("2026-03-01".to_owned());
        assert!(!is_vulnerable(&advisory, "insecure-pkg", &v("1.0.0")));
    }

    // --- database load + lockfile matching ----------------------------------

    use crate::lockfile::{LockedPackage, LockedSource};

    fn lockfile(entries: &[(&str, &str)]) -> Lockfile {
        Lockfile::new(
            entries
                .iter()
                .map(|(name, version)| LockedPackage {
                    name: (*name).to_owned(),
                    version: v(version),
                    source: Some(LockedSource::Registry),
                    checksum: None,
                    dependencies: Vec::new(),
                })
                .collect(),
        )
    }

    #[test]
    fn load_reads_every_toml_file_recursively() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "one.toml", VALID);
        fs::create_dir(dir.path().join("nested")).expect("mkdir");
        let nested_advisory = VALID.replace("LBSEC-2026-0001", "LBSEC-2026-0002");
        write(&dir.path().join("nested"), "two.toml", &nested_advisory);

        let (db, errors) = AdvisoryDb::load(dir.path());
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(db.len(), 2);
    }

    #[test]
    fn load_collects_malformed_files_as_warnings_not_fatal() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "good.toml", VALID);
        write(dir.path(), "bad.toml", "id = \"nonsense\"\n");

        let (db, errors) = AdvisoryDb::load(dir.path());
        assert_eq!(db.len(), 1, "the good advisory still loads");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].path.ends_with("bad.toml"));
    }

    #[test]
    fn load_of_missing_directory_yields_an_empty_db() {
        let (db, errors) = AdvisoryDb::load(Path::new("this/does/not/exist"));
        assert!(db.is_empty());
        assert!(errors.is_empty());
    }

    #[test]
    fn check_matches_by_name_and_version_and_skips_the_rest() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "vuln.toml", VALID); // insecure-pkg, affected <1.2.3
        let (db, errors) = AdvisoryDb::load(dir.path());
        assert!(errors.is_empty());

        let lock = lockfile(&[
            ("insecure-pkg", "1.0.0"), // affected
            ("other-pkg", "1.0.0"),    // different package, never a finding
        ]);
        let findings = db.check(&lock);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].package, "insecure-pkg");
        assert_eq!(findings[0].version, v("1.0.0"));
        assert_eq!(findings[0].advisory.id, "LBSEC-2026-0001");
    }

    #[test]
    fn check_is_empty_for_a_clean_lockfile() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "vuln.toml", VALID);
        let (db, _) = AdvisoryDb::load(dir.path());

        let lock = lockfile(&[("some-other-pkg", "1.0.0")]);
        assert!(db.check(&lock).is_empty());
    }

    #[test]
    fn check_is_sorted_deterministically() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "a.toml", VALID);
        let b = VALID
            .replace("LBSEC-2026-0001", "LBSEC-2026-0000")
            .replace("insecure-pkg", "zzz-pkg");
        write(dir.path(), "b.toml", &b);
        let (db, errors) = AdvisoryDb::load(dir.path());
        assert!(errors.is_empty());

        let lock = lockfile(&[("zzz-pkg", "1.0.0"), ("insecure-pkg", "1.0.0")]);
        let findings = db.check(&lock);
        assert_eq!(findings.len(), 2);
        // "insecure-pkg" sorts before "zzz-pkg" regardless of lockfile order.
        assert_eq!(findings[0].package, "insecure-pkg");
        assert_eq!(findings[1].package, "zzz-pkg");
    }
}
