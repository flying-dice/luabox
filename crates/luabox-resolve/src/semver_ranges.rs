//! Cargo-compatible semver requirement semantics over [`pubgrub::Ranges`]
//! (SPEC.md §6: "full semver").
//!
//! PubGrub reasons about mathematically closed version *sets*; cargo's
//! requirement grammar (`^` default, `=`, `>=`, `<`, `~`, wildcards,
//! comma-conjunction) is translated here into [`Ranges<semver::Version>`].
//!
//! Pre-release semantics follow cargo/the `semver` crate: a pre-release
//! version is only eligible when the requirement explicitly names a
//! pre-release of the *same* `major.minor.patch` triple. Two mechanisms
//! encode this:
//!
//! - Exclusive upper bounds use a `-0` sentinel (`<2.0.0` becomes
//!   `<2.0.0-0`), so `2.0.0-alpha` never sneaks under a plain `<2.0.0`.
//! - [`version_matches`] gates pre-release versions on the range having a
//!   bound with a real (non-sentinel) pre-release of the same triple. This
//!   handles "interior" pre-releases: `^1.0` mathematically spans
//!   `1.5.0-alpha`, but cargo would never select it.
//!
//! [`req_to_ranges`] is exhaustively cross-checked against
//! [`semver::VersionReq::matches`] in the tests below.

use std::fmt::Write as _;
use std::ops::Bound;

use pubgrub::Ranges;
use semver::{BuildMetadata, Comparator, Op, Prerelease, Version, VersionReq};

/// The version-set type the solver runs PubGrub over.
pub(crate) type VersionRanges = Ranges<Version>;

/// The `-0` sentinel: the smallest possible pre-release of a triple, used
/// as an exclusive bound that also excludes that triple's pre-releases.
fn sentinel(major: u64, minor: u64, patch: u64) -> Version {
    #[expect(
        clippy::expect_used,
        reason = "`0` is a compile-time-constant valid pre-release identifier"
    )]
    let pre = Prerelease::new("0").expect("`0` is a valid pre-release");
    Version {
        major,
        minor,
        patch,
        pre,
        build: BuildMetadata::EMPTY,
    }
}

fn plain(major: u64, minor: u64, patch: u64) -> Version {
    Version::new(major, minor, patch)
}

/// Increments a version component, refusing versions so large that forming
/// the `+1` upper bound would overflow `u64`.
///
/// Every `^`/`~`/`=`/`<=`/`>` bound in this module is built from a
/// neighbouring version whose leftmost varying component is one greater than
/// the requirement's (`^1.2.3` → `<2.0.0`, `~1.2` → `<1.3.0`, …). A component
/// of `u64::MAX` makes that `+1` overflow. Such a value is not a real release
/// — it only arrives from an adversarial manifest or registry entry — so this
/// rejects it with a parse error rather than panicking (debug) or wrapping to
/// a nonsensical range (release). The error flows out through
/// [`req_to_ranges`]'s `Result`.
fn bump(component: u64) -> Result<u64, String> {
    component.checked_add(1).ok_or_else(|| {
        format!("version component {component} is too large to form a requirement bound")
    })
}

/// The fully-specified version a comparator names (requires minor + patch).
fn base_version(c: &Comparator) -> Version {
    Version {
        major: c.major,
        minor: c.minor.unwrap_or(0),
        patch: c.patch.unwrap_or(0),
        pre: c.pre.clone(),
        build: BuildMetadata::EMPTY,
    }
}

/// Converts a full [`VersionReq`] (comma = conjunction) into ranges.
///
/// Errors only on comparator operators this translation does not know —
/// `semver::Op` is `#[non_exhaustive]`, so a future `semver` minor release
/// could add one.
pub(crate) fn req_to_ranges(req: &VersionReq) -> Result<VersionRanges, String> {
    let mut ranges = Ranges::full();
    for comparator in &req.comparators {
        ranges = ranges.intersection(&comparator_to_ranges(comparator)?);
    }
    Ok(ranges)
}

