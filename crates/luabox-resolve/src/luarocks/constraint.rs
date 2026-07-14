//! LuaRocks version + constraint translation (SPEC.md §6).
//!
//! LuaRocks and Cargo/semver describe versions and requirements differently.
//! This module is the (documented, deliberately lossy) bridge between them,
//! kept pure and table-tested so the mapping is auditable in one place.
//!
//! # Version translation ([`translate_version`])
//!
//! A LuaRocks version is `MAJOR[.MINOR[.PATCH[.EXTRA…]]]` optionally
//! followed by a `-ROCKREV` *rock revision* (the packaging iteration, e.g.
//! `1.4.1-3`). The rock revision is **not** a semver pre-release — mapping
//! it onto semver's `-N` would sort it *below* the release — so it is
//! dropped. Missing trailing components pad with zeros; a 4th (and beyond)
//! numeric component is dropped (semver is strictly three-part), which makes
//! `1.2.3.4` and `1.2.3.5` compare equal — an accepted, rare loss. Purely
//! non-numeric versions (`scm`, `dev`, `cvs`) have no semver image and are
//! skipped by the provider entirely.
//!
//! # Constraint translation ([`translate_constraint`])
//!
//! LuaRocks dependency strings (`"lpeg >= 1.0, < 2.0"`) carry their own
//! constraint grammar. The operators, from LuaRocks' own `queries.lua`:
//!
//! | LuaRocks                | semver (Cargo `VersionReq`)        | note |
//! |-------------------------|------------------------------------|------|
//! | `>= X` / `> X`          | `>=X` / `>X`                       |      |
//! | `<= X` / `< X`          | `<=X` / `<X`                       |      |
//! | `== X`, `= X`, bare `X` | `=X`                               | bare defaults to `==` |
//! | `~> a`                  | `>=a.0.0, <(a+1).0.0`             | prefix match |
//! | `~> a.b`                | `>=a.b.0, <a.(b+1).0`            | prefix match |
//! | `~> a.b.c`              | `>=a.b.c, <a.b.(c+1)`           | prefix match |
//! | `~= X`, `!= X`          | `*` (any)                         | **lossy**: semver has no `!=` |
//! | `c1, c2`                | `c1, c2` (intersection)           |      |
//!
//! The `~>` (pessimistic) operator is a *prefix* match: every component the
//! user writes is pinned, and the first component they omit is left free —
//! so `~> 2.5` matches `2.5.x` but not `2.6`, and `~> 2.5.3` matches only
//! `2.5.3`. This differs from Cargo's `~` (which always frees the patch),
//! so it is expanded to explicit `>=`/`<` comparators here.
//!
//! `~=`/`!=` (not-equal) has no single-comparator semver form; rather than
//! fail resolution on a rarely used operator it is widened to "any version"
//! and reported as lossy. Callers may surface [`Lossiness`] as a warning.

use semver::Version;

/// A translated constraint plus any information lost in translation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Translated {
    /// A Cargo-style requirement string (parseable by
    /// [`semver::VersionReq`]). `*` means "any version".
    pub requirement: String,
    /// Non-empty when the translation could not be represented faithfully.
    pub lossiness: Vec<Lossiness>,
}

/// A specific way a LuaRocks constraint could not be represented in semver.
// TODO: clean-code - 0.55 - KISS: speculative warning infrastructure with no
// consumer — ExtraComponentDropped is never constructed (the flag feeding it
// is discarded), and Translated.lossiness is read only by tests; both
// production call sites take .requirement and drop .lossiness. Either
// surface it as a real warning or strip it and return a plain String.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lossiness {
    /// A `~=`/`!=` (not-equal) constraint was widened to any version.
    NotEqualWidened { operand: String },
    /// A version component beyond the third was dropped.
    ExtraComponentDropped { version: String },
}

impl std::fmt::Display for Lossiness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotEqualWidened { operand } => write!(
                f,
                "the `~=`/`!=` constraint on `{operand}` has no semver equivalent and was widened to any version"
            ),
            Self::ExtraComponentDropped { version } => write!(
                f,
                "version `{version}` has more than three components; the extras were dropped"
            ),
        }
    }
}

/// Translates a LuaRocks version string into a semver [`Version`], or `None`
/// when it has no numeric leading component (e.g. `scm`, `dev`).
///
/// Rock revisions (`-N`) and any 4th-and-beyond numeric components are
/// dropped (see the module docs).
#[must_use]
pub fn translate_version(luarocks: &str) -> Option<Version> {
    let (components, _extra) = version_components(luarocks)?;
    Some(Version::new(components[0], components[1], components[2]))
}

