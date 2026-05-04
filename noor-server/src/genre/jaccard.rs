//! Weighted Jaccard similarity over genre path strings, with a flat
//! shared-ancestor bonus.
//!
//! Used by radio scoring to express "genre coherence with the seed" as a
//! single `[0.0, 1.0]` value. Pure operation: caller fetches the genre
//! path strings (typically from `queries::get_genres_for_tracks`), calls
//! [`weighted_genre_set`] per track, then [`weighted_jaccard`] per pair.
//!
//! ## Formula
//!
//! Each path like `"Electronic > House"` produces a weighted set:
//! - the leaf segment (`"House"`) gets weight `1.0`,
//! - every other segment (`"Electronic"`) gets weight [`ANCESTOR_WEIGHT`].
//! When the same name appears in multiple paths (leaf in one, ancestor
//! in another), the larger weight wins.
//!
//! Jaccard between two weighted sets:
//! ```text
//! intersection_sum = Σ min(seed[k], cand[k]) over shared keys
//! union_sum        = Σ max(seed[k], cand[k]) over all keys
//! base             = intersection_sum / union_sum
//! ```
//!
//! On top of `base`, a flat [`ANCESTOR_BONUS`] is added if any shared
//! key has the ancestor weight on at least one side. This rewards
//! "tracks that pass through the same parent genre" with a recognisable
//! bump even when the leaves differ. Capped at `1.0`.

use std::collections::HashMap;

/// Weight assigned to ancestor segments in a genre path. Leaf segments
/// always get `1.0`. The 0.7 vs 1.0 spread keeps siblings (different
/// leaves under the same parent) clearly distinguishable from same-leaf
/// matches, while still giving meaningful credit to shared parents.
pub const ANCESTOR_WEIGHT: f64 = 0.7;

/// Flat additive bonus applied to the Jaccard result when at least one
/// shared key is an ancestor on at least one side. Capped post-add at
/// `1.0`. Empirically chosen so that "siblings under same parent"
/// produces a `final ≈ 0.36` rather than the bare `0.26` the weighted
/// Jaccard alone gives — siblings deserve recognition.
pub const ANCESTOR_BONUS: f64 = 0.10;

/// Build a weighted genre set from path strings of the shape
/// `"Parent > Leaf"` (or `"Leaf"` for top-level genres).
///
/// Multiple paths are unioned: if the same name appears twice, the
/// larger weight wins (a leaf in one path can't be downgraded to an
/// ancestor by another path). Empty input produces an empty map.
pub fn weighted_genre_set(paths: &[String]) -> HashMap<String, f64> {
    let mut out: HashMap<String, f64> = HashMap::new();
    for path in paths {
        let segments: Vec<&str> = path
            .split(" > ")
            .map(str::trim)
            .filter(|seg| !seg.is_empty())
            .collect();
        if segments.is_empty() {
            continue;
        }
        let last = segments.len() - 1;
        for (i, seg) in segments.iter().enumerate() {
            let weight = if i == last { 1.0 } else { ANCESTOR_WEIGHT };
            let key = seg.to_string();
            out.entry(key)
                .and_modify(|w| {
                    if weight > *w {
                        *w = weight;
                    }
                })
                .or_insert(weight);
        }
    }
    out
}

