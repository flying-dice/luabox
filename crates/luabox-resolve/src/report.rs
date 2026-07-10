//! Cargo-quality conflict reports (SPEC.md §6) over PubGrub's derivation
//! tree.
//!
//! PubGrub explains a failed resolution as a tree of incompatibilities;
//! [`ResolveReportFormatter`] renders it as "because X depends on Y ^1 and
//! Z requires Y ^2, …" prose: the root project is named (not called
//! `root`), version sets print in requirement notation with `-0` sentinels
//! stripped ([`display_ranges`]), and lua-versions incompatibilities carry
//! their self-describing reason straight from the solver.

use pubgrub::{Derived, External, Map, Ranges, ReportFormatter, Term};

use crate::semver_ranges::{VersionRanges, display_ranges};
use crate::solver::PkgKey;

/// Formatter plugged into [`pubgrub::DefaultStringReporter`] by the solver.
pub(crate) struct ResolveReportFormatter {
    root_name: String,
}

impl ResolveReportFormatter {
    pub(crate) fn new(root_name: impl Into<String>) -> Self {
        Self {
            root_name: root_name.into(),
        }
    }

    fn pkg(&self, package: &PkgKey) -> String {
        match package {
            PkgKey::Root => self.root_name.clone(),
            PkgKey::Pkg(id) => id.to_string(),
        }
    }

    /// `name`, `name 1.2.3`, or `name >=1.0.0, <2.0.0`. The root project
    /// always reads as its bare name — its version is never in question.
    fn pkg_range(&self, package: &PkgKey, ranges: &VersionRanges) -> String {
        if matches!(package, PkgKey::Root) || ranges == &Ranges::full() {
            self.pkg(package)
        } else {
            format!("{} {}", self.pkg(package), display_ranges(ranges))
        }
    }

    fn dependency_line(
        &self,
        package: &PkgKey,
        package_set: &VersionRanges,
        dependency: &PkgKey,
        dependency_set: &VersionRanges,
    ) -> String {
        format!(
            "{} depends on {}",
            self.pkg_range(package, package_set),
            self.pkg_range(dependency, dependency_set)
        )
    }
}

type M = String;

impl ReportFormatter<PkgKey, VersionRanges, M> for ResolveReportFormatter {
    type Output = String;

    fn format_external(&self, external: &External<PkgKey, VersionRanges, M>) -> String {
        match external {
            External::NotRoot(..) => {
                format!("we are solving dependencies of {}", self.root_name)
            }
            External::NoVersions(package, set) => {
                if set == &Ranges::full() {
                    format!("there are no versions of {}", self.pkg(package))
                } else {
                    format!(
                        "no version of {} matches {}",
                        self.pkg(package),
                        display_ranges(set)
                    )
                }
            }
            // Custom incompatibilities (e.g. lua-versions) carry a
            // self-describing sentence built by the solver.
            External::Custom(_, _, reason) => reason.clone(),
            External::FromDependencyOf(package, package_set, dependency, dependency_set) => {
                self.dependency_line(package, package_set, dependency, dependency_set)
            }
        }
    }

    fn format_terms(&self, terms: &Map<PkgKey, Term<VersionRanges>>) -> String {
        let terms_vec: Vec<_> = terms.iter().collect();
        match terms_vec.as_slice() {
            [] | [(PkgKey::Root, _)] => "version solving failed".into(),
            [(package, Term::Positive(range))] => {
                format!("{} cannot be used", self.pkg_range(package, range))
            }
            [(package, Term::Negative(range))] => {
                format!("{} is required", self.pkg_range(package, range))
            }
            [(p1, Term::Positive(r1)), (p2, Term::Negative(r2))] => {
                self.dependency_line(p1, r1, p2, r2)
            }
            [(p1, Term::Negative(r1)), (p2, Term::Positive(r2))] => {
                self.dependency_line(p2, r2, p1, r1)
            }
            slice => {
                let mut parts: Vec<String> = Vec::with_capacity(slice.len());
                for (package, term) in slice {
                    match term {
                        Term::Positive(range) => parts.push(self.pkg_range(package, range)),
                        Term::Negative(range) => {
                            parts.push(format!("not {}", self.pkg_range(package, range)));
                        }
                    }
                }
                format!("{} are incompatible", parts.join(" and "))
            }
        }
    }

    fn explain_both_external(
        &self,
        external1: &External<PkgKey, VersionRanges, M>,
        external2: &External<PkgKey, VersionRanges, M>,
        current_terms: &Map<PkgKey, Term<VersionRanges>>,
    ) -> String {
        format!(
            "Because {} and {}, {}.",
            self.format_external(external1),
            self.format_external(external2),
            self.format_terms(current_terms)
        )
    }

    fn explain_both_ref(
        &self,
        ref_id1: usize,
        derived1: &Derived<PkgKey, VersionRanges, M>,
        ref_id2: usize,
        derived2: &Derived<PkgKey, VersionRanges, M>,
        current_terms: &Map<PkgKey, Term<VersionRanges>>,
    ) -> String {
        format!(
            "Because {} ({ref_id1}) and {} ({ref_id2}), {}.",
            self.format_terms(&derived1.terms),
            self.format_terms(&derived2.terms),
            self.format_terms(current_terms)
        )
    }

    fn explain_ref_and_external(
        &self,
        ref_id: usize,
        derived: &Derived<PkgKey, VersionRanges, M>,
        external: &External<PkgKey, VersionRanges, M>,
        current_terms: &Map<PkgKey, Term<VersionRanges>>,
    ) -> String {
        format!(
            "Because {} ({ref_id}) and {}, {}.",
            self.format_terms(&derived.terms),
            self.format_external(external),
            self.format_terms(current_terms)
        )
    }

    fn and_explain_external(
        &self,
        external: &External<PkgKey, VersionRanges, M>,
        current_terms: &Map<PkgKey, Term<VersionRanges>>,
    ) -> String {
        format!(
            "And because {}, {}.",
            self.format_external(external),
            self.format_terms(current_terms)
        )
    }

    fn and_explain_ref(
        &self,
        ref_id: usize,
        derived: &Derived<PkgKey, VersionRanges, M>,
        current_terms: &Map<PkgKey, Term<VersionRanges>>,
    ) -> String {
        format!(
            "And because {} ({ref_id}), {}.",
            self.format_terms(&derived.terms),
            self.format_terms(current_terms)
        )
    }

    fn and_explain_prior_and_external(
        &self,
        prior_external: &External<PkgKey, VersionRanges, M>,
        external: &External<PkgKey, VersionRanges, M>,
        current_terms: &Map<PkgKey, Term<VersionRanges>>,
    ) -> String {
        format!(
            "And because {} and {}, {}.",
            self.format_external(prior_external),
            self.format_external(external),
            self.format_terms(current_terms)
        )
    }
}