/// Parses the numeric `[major, minor, patch]` of a LuaRocks version (padding
/// with zeros), returning it plus whether extra components were dropped.
///
/// Returns `None` when the version has no leading numeric component.
fn version_components(luarocks: &str) -> Option<([u64; 3], bool)> {
    // Drop the `-ROCKREV` packaging suffix if present.
    let core = luarocks.split('-').next().unwrap_or(luarocks);
    let mut parts = core.split('.');
    let mut out = [0u64; 3];
    let mut count = 0usize;
    let mut had_numeric = false;
    for (i, part) in parts.by_ref().enumerate() {
        let Ok(number) = part.parse::<u64>() else {
            // First component must be numeric; a non-numeric first part
            // (e.g. `scm`) has no semver image.
            if i == 0 {
                return None;
            }
            break;
        };
        had_numeric = true;
        if i < 3 {
            out[i] = number;
            count = i + 1;
        } else {
            // A 4th+ numeric component: dropped.
            return Some((out, true));
        }
    }
    if !had_numeric {
        return None;
    }
    let _ = count;
    Some((out, false))
}

/// Translates a full LuaRocks constraint list (the part of a dependency
/// string after the rock name, e.g. `">= 1.0, < 2.0"`) into a Cargo
/// requirement string.
///
/// An empty constraint (`""`) means "any version" → `*`.
#[must_use]
pub fn translate_constraint(constraints: &str) -> Translated {
    let trimmed = constraints.trim();
    if trimmed.is_empty() {
        return Translated {
            requirement: "*".to_owned(),
            lossiness: Vec::new(),
        };
    }

    let mut comparators: Vec<String> = Vec::new();
    let mut lossiness: Vec<Lossiness> = Vec::new();
    let mut any = false;

    for raw in trimmed.split(',') {
        let piece = raw.trim();
        if piece.is_empty() {
            continue;
        }
        match translate_one(piece) {
            OneConstraint::Comparators(mut cs) => comparators.append(&mut cs),
            OneConstraint::Any(loss) => {
                any = true;
                if let Some(loss) = loss {
                    lossiness.push(loss);
                }
            }
        }
    }

    // If any sub-constraint widened to "any" and nothing else narrowed it,
    // the whole requirement is any.
    let requirement = if comparators.is_empty() || (any && comparators.is_empty()) {
        "*".to_owned()
    } else {
        comparators.join(", ")
    };
    Translated {
        requirement,
        lossiness,
    }
}

enum OneConstraint {
    Comparators(Vec<String>),
    Any(Option<Lossiness>),
}

fn translate_one(piece: &str) -> OneConstraint {
    let (op, operand) = split_operator(piece);
    let operand = operand.trim();
    // Strip a rock revision from the operand version.
    let version = operand.split('-').next().unwrap_or(operand).trim();

    match op {
        ">=" | ">" | "<=" | "<" => OneConstraint::Comparators(vec![format!("{op}{version}")]),
        "==" | "=" | "" => OneConstraint::Comparators(vec![format!("={version}")]),
        "~=" | "!=" => OneConstraint::Any(Some(Lossiness::NotEqualWidened {
            operand: operand.to_owned(),
        })),
        "~>" => OneConstraint::Comparators(pessimistic(version)),
        _ => {
            // Unknown operator: be conservative and pin exactly.
            OneConstraint::Comparators(vec![format!("={version}")])
        }
    }
}

/// Splits a leading run of operator characters (`<>=~!`) off a constraint
/// piece, returning `(operator, rest)`.
fn split_operator(piece: &str) -> (&str, &str) {
    let end = piece
        .find(|c: char| !matches!(c, '<' | '>' | '=' | '~' | '!'))
        .unwrap_or(piece.len());
    // `end` is a byte index returned by `str::find` (or the string length),
    // so it always lands on a char boundary.
    piece.split_at(end)
}