fn comparator_to_ranges(c: &Comparator) -> Result<VersionRanges, String> {
    let major = c.major;
    Ok(match c.op {
        // `=1.2.3` is a singleton; `=1.2` / `1.2.*` pin the pair; `=1` /
        // `1.*` pin the major.
        Op::Exact | Op::Wildcard => match (c.minor, c.patch) {
            (Some(_), Some(_)) => Ranges::singleton(base_version(c)),
            (Some(minor), None) => {
                Ranges::between(plain(major, minor, 0), sentinel(major, bump(minor)?, 0))
            }
            (None, _) => Ranges::between(plain(major, 0, 0), sentinel(bump(major)?, 0, 0)),
        },
        Op::Greater => match (c.minor, c.patch) {
            (Some(_), Some(_)) => Ranges::strictly_higher_than(base_version(c)),
            (Some(minor), None) => Ranges::higher_than(plain(major, bump(minor)?, 0)),
            (None, _) => Ranges::higher_than(plain(bump(major)?, 0, 0)),
        },
        Op::GreaterEq => match (c.minor, c.patch) {
            (Some(_), Some(_)) => Ranges::higher_than(base_version(c)),
            (Some(minor), None) => Ranges::higher_than(plain(major, minor, 0)),
            (None, _) => Ranges::higher_than(plain(major, 0, 0)),
        },
        Op::Less => match (c.minor, c.patch) {
            (Some(minor), Some(patch)) => {
                if c.pre.is_empty() {
                    Ranges::strictly_lower_than(sentinel(major, minor, patch))
                } else {
                    Ranges::strictly_lower_than(base_version(c))
                }
            }
            (Some(minor), None) => Ranges::strictly_lower_than(sentinel(major, minor, 0)),
            (None, _) => Ranges::strictly_lower_than(sentinel(major, 0, 0)),
        },
        Op::LessEq => match (c.minor, c.patch) {
            (Some(_), Some(_)) => Ranges::lower_than(base_version(c)),
            (Some(minor), None) => Ranges::strictly_lower_than(sentinel(major, bump(minor)?, 0)),
            (None, _) => Ranges::strictly_lower_than(sentinel(bump(major)?, 0, 0)),
        },
        Op::Tilde => match (c.minor, c.patch) {
            (Some(minor), Some(_)) => {
                Ranges::between(base_version(c), sentinel(major, bump(minor)?, 0))
            }
            (Some(minor), None) => {
                Ranges::between(plain(major, minor, 0), sentinel(major, bump(minor)?, 0))
            }
            (None, _) => Ranges::between(plain(major, 0, 0), sentinel(bump(major)?, 0, 0)),
        },
        // Caret: compatible within the leftmost non-zero component.
        Op::Caret => match (c.minor, c.patch) {
            (Some(minor), Some(patch)) => {
                let upper = if major > 0 {
                    sentinel(bump(major)?, 0, 0)
                } else if minor > 0 {
                    sentinel(0, bump(minor)?, 0)
                } else {
                    sentinel(0, 0, bump(patch)?)
                };
                Ranges::between(base_version(c), upper)
            }
            (Some(minor), None) => {
                let upper = if major > 0 {
                    sentinel(bump(major)?, 0, 0)
                } else {
                    sentinel(0, bump(minor)?, 0)
                };
                Ranges::between(plain(major, minor, 0), upper)
            }
            (None, _) => Ranges::between(plain(major, 0, 0), sentinel(bump(major)?, 0, 0)),
        },
        // `semver::Op` is non-exhaustive; refuse rather than mis-resolve.
        op => return Err(format!("unsupported version-requirement operator {op:?}")),
    })
}

/// Cargo-semantics membership test: mathematical containment plus the
/// pre-release opt-in rule.
pub(crate) fn version_matches(ranges: &VersionRanges, version: &Version) -> bool {
    ranges.contains(version) && (version.pre.is_empty() || permits_prerelease(ranges, version))
}

/// True when some bound of `ranges` is a real (non-sentinel) pre-release of
/// `version`'s `major.minor.patch` triple — i.e. the requirement explicitly
/// asked for pre-releases there.
fn permits_prerelease(ranges: &VersionRanges, version: &Version) -> bool {
    ranges
        .iter()
        .flat_map(|(low, high)| [low, high])
        .any(|bound| match bound {
            Bound::Included(b) | Bound::Excluded(b) => {
                !b.pre.is_empty()
                    && b.pre.as_str() != "0"
                    && b.major == version.major
                    && b.minor == version.minor
                    && b.patch == version.patch
            }
            Bound::Unbounded => false,
        })
}

/// Human-readable range rendering for conflict reports: `-0` sentinels are
/// stripped (`<2.0.0-0` reads `<2.0.0`), a singleton reads as the bare
/// version, and the full range reads `*`.
pub(crate) fn display_ranges(ranges: &VersionRanges) -> String {
    if ranges == &Ranges::full() {
        return "*".to_owned();
    }
    if let Some(version) = ranges.as_singleton() {
        return version.to_string();
    }
    let segments: Vec<String> = ranges.iter().map(display_segment).collect();
    if segments.is_empty() {
        "<no versions>".to_owned()
    } else {
        segments.join(" or ")
    }
}

