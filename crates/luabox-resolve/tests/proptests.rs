//! SPEC.md §16.2 property tests: lockfile determinism and solver
//! idempotence over randomly generated dependency universes.
//!
//! - **Determinism**: resolving the same universe twice yields
//!   byte-identical lockfiles (or byte-identical conflict reports).
//! - **Idempotence**: resolve → write lock → re-resolve with lock yields
//!   zero changes.
//! - **Soundness**: every solution respects every requirement
//!   ([`verify_resolution`]).

// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]

use std::fmt::Write as _;
use std::path::Path;

use luabox_resolve::{Manifest, StaticProvider, resolve, verify_resolution};
use proptest::prelude::*;

/// How a generated dependency edge constrains its target.
#[derive(Debug, Clone, Copy)]
enum ReqKind {
    CaretMajor,
    Exact,
    AtLeast,
    CaretMinor,
}

/// A generated dependency universe: `packages[i]` is a list of
/// `(major, minor)` versions; `edges[i][j]` (only meaningful for `j < i`,
/// keeping the graph acyclic) says how each version of package `i` depends
/// on package `j`; `roots[i]` says whether the project depends on `i`.
#[derive(Debug, Clone)]
struct Universe {
    packages: Vec<Vec<(u64, u64)>>,
    edges: Vec<Vec<Option<(ReqKind, u8)>>>,
    roots: Vec<bool>,
}

const MAX_PACKAGES: usize = 6;

fn arb_req_kind() -> impl Strategy<Value = ReqKind> {
    prop_oneof![
        Just(ReqKind::CaretMajor),
        Just(ReqKind::Exact),
        Just(ReqKind::AtLeast),
        Just(ReqKind::CaretMinor),
    ]
}

fn arb_universe() -> impl Strategy<Value = Universe> {
    let versions = proptest::collection::btree_set((1u64..=3, 0u64..=2), 1..=3)
        .prop_map(|set| set.into_iter().collect::<Vec<_>>());
    let packages = proptest::collection::vec(versions, 1..=MAX_PACKAGES);
    let edge = proptest::option::weighted(0.4, (arb_req_kind(), any::<u8>()));
    let edges =
        proptest::collection::vec(proptest::collection::vec(edge, MAX_PACKAGES), MAX_PACKAGES);
    let roots = proptest::collection::vec(any::<bool>(), MAX_PACKAGES);
    (packages, edges, roots).prop_map(|(packages, edges, roots)| Universe {
        packages,
        edges,
        roots,
    })
}

fn pkg_name(index: usize) -> String {
    format!("pkg{index}")
}

fn req_for(kind: ReqKind, pick: u8, versions: &[(u64, u64)]) -> String {
    let (major, minor) = versions[usize::from(pick) % versions.len()];
    match kind {
        ReqKind::CaretMajor => format!("^{major}"),
        ReqKind::Exact => format!("={major}.{minor}.0"),
        ReqKind::AtLeast => format!(">={major}.{minor}.0"),
        ReqKind::CaretMinor => format!("^{major}.{minor}"),
    }
}

/// Materializes the universe into a provider plus a root manifest.
fn materialize(universe: &Universe) -> (StaticProvider, Manifest) {
    let mut provider = StaticProvider::new();
    for (i, versions) in universe.packages.iter().enumerate() {
        for (major, minor) in versions {
            // Each version of package i depends on a generated subset of
            // lower-indexed packages (acyclic by construction; PubGrub
            // handles cycles, this just keeps universes mostly solvable).
            let deps: Vec<(String, String)> = (0..i)
                .filter_map(|j| {
                    universe.edges[i][j].map(|(kind, pick)| {
                        (pkg_name(j), req_for(kind, pick, &universe.packages[j]))
                    })
                })
                .collect();
            let dep_refs: Vec<(&str, &str)> = deps
                .iter()
                .map(|(name, req)| (name.as_str(), req.as_str()))
                .collect();
            provider.add(&pkg_name(i), &format!("{major}.{minor}.0"), &dep_refs);
        }
    }

    let mut dependencies = String::new();
    let mut any_root = false;
    for (i, versions) in universe.packages.iter().enumerate() {
        if universe.roots[i] {
            any_root = true;
            let (major, _) = versions[0];
            let _ = writeln!(dependencies, "{} = \"^{major}\"", pkg_name(i));
        }
    }
    if !any_root {
        // Guarantee at least one root dependency so resolution is not
        // trivially empty.
        let (major, _) = universe.packages[0][0];
        let _ = writeln!(dependencies, "{} = \"^{major}\"", pkg_name(0));
    }

    let manifest = Manifest::parse(&format!(
        "[package]\nname = \"proproot\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n[dependencies]\n{dependencies}"
    ))
    .expect("generated root manifest is valid");
    (provider, manifest)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Resolving the same inputs twice is bit-for-bit deterministic —
    /// for successes (identical lockfiles) *and* failures (identical
    /// conflict reports).
    #[test]
    fn resolution_is_deterministic(universe in arb_universe()) {
        let (provider, manifest) = materialize(&universe);
        let first = resolve(&manifest, Path::new("."), &provider, None);
        let second = resolve(&manifest, Path::new("."), &provider, None);
        match (first, second) {
            (Ok(a), Ok(b)) => {
                prop_assert_eq!(a.lockfile.to_toml_string(), b.lockfile.to_toml_string());
                prop_assert_eq!(a.packages, b.packages);
            }
            (Err(a), Err(b)) => prop_assert_eq!(a.to_string(), b.to_string()),
            (a, b) => prop_assert!(false, "diverging outcomes: {a:?} vs {b:?}"),
        }
    }

    /// resolve → lock → resolve-with-lock changes nothing, and the
    /// lockfile round-trips through text losslessly.
    #[test]
    fn locked_re_resolve_is_idempotent(universe in arb_universe()) {
        let (provider, manifest) = materialize(&universe);
        let Ok(first) = resolve(&manifest, Path::new("."), &provider, None) else {
            return Ok(()); // Unsolvable universes are covered above.
        };
        let text = first.lockfile.to_toml_string();
        let reloaded = luabox_resolve::Lockfile::parse(&text)
            .expect("generated lockfile parses");
        prop_assert_eq!(&reloaded, &first.lockfile);
        prop_assert_eq!(reloaded.to_toml_string(), text.clone());

        let second = resolve(&manifest, Path::new("."), &provider, Some(&reloaded))
            .expect("lock-driven re-resolve succeeds");
        prop_assert_eq!(second.lockfile.to_toml_string(), text);
    }

    /// Every solution respects every requirement (and lua-versions).
    #[test]
    fn solutions_satisfy_all_requirements(universe in arb_universe()) {
        let (provider, manifest) = materialize(&universe);
        if let Ok(resolution) = resolve(&manifest, Path::new("."), &provider, None) {
            let verdict = verify_resolution(&manifest, Path::new("."), &provider, &resolution);
            prop_assert!(verdict.is_ok(), "invalid solution: {}", verdict.unwrap_err());
        }
    }
}