/// Compute weighted Jaccard with a flat shared-ancestor bonus.
///
/// Returns `1.0` when both sets are empty (a degenerate match — caller
/// should usually filter this case before scoring). Returns `0.0` for
/// genuinely disjoint sets.
pub fn weighted_jaccard(seed: &HashMap<String, f64>, cand: &HashMap<String, f64>) -> f64 {
    if seed.is_empty() && cand.is_empty() {
        return 1.0;
    }

    let mut intersection_sum = 0.0;
    let mut shared_ancestor_present = false;

    for (key, seed_w) in seed {
        if let Some(cand_w) = cand.get(key) {
            intersection_sum += seed_w.min(*cand_w);
            // Shared key qualifies as an ancestor case if it carries the
            // ancestor weight on at least one side (could be ancestor in
            // seed, in cand, or both).
            if (*seed_w - ANCESTOR_WEIGHT).abs() < 1e-9 || (*cand_w - ANCESTOR_WEIGHT).abs() < 1e-9
            {
                shared_ancestor_present = true;
            }
        }
    }

    let mut union_sum = 0.0;
    for (key, seed_w) in seed {
        let cand_w = cand.get(key).copied().unwrap_or(0.0);
        union_sum += seed_w.max(cand_w);
    }
    for (key, cand_w) in cand {
        if !seed.contains_key(key) {
            union_sum += cand_w;
        }
    }

    if union_sum <= 0.0 {
        return 0.0;
    }

    let base = intersection_sum / union_sum;
    let bonus = if shared_ancestor_present {
        ANCESTOR_BONUS
    } else {
        0.0
    };
    (base + bonus).min(1.0).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    fn approx(a: f64, b: f64, tolerance: f64) -> bool {
        (a - b).abs() < tolerance
    }

    // ── weighted_genre_set ───────────────────────────────────────────────────

    #[test]
    fn weighted_set_marks_leaf_at_full_weight() {
        let set = weighted_genre_set(&paths(&["Electronic > House"]));
        assert!(approx(set["House"], 1.0, 1e-9));
    }

    #[test]
    fn weighted_set_marks_ancestor_at_reduced_weight() {
        let set = weighted_genre_set(&paths(&["Electronic > House"]));
        assert!(approx(set["Electronic"], ANCESTOR_WEIGHT, 1e-9));
    }

    #[test]
    fn weighted_set_handles_top_level_genre_with_no_parent() {
        let set = weighted_genre_set(&paths(&["Pop"]));
        assert!(approx(set["Pop"], 1.0, 1e-9));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn weighted_set_promotes_to_leaf_weight_when_same_name_appears_in_two_paths() {
        // "Electronic" appears as leaf in one path and ancestor in
        // another. Leaf weight wins.
        let set = weighted_genre_set(&paths(&["Electronic", "Electronic > House"]));
        assert!(approx(set["Electronic"], 1.0, 1e-9));
        assert!(approx(set["House"], 1.0, 1e-9));
    }

    #[test]
    fn weighted_set_skips_empty_segments() {
        let set = weighted_genre_set(&paths(&["", "  ", "Electronic > House"]));
        assert_eq!(set.len(), 2);
    }

    // ── weighted_jaccard: the canonical fixtures from Phase 2b plan ────────

    #[test]
    fn jaccard_same_leaf_matches_at_one() {
        // Same leaf both sides → Jaccard 1.0; ancestor bonus pushes
        // past the cap, clamped back to 1.0.
        let seed = weighted_genre_set(&paths(&["Electronic > House"]));
        let cand = weighted_genre_set(&paths(&["Electronic > House"]));
        assert!(approx(weighted_jaccard(&seed, &cand), 1.000, 1e-6));
    }

    #[test]
    fn jaccard_siblings_under_same_parent_lands_in_zero_two_to_zero_five() {
        // Different leaves, same parent. Base = 0.7/2.7 ≈ 0.259,
        // shared ancestor bonus +0.10 → 0.359. Should fall in the
        // user-locked 0.2–0.5 band.
        let seed = weighted_genre_set(&paths(&["Electronic > House"]));
        let cand = weighted_genre_set(&paths(&["Electronic > Techno"]));
        let result = weighted_jaccard(&seed, &cand);
        assert!(
            result > 0.2 && result < 0.5,
            "expected 0.2..0.5, got {result}"
        );
        assert!(approx(result, 0.359, 1e-3));
    }

    #[test]
    fn jaccard_disjoint_trees_match_at_zero() {
        let seed = weighted_genre_set(&paths(&["Electronic > House"]));
        let cand = weighted_genre_set(&paths(&["Jazz > Bebop"]));
        assert!(approx(weighted_jaccard(&seed, &cand), 0.000, 1e-9));
    }

    #[test]
    fn jaccard_multi_genre_high_overlap_above_zero_five() {
        // Seed has [Electronic>House, Pop], cand has [Electronic>House].
        // Base = 1.7/2.7 ≈ 0.630, +0.10 → 0.730. Substantial overlap
        // case must land well above 0.5.
        let seed = weighted_genre_set(&paths(&["Electronic > House", "Pop"]));
        let cand = weighted_genre_set(&paths(&["Electronic > House"]));
        let result = weighted_jaccard(&seed, &cand);
        assert!(result > 0.5, "expected > 0.5, got {result}");
        assert!(approx(result, 0.730, 1e-3));
    }

    #[test]
    fn jaccard_multi_genre_low_overlap_only_one_leaf_match() {
        // Seed has [Electronic>House, Pop], cand has [Pop]. Shared key
        // is Pop (leaf-leaf, no ancestor bonus). Base = 1.0/2.7 ≈ 0.370.
        let seed = weighted_genre_set(&paths(&["Electronic > House", "Pop"]));
        let cand = weighted_genre_set(&paths(&["Pop"]));
        let result = weighted_jaccard(&seed, &cand);
        assert!(approx(result, 0.370, 1e-3), "got {result}");
    }

    // ── Additional sanity / monotonicity ────────────────────────────────────

    #[test]
    fn jaccard_top_level_same_genre_matches_at_one() {
        let seed = weighted_genre_set(&paths(&["Pop"]));
        let cand = weighted_genre_set(&paths(&["Pop"]));
        assert!(approx(weighted_jaccard(&seed, &cand), 1.0, 1e-9));
    }

    #[test]
    fn jaccard_top_level_disjoint_genres_match_at_zero() {
        let seed = weighted_genre_set(&paths(&["Pop"]));
        let cand = weighted_genre_set(&paths(&["Rock"]));
        assert!(approx(weighted_jaccard(&seed, &cand), 0.0, 1e-9));
    }

    #[test]
    fn jaccard_asymmetric_depth_seed_is_top_level_cand_under_it() {
        // Seed = {Electronic: 1.0} (Electronic as leaf).
        // Cand = {Electronic: 0.7, House: 1.0}.
        // Shared key Electronic has 0.7 on cand side → ancestor bonus.
        // Intersection = min(1.0, 0.7) = 0.7. Union = 1.0 + 1.0 = 2.0.
        // Base = 0.35, +0.10 → 0.45.
        let seed = weighted_genre_set(&paths(&["Electronic"]));
        let cand = weighted_genre_set(&paths(&["Electronic > House"]));
        let result = weighted_jaccard(&seed, &cand);
        assert!(approx(result, 0.450, 1e-3), "got {result}");
    }

    #[test]
    fn jaccard_is_monotonic_across_overlap_gradient() {
        // Sort five intuitive cases by expected similarity, assert the
        // result order matches.
        let same_leaf = weighted_jaccard(
            &weighted_genre_set(&paths(&["Electronic > House"])),
            &weighted_genre_set(&paths(&["Electronic > House"])),
        );
        let multi_high = weighted_jaccard(
            &weighted_genre_set(&paths(&["Electronic > House", "Pop"])),
            &weighted_genre_set(&paths(&["Electronic > House"])),
        );
        let asymmetric = weighted_jaccard(
            &weighted_genre_set(&paths(&["Electronic"])),
            &weighted_genre_set(&paths(&["Electronic > House"])),
        );
        let multi_low = weighted_jaccard(
            &weighted_genre_set(&paths(&["Electronic > House", "Pop"])),
            &weighted_genre_set(&paths(&["Pop"])),
        );
        let siblings = weighted_jaccard(
            &weighted_genre_set(&paths(&["Electronic > House"])),
            &weighted_genre_set(&paths(&["Electronic > Techno"])),
        );
        let disjoint = weighted_jaccard(
            &weighted_genre_set(&paths(&["Electronic > House"])),
            &weighted_genre_set(&paths(&["Jazz > Bebop"])),
        );

        assert!(same_leaf >= multi_high, "{same_leaf} vs {multi_high}");
        assert!(multi_high > asymmetric, "{multi_high} vs {asymmetric}");
        assert!(asymmetric > multi_low, "{asymmetric} vs {multi_low}");
        assert!(multi_low > siblings, "{multi_low} vs {siblings}");
        assert!(siblings > disjoint, "{siblings} vs {disjoint}");
    }

    #[test]
    fn jaccard_clamps_to_unit_interval_under_bonus_overshoot() {
        // Same leaf → base 1.0, bonus 0.10, capped at 1.0.
        let seed = weighted_genre_set(&paths(&["Electronic > House"]));
        let cand = weighted_genre_set(&paths(&["Electronic > House"]));
        let result = weighted_jaccard(&seed, &cand);
        assert!(result <= 1.0);
        assert!(result >= 0.0);
    }

    #[test]
    fn jaccard_empty_seed_or_cand_does_not_panic() {
        let empty: HashMap<String, f64> = HashMap::new();
        let some = weighted_genre_set(&paths(&["Electronic > House"]));
        // Empty vs non-empty: union has mass, intersection is zero,
        // jaccard is 0. Ancestor bonus does not apply (no shared key).
        assert!(approx(weighted_jaccard(&empty, &some), 0.0, 1e-9));
        assert!(approx(weighted_jaccard(&some, &empty), 0.0, 1e-9));
        // Both empty: caller-friendly degenerate match returns 1.0.
        assert!(approx(weighted_jaccard(&empty, &empty), 1.0, 1e-9));
    }
}