fn display_segment((low, high): (&Bound<Version>, &Bound<Version>)) -> String {
    let mut out = String::new();
    match low {
        Bound::Included(v) => {
            let _ = write!(out, ">={v}");
        }
        Bound::Excluded(v) => {
            let _ = write!(out, ">{v}");
        }
        Bound::Unbounded => {}
    }
    match high {
        Bound::Included(v) => {
            if !out.is_empty() {
                out.push_str(", ");
            }
            let _ = write!(out, "<={v}");
        }
        Bound::Excluded(v) => {
            if !out.is_empty() {
                out.push_str(", ");
            }
            let _ = write!(out, "<{}", strip_sentinel(v));
        }
        Bound::Unbounded => {}
    }
    if out.is_empty() {
        // Both unbounded is the full range, handled by the caller; keep a
        // defensive rendering anyway.
        out.push('*');
    }
    out
}

/// Renders `x.y.z-0` (our exclusive-bound sentinel) as `x.y.z`.
fn strip_sentinel(version: &Version) -> String {
    if version.pre.as_str() == "0" {
        format!("{}.{}.{}", version.major, version.minor, version.patch)
    } else {
        version.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranges(req: &str) -> VersionRanges {
        req_to_ranges(&VersionReq::parse(req).expect("valid req")).expect("supported operators")
    }

    fn v(s: &str) -> Version {
        Version::parse(s).expect("valid version")
    }

    /// The load-bearing test: every requirement form agrees with
    /// `semver::VersionReq::matches` (cargo's own semantics) across a
    /// version matrix that includes pre-releases and sentinel lookalikes.
    #[test]
    fn agrees_with_semver_crate_matches() {
        let reqs = [
            "1",
            "1.2",
            "1.2.3",
            "^1.2.3",
            "^0.2.3",
            "^0.0.3",
            "^0.0",
            "^0",
            "~1.2.3",
            "~1.2",
            "~1",
            "=1.2.3",
            "=1.2",
            "=1",
            ">=1.2.3",
            ">1.2.3",
            "<1.2.3",
            "<=1.2.3",
            ">=1.2",
            ">1.2",
            "<1.2",
            "<=1.2",
            ">=1",
            ">1",
            "<1",
            "<=1",
            "*",
            "1.*",
            "1.2.*",
            ">=1.2.3, <1.8",
            ">=1.2, <2",
            ">=1.0.0-alpha",
            ">1.0.0-alpha",
            "<2.0.0-beta",
            "<=2.0.0-beta.2",
            "=2.0.0-beta.2",
            "~2.1.0-rc.1",
            "^1.0.0-alpha.1",
            "^0.2.3-beta",
        ];
        let versions = [
            "0.0.1",
            "0.0.3",
            "0.0.4",
            "0.1.0",
            "0.2.3",
            "0.2.9",
            "0.3.0",
            "0.9.9",
            "1.0.0",
            "1.0.1",
            "1.2.0",
            "1.2.3",
            "1.2.4",
            "1.3.0",
            "1.8.0",
            "1.9.9",
            "2.0.0",
            "2.0.1",
            "2.1.0",
            "3.0.0",
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-beta",
            "1.2.3-rc.1",
            "1.2.4-0",
            "0.2.3-beta.4",
            "2.0.0-beta.2",
            "2.0.0-beta.3",
            "2.1.0-rc.1",
            "2.1.0-rc.2",
            "3.0.0-alpha",
        ];
        for req_text in reqs {
            let req = VersionReq::parse(req_text).expect("valid req");
            let ranges = req_to_ranges(&req).expect("supported operators");
            for version_text in versions {
                let version = v(version_text);
                assert_eq!(
                    version_matches(&ranges, &version),
                    req.matches(&version),
                    "req `{req_text}` vs version `{version_text}`"
                );
            }
        }
    }

    #[test]
    fn bare_requirement_defaults_to_caret() {
        // `1.14` in a manifest means `^1.14`, cargo-style.
        let r = ranges("1.14");
        assert!(version_matches(&r, &v("1.14.0")));
        assert!(version_matches(&r, &v("1.99.0")));
        assert!(!version_matches(&r, &v("2.0.0")));
        assert!(!version_matches(&r, &v("1.13.9")));
    }

    #[test]
    fn caret_zero_majors_narrow() {
        let r = ranges("^0.2.3");
        assert!(version_matches(&r, &v("0.2.9")));
        assert!(!version_matches(&r, &v("0.3.0")));
        let r = ranges("^0.0.3");
        assert!(version_matches(&r, &v("0.0.3")));
        assert!(!version_matches(&r, &v("0.0.4")));
    }

    #[test]
    fn prerelease_needs_explicit_opt_in() {
        // `^1.0` spans 1.5.0-alpha mathematically, but cargo never selects
        // interior pre-releases.
        let r = ranges("^1.0");
        assert!(!version_matches(&r, &v("1.5.0-alpha")));
        assert!(!version_matches(&r, &v("2.0.0-alpha")));
        // Explicit pre-release requirements admit pre-releases of that triple.
        let r = ranges(">=1.1.0-alpha");
        assert!(version_matches(&r, &v("1.1.0-alpha.2")));
        assert!(version_matches(&r, &v("1.2.0")));
        assert!(!version_matches(&r, &v("1.2.0-rc.1")));
    }

    #[test]
    fn conjunction_intersects() {
        let r = ranges(">=1.2.3, <1.8");
        assert!(version_matches(&r, &v("1.2.3")));
        assert!(version_matches(&r, &v("1.7.9")));
        assert!(!version_matches(&r, &v("1.8.0")));
        assert!(!version_matches(&r, &v("1.2.2")));
    }

    #[test]
    fn u64_max_components_error_instead_of_overflowing() {
        // `u64::MAX` (2^64 - 1) as a version component makes the `+1` upper
        // bound overflow. An adversarial manifest/registry could ship these;
        // they must surface as an error, never a debug panic or a wrapped
        // (nonsensical) range. One case per `+1`-bearing operator family:
        // bare (caret default), caret, tilde, and the comparison forms.
        // The overflow triggers only when the component that gets `+1`'d is
        // itself the ceiling, so `max` sits in the bumped position each time.
        let max = u64::MAX; // 18446744073709551615
        let overflowing = [
            format!("{max}"),         // bare -> ^ default, major+1
            format!("^{max}.1.0"),    // caret, major+1
            format!("^0.{max}.0"),    // caret zero-major, minor+1
            format!("^0.0.{max}"),    // caret zero-minor, patch+1
            format!("~{max}"),        // tilde, major+1
            format!("~{max}.{max}"),  // tilde, minor+1
            format!("={max}"),        // exact major -> major+1 upper
            format!("={max}.{max}"),  // exact major.minor -> minor+1 upper
            format!("{max}.*"),       // wildcard major -> major+1 upper
            format!("{max}.{max}.*"), // wildcard major.minor -> minor+1 upper
            format!(">{max}"),        // greater major -> major+1 lower
            format!(">{max}.{max}"),  // greater major.minor -> minor+1 lower
            format!("<={max}"),       // less-eq major -> major+1 upper
            format!("<={max}.{max}"), // less-eq major.minor -> minor+1 upper
        ];
        for text in overflowing {
            let req = VersionReq::parse(&text).expect("valid req");
            let err = req_to_ranges(&req).expect_err(&format!("`{text}` must error, not overflow"));
            assert!(
                err.contains("too large"),
                "`{text}` should report an oversized component, got `{err}`"
            );
        }

        // One below the ceiling still forms a valid bound (only true overflow
        // is rejected, not merely-large versions).
        let ok = u64::MAX - 1;
        assert!(req_to_ranges(&VersionReq::parse(&format!("^{ok}.0.0")).unwrap()).is_ok());
        // Operator forms that never build a `+1` bound are unaffected even at
        // the ceiling.
        assert!(req_to_ranges(&VersionReq::parse(&format!(">={max}")).unwrap()).is_ok());
        assert!(req_to_ranges(&VersionReq::parse(&format!("<{max}")).unwrap()).is_ok());
        assert!(req_to_ranges(&VersionReq::parse(&format!("={max}.2.3")).unwrap()).is_ok());
    }

    #[test]
    fn display_strips_sentinels_and_names_singletons() {
        assert_eq!(display_ranges(&ranges("^1.2")), ">=1.2.0, <2.0.0");
        assert_eq!(display_ranges(&ranges("=1.2.3")), "1.2.3");
        assert_eq!(display_ranges(&ranges("*")), "*");
        assert_eq!(display_ranges(&ranges(">=2")), ">=2.0.0");
        assert_eq!(display_ranges(&ranges("<1.5.0")), "<1.5.0");
        assert_eq!(
            display_ranges(&ranges(">=1.0.0-alpha, <1.0.0-beta")),
            ">=1.0.0-alpha, <1.0.0-beta"
        );
    }
}
