//! The crate's one "did you mean" nudge.
//!
//! Two callers want it and must not drift apart: `undefined-global` nudges a
//! misspelled global towards a known one (ticket #103's `prnit` → `print`),
//! and [`crate::config`] nudges an unknown `[lint]` key towards a rule id or
//! tier name. `luabox-manifest` has the same helper for its own unknown-key
//! errors, but keeps it private — the two crates are different bounded
//! contexts (SPEC.md §16) and neither owns a shared string-distance utility.

/// The closest candidate within edit distance 1-2, if any.
///
/// Distance 0 (an exact match) is not a suggestion and is skipped, so callers
/// may pass the full candidate set without filtering the name out first. Ties
/// are broken by shortest distance, then alphabetically, so the hint is
/// deterministic across runs and hash orders.
pub(crate) fn closest<'a>(
    name: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let mut best: Option<(usize, &str)> = None;
    for candidate in candidates {
        let dist = levenshtein(name, candidate);
        if dist == 0 || dist > 2 {
            continue;
        }
        let better = match best {
            None => true,
            Some((best_dist, best_name)) => {
                dist < best_dist || (dist == best_dist && candidate < best_name)
            }
        };
        if better {
            best = Some((dist, candidate));
        }
    }
    best.map(|(_, name)| name)
}

/// Plain Levenshtein edit distance (insert/delete/substitute). Identifiers,
/// rule ids, and tier names are ASCII, so byte-wise comparison is exact — no
/// need for Unicode grapheme handling.
fn levenshtein(a: &str, b: &str) -> usize {
    let a = a.as_bytes();
    let b = b.as_bytes();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levenshtein_basics() {
        assert_eq!(levenshtein("print", "print"), 0);
        assert_eq!(levenshtein("prnit", "print"), 2);
        assert_eq!(levenshtein("pedantics", "pedantic"), 1);
        assert_eq!(levenshtein("", "abc"), 3);
    }

    #[test]
    fn closest_picks_the_nearest_candidate_and_skips_exact_matches() {
        let candidates = ["unused-local", "unused-param", "pedantic"];
        assert_eq!(
            closest("unused-locl", candidates.iter().copied()),
            Some("unused-local")
        );
        // An exact match is not a suggestion.
        assert_eq!(closest("pedantic", candidates.iter().copied()), None);
        // Nothing within distance 2.
        assert_eq!(
            closest("totally-unrelated", candidates.iter().copied()),
            None
        );
    }

    #[test]
    fn ties_are_broken_alphabetically_for_determinism() {
        // Both are distance 1 from `unused-loca`.
        let candidates = ["unused-locau", "unused-local"];
        assert_eq!(
            closest("unused-loca", candidates.iter().copied()),
            Some("unused-local")
        );
    }
}