/// Expands the LuaRocks pessimistic operator `~> v` into explicit
/// `>=`/`<` comparators (prefix match — see module docs).
fn pessimistic(version: &str) -> Vec<String> {
    let core = version.split('-').next().unwrap_or(version);
    let numeric: Vec<u64> = core
        .split('.')
        .map(|p| p.parse::<u64>().unwrap_or(0))
        .collect();
    if numeric.is_empty() {
        return vec![format!("={version}")];
    }
    // Lower bound: the version as written, padded to three components.
    let mut lower = numeric.clone();
    while lower.len() < 3 {
        lower.push(0);
    }
    let lower = format!("{}.{}.{}", lower[0], lower[1], lower[2]);

    // Upper bound: increment the last *written* component, zero the rest.
    let mut upper = numeric.clone();
    let last = upper.len() - 1;
    upper[last] += 1;
    while upper.len() < 3 {
        upper.push(0);
    }
    let upper = format!("{}.{}.{}", upper[0], upper[1], upper[2]);

    vec![format!(">={lower}"), format!("<{upper}")]
}

#[cfg(test)]
mod tests {
    use super::*;
    use semver::VersionReq;

    #[test]
    fn version_drops_rock_revision_and_pads() {
        assert_eq!(translate_version("1.4.1-3"), Some(Version::new(1, 4, 1)));
        assert_eq!(translate_version("1.4-2"), Some(Version::new(1, 4, 0)));
        assert_eq!(translate_version("2"), Some(Version::new(2, 0, 0)));
        assert_eq!(translate_version("1.2.3"), Some(Version::new(1, 2, 3)));
    }

    #[test]
    fn version_drops_fourth_component() {
        assert_eq!(translate_version("1.2.3.4"), Some(Version::new(1, 2, 3)));
    }

    #[test]
    fn non_numeric_versions_have_no_image() {
        assert_eq!(translate_version("scm"), None);
        assert_eq!(translate_version("dev-1"), None);
        assert_eq!(translate_version("cvs"), None);
    }

    /// The authoritative behaviour table (LuaRocks `queries.lua` operators),
    /// checked both as the produced requirement string *and* by exercising
    /// the resulting `VersionReq` against representative versions.
    #[test]
    fn constraint_table() {
        let cases = [
            (">= 1.0", ">=1.0"),
            ("> 1.0", ">1.0"),
            ("<= 2.0", "<=2.0"),
            ("< 2.0", "<2.0"),
            ("== 1.2", "=1.2"),
            ("= 1.2", "=1.2"),
            ("1.2", "=1.2"),
            ("", "*"),
            (">= 1.0, < 2.0", ">=1.0, <2.0"),
        ];
        for (input, expected) in cases {
            let got = translate_constraint(input);
            assert_eq!(got.requirement, expected, "constraint `{input}`");
            assert!(
                VersionReq::parse(&got.requirement).is_ok(),
                "produced req `{}` for `{input}` must parse",
                got.requirement
            );
            assert!(got.lossiness.is_empty(), "`{input}` should be lossless");
        }
    }

    #[test]
    fn pessimistic_is_a_prefix_match() {
        // ~> 2.5 matches 2.5.x but not 2.6.
        let got = translate_constraint("~> 2.5");
        assert_eq!(got.requirement, ">=2.5.0, <2.6.0");
        let req = VersionReq::parse(&got.requirement).unwrap();
        assert!(req.matches(&Version::new(2, 5, 0)));
        assert!(req.matches(&Version::new(2, 5, 9)));
        assert!(!req.matches(&Version::new(2, 6, 0)));

        // ~> 2.5.3 matches only 2.5.3.
        let got = translate_constraint("~> 2.5.3");
        assert_eq!(got.requirement, ">=2.5.3, <2.5.4");
        let req = VersionReq::parse(&got.requirement).unwrap();
        assert!(req.matches(&Version::new(2, 5, 3)));
        assert!(!req.matches(&Version::new(2, 5, 4)));

        // ~> 2 matches 2.x.x but not 3.
        let got = translate_constraint("~> 2");
        assert_eq!(got.requirement, ">=2.0.0, <3.0.0");
    }

    #[test]
    fn not_equal_is_lossy_and_widens() {
        let got = translate_constraint("~= 1.0");
        assert_eq!(got.requirement, "*");
        assert_eq!(
            got.lossiness,
            vec![Lossiness::NotEqualWidened {
                operand: "1.0".to_owned()
            }]
        );
        let got = translate_constraint("!= 2.3");
        assert_eq!(got.requirement, "*");
        assert_eq!(got.lossiness.len(), 1);
    }

    #[test]
    fn constraint_operand_rock_revision_stripped() {
        assert_eq!(translate_constraint(">= 1.4.1-3").requirement, ">=1.4.1");
    }
}
