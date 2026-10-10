//! Candidate scoring: ISRC dedup, per-source normalization, penalties, taste and genre signals, reasons.

use super::*;

/// Group candidates by normalised (artist, title) and pick one winner per
/// group.
///
/// The historical behaviour was "library wins all ties because it iterates
/// first", which silently cannibalised Last.fm's exploration value when the
/// library was dense around a seed. The new rule:
///
/// - If a library candidate is in the group AND its `similarity_score` is
///   within 5% of the best non-library score, library wins. Preserves the
///   "prefer in-library, all else equal" instinct without letting it
///   dominate when an external source is meaningfully more confident.
/// - Otherwise the highest `similarity_score` wins.
/// - Ties (within 1e-9 after the 5% rule) break by source priority
///   Library > Engine > Lastfm so HashMap iteration order doesn't flap
///   between runs.
///
/// Order across groups follows first-seen insertion order across the
/// library/lastfm/engine input slices (in that order). Each input is
/// expected to be roughly score-sorted by its producer, so first-seen
/// approximates a stable, score-ordered output across the deduped set.
pub(super) fn combine_with_dedup(
    library: Vec<RadioCandidate>,
    lastfm: Vec<RadioCandidate>,
    engine: Vec<RadioCandidate>,
) -> Vec<RadioCandidate> {
    let mut groups: HashMap<String, Vec<RadioCandidate>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    for source_list in [library, lastfm, engine] {
        for cand in source_list {
            let norm = normalize_for_dedup(&cand.artist_name, &cand.title);
            if norm.is_empty() {
                continue;
            }
            if !groups.contains_key(&norm) {
                order.push(norm.clone());
            }
            groups.entry(norm).or_default().push(cand);
        }
    }

    let mut out = Vec::with_capacity(order.len());
    for norm in order {
        if let Some(winner) = pick_dedup_winner(groups.remove(&norm).unwrap_or_default()) {
            out.push(winner);
        }
    }
    out
}

// Per-source hybrid normalization. The two halves rein in different failure
// modes: percentile-clipping (p10-p90) drops outliers without compressing the
// bulk; rank-norm guarantees a fair mapping when the score distribution is
// degenerate (all scores nearly equal, e.g. a Last.fm result with everything
// at match=1.0). Half-and-half so neither dominates.
//
// Skipped when N < 5 because both halves get noisy: rank-norm of 4 elements
// produces only 4 distinct values, and p10/p90 quantiles aren't meaningful.
// In that regime, the legacy raw-score behavior is preferable.
pub(super) fn normalize_source_scores(candidates: &mut [RadioCandidate]) {
    use std::collections::HashMap;
    // Bucket candidates by source. We'll compute the normalization parameters
    // per source then walk the candidates once more applying them.
    let mut by_source: HashMap<RadioSource, Vec<usize>> = HashMap::new();
    for (idx, c) in candidates.iter().enumerate() {
        by_source.entry(c.source).or_default().push(idx);
    }

    for indices in by_source.values() {
        let n = indices.len();
        if n < 5 {
            continue;
        }
        if n == 1 {
            candidates[indices[0]].similarity_score = 1.0;
            continue;
        }
        // Sort indices descending by raw score. Position-in-sorted-order maps
        // to rank: best-ranked = 0 → rank_norm 1.0, worst-ranked = N-1 → 0.0.
        let mut sorted = indices.clone();
        sorted.sort_by(|&a, &b| {
            candidates[b]
                .similarity_score
                .partial_cmp(&candidates[a].similarity_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // p10 and p90 from the same set of raw scores (in any order).
        let mut raw_scores: Vec<f64> = indices
            .iter()
            .map(|&i| candidates[i].similarity_score)
            .collect();
        raw_scores.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p10 = percentile(&raw_scores, 0.10);
        let p90 = percentile(&raw_scores, 0.90);
        let pct_range = p90 - p10;
        let degenerate = pct_range < 1e-6;

        let n_minus_1 = (n as f64) - 1.0;
        for (sorted_idx, &cand_idx) in sorted.iter().enumerate() {
            let rank_norm = 1.0 - (sorted_idx as f64) / n_minus_1;
            let raw = candidates[cand_idx].similarity_score;
            let pct_clipped = if degenerate {
                0.5
            } else {
                ((raw - p10) / pct_range).clamp(0.0, 1.0)
            };
            candidates[cand_idx].similarity_score = 0.5 * pct_clipped + 0.5 * rank_norm;
        }
    }
}

// Soft confidence penalty: library candidates with confidence below the
// threshold get a 0.75 score multiplier. Soft, not a hard drop, because the
// confidence formula is heuristic and we don't want to silently lose cold-
// start tracks (which floor at 0.25). Lastfm + engine candidates pass through
// unchanged — they have no confidence value, and a None should not behave the
// same as "0.0 confidence".
pub(super) fn apply_confidence_penalty(candidates: &mut [RadioCandidate], min_confidence: f64) {
    const PENALTY_MULTIPLIER: f64 = 0.75;
    for cand in candidates.iter_mut() {
        if let Some(conf) = cand.confidence
            && conf < min_confidence
        {
            cand.similarity_score *= PENALTY_MULTIPLIER;
        }
    }
}

// Hub penalty: tracks that appear as a neighbor for many seeds (high in-degree
// percentile) get downweighted. Library candidates only — Lastfm + engine have
// no in-degree data. Returns the cumulative penalty magnitude (sum of (1 -
// multiplier) over all penalized candidates) for diagnostics.
//
// The 1/(1 + k*pct) shape gives a smooth slope: pct=0 → multiplier 1.0 (no
// penalty), pct=1 → 1/(1+k). With hub_penalty=0.35 (Mixed default), top-hub
// gets 0.74×, mid-hub (pct=0.5) gets 0.85×.
pub(super) fn apply_hub_penalty(candidates: &mut [RadioCandidate], hub_penalty: f64) -> f64 {
    if hub_penalty <= 0.0 {
        return 0.0;
    }
    let mut total = 0.0;
    for cand in candidates.iter_mut() {
        if let Some(pct) = cand.candidate_in_degree_percentile {
            let multiplier = 1.0 / (1.0 + hub_penalty * pct);
            total += 1.0 - multiplier;
            cand.similarity_score *= multiplier;
        }
    }
    total
}

// Linear-interpolated percentile of an already-ascending-sorted slice. q is in
// [0, 1]. Avoids pulling in a full statistics dep just for two values.
pub(super) fn percentile(sorted_asc: &[f64], q: f64) -> f64 {
    if sorted_asc.is_empty() {
        return 0.0;
    }
    if sorted_asc.len() == 1 {
        return sorted_asc[0];
    }
    let q = q.clamp(0.0, 1.0);
    let pos = q * (sorted_asc.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    if lo == hi {
        sorted_asc[lo]
    } else {
        let frac = pos - lo as f64;
        sorted_asc[lo] * (1.0 - frac) + sorted_asc[hi] * frac
    }
}

/// Returns the higher-priority numeric for tie-breaking. Order is
/// Library, then Engine, then Lastfm — matching the implicit preference
/// from the legacy behaviour (library first), narrowed to only fire on
/// score ties.
pub(super) fn source_priority(source: RadioSource) -> u8 {
    match source {
        RadioSource::Library => 3,
        RadioSource::Engine => 2,
        RadioSource::Lastfm | RadioSource::Tidal => 1,
    }
}

pub(super) const LIBRARY_TIE_BREAK_THRESHOLD: f64 = 0.95;

pub(super) fn pick_dedup_winner(group: Vec<RadioCandidate>) -> Option<RadioCandidate> {
    if group.is_empty() {
        return None;
    }

    let best_library_score = group
        .iter()
        .filter(|c| c.source == RadioSource::Library)
        .map(|c| c.similarity_score)
        .fold(f64::NEG_INFINITY, f64::max);
    let best_other_score = group
        .iter()
        .filter(|c| c.source != RadioSource::Library)
        .map(|c| c.similarity_score)
        .fold(f64::NEG_INFINITY, f64::max);

    let library_present = best_library_score.is_finite();
    let other_threshold = if best_other_score.is_finite() {
        best_other_score * LIBRARY_TIE_BREAK_THRESHOLD
    } else {
        f64::NEG_INFINITY
    };

    if library_present && best_library_score >= other_threshold {
        // Library wins: pick the highest-scoring library candidate.
        return group
            .into_iter()
            .filter(|c| c.source == RadioSource::Library)
            .max_by(|a, b| {
                a.similarity_score
                    .partial_cmp(&b.similarity_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
    }

    // Highest-score wins. Tie-break by source priority so HashMap
    // iteration order doesn't make the output flap.
    group.into_iter().max_by(|a, b| {
        a.similarity_score
            .partial_cmp(&b.similarity_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| source_priority(a.source).cmp(&source_priority(b.source)))
    })
}

/// Saturation constant for affinity compression. At `x = K` the
/// compressed value is 0.5; at `x = 4K` it's 0.8; asymptote is 1.0.
/// `pos` and `neg` are unbounded recency-weighted accumulators that
/// easily reach 20–50 for any artist with recent listen history, so the
/// raw values cannot drive the multiplier directly without saturating.
pub(super) const AFFINITY_SATURATION: f64 = 10.0;

/// Bounded effect a saturated positive affinity adds to the multiplier.
/// At `pos = ∞` the multiplier is `1.0 + SCALE_POS = 1.20`.
pub(super) const AFFINITY_SCALE_POS: f64 = 0.20;

/// Bounded effect a saturated negative affinity subtracts. At
/// `neg = ∞` the multiplier is `1.0 - SCALE_NEG = 0.70`. Asymmetric vs
/// `SCALE_POS` to mirror automix's "negatives hurt more than positives
/// help" weighting (0.5 / 0.65 there, 0.20 / 0.30 here).
pub(super) const AFFINITY_SCALE_NEG: f64 = 0.30;

/// Floor on the multiplier so even heavily-skipped artists still appear
/// in the queue at low rank rather than being eliminated entirely. The
/// formula above never produces a value below `1.0 − SCALE_NEG = 0.70`,
/// so this is a defensive backstop, not the load-bearing clamp the
/// previous implementation relied on.
pub(super) const AFFINITY_FLOOR: f64 = 0.1;

/// Apply per-user taste signals to a deduped candidate list. Drops tracks
/// the user just skipped; nudges `similarity_score` up for liked artists
/// and down for skipped ones.
///
/// Compression first: the raw `pos` and `neg` accumulators get
/// `x / (x + K)` so 20 vs 50 vs 200 all map to roughly the same
/// region of `[0, 1]`. Then asymmetric scaling: positives are worth at
/// most +20% of the score, negatives at most −30%, mirroring automix's
/// pos:neg ratio at a magnitude that suits radio's bounded
/// `similarity_score`.
///
/// Resolver misses (last.fm hit naming an artist not in the library) leave
/// `similarity_score` unchanged. That is the documented Phase 2a
/// behaviour: unknown artists carry no affinity, full stop.
pub(super) fn apply_taste_signals(
    candidates: &mut Vec<RadioCandidate>,
    taste: &TasteVector,
    resolver: &ArtistResolver,
) {
    // Hard suppression: drop tracks the user just skipped. Only fires
    // for candidates with a real library track_id; last.fm hits have
    // track_id = 0 and skip the check.
    candidates
        .retain(|cand| cand.track_id == 0 || !taste.skipped_track_ids.contains(&cand.track_id));

    for cand in candidates.iter_mut() {
        if let Some(artist_id) = resolver.lookup(&cand.artist_name)
            && let Some(affinity) = taste.artist_affinity.get(&artist_id)
        {
            let pos_c = affinity.pos / (affinity.pos + AFFINITY_SATURATION);
            let neg_c = affinity.neg / (affinity.neg + AFFINITY_SATURATION);
            let multiplier = 1.0 + (pos_c * AFFINITY_SCALE_POS) - (neg_c * AFFINITY_SCALE_NEG);
            cand.similarity_score *= multiplier.max(AFFINITY_FLOOR);
        }
    }
}

/// Compute a weighted Jaccard genre similarity between the seed and
/// every candidate that has a real library `track_id`.
///
/// Returns a `HashMap` keyed by `(source, track_id, normalised dedup
/// key)` — the same shape `pre_affinity_scores` uses, deliberately
/// stable across `apply_taste_signals`'s `candidates.retain(...)`
/// drops. Lastfm candidates (track_id == 0) and library candidates
/// with no genre rows are absent from the map; callers should treat
/// absence as "no signal" rather than "zero similarity".
///
/// On any DB error or missing seed genres, returns an empty map and
/// logs. Genre signal failure must not take the radio request offline.
pub(super) fn compute_genre_jaccard(
    db: &Database,
    seed_track_id: i64,
    candidates: &[RadioCandidate],
) -> HashMap<(RadioSource, i64, String), f64> {
    use crate::genre::jaccard::{weighted_genre_set, weighted_jaccard};

    let cand_ids: Vec<i64> = candidates
        .iter()
        .map(|c| c.track_id)
        .filter(|id| *id > 0)
        .collect();
    if cand_ids.is_empty() {
        return HashMap::new();
    }

    let mut all_ids = cand_ids.clone();
    all_ids.push(seed_track_id);
    all_ids.sort_unstable();
    all_ids.dedup();

    let resolved_by_track = match db.with_conn(move |conn| {
        crate::db::queries::get_genres_for_tracks_with_fallback(conn, &all_ids)
    }) {
        Ok(m) => m,
        Err(err) => {
            tracing::warn!(
                seed_track_id,
                "radio: genre enrichment query failed ({err:#}); skipping genre signal"
            );
            return HashMap::new();
        }
    };

    let seed_set = match resolved_by_track.get(&seed_track_id) {
        Some(rows) => {
            let paths = crate::db::queries::ResolvedGenre::paths_only(rows);
            weighted_genre_set(&paths)
        }
        None => {
            // Truly empty even after album/artist fallback (no siblings tagged
            // anywhere). Older logging said "skipped for all candidates" — the
            // failure mode is identical, just rarer now.
            tracing::debug!(
                seed_track_id,
                "radio: seed has no genre rows even after fallback; genre Jaccard skipped for all candidates"
            );
            return HashMap::new();
        }
    };

    let mut out = HashMap::new();
    for cand in candidates.iter() {
        if cand.track_id <= 0 {
            continue;
        }
        let Some(rows) = resolved_by_track.get(&cand.track_id) else {
            continue;
        };
        let paths = crate::db::queries::ResolvedGenre::paths_only(rows);
        let cand_set = weighted_genre_set(&paths);
        let score = weighted_jaccard(&seed_set, &cand_set);
        let key = (
            cand.source,
            cand.track_id,
            normalize_for_dedup(&cand.artist_name, &cand.title),
        );
        out.insert(key, score);
    }
    out
}

/// Phase 2b Stage 2: genre coherence multiplier and mode-based hard
/// reject.
///
/// For each candidate with a Jaccard value (library/engine candidates;
/// lastfm hits are absent from the map and pass through untouched):
///
/// - **Hard reject** if `jaccard < threshold[blend]`. Familiar drops
///   below 0.10, Mixed drops below 0.05, Adventurous never drops.
///   This filters out candidates that share no genre relationship with
///   the seed at all — the kind of false-positive Phase 2b targets.
///
/// - **Multiplier** `1.0 + (jaccard * 0.30) - ((1.0 - jaccard) * 0.20
///   when jaccard < 0.5 else 0)`. Substantial overlap (jaccard >= 0.5)
///   only ever helps; partial overlap can demote. Floored at 0.1
///   defensively (the formula itself never goes below 0.80).
///
/// Lastfm candidates pass through with no adjustment by design — the
/// system has no genre data for tracks not in the library, and a
/// library-artist proxy was rejected as too lossy. They compete on
/// source-native similarity score and artist-affinity multiplier
/// only.
pub(super) fn apply_genre_signals(
    candidates: &mut Vec<RadioCandidate>,
    jaccard_by_key: &HashMap<(RadioSource, i64, String), f64>,
    blend: RadioBlend,
) {
    let hard_reject_threshold = match blend {
        RadioBlend::Familiar => Some(0.10),
        RadioBlend::Mixed => Some(0.05),
        RadioBlend::Adventurous => None,
    };

    candidates.retain_mut(|cand| {
        let key = (
            cand.source,
            cand.track_id,
            normalize_for_dedup(&cand.artist_name, &cand.title),
        );
        let Some(jaccard) = jaccard_by_key.get(&key).copied() else {
            // No genre data — pass through unchanged.
            return true;
        };

        // Mode-based hard reject.
        if let Some(threshold) = hard_reject_threshold
            && jaccard < threshold
        {
            return false;
        }

        // Multiplier per the locked Phase 2b formula.
        let bonus = jaccard * 0.30;
        let penalty = if jaccard < 0.5 {
            (1.0 - jaccard) * 0.20
        } else {
            0.0
        };
        let multiplier = (1.0 + bonus - penalty).max(0.1);
        cand.similarity_score *= multiplier;
        true
    });
}

/// Append a structured JSON suffix to each candidate's `reason` string,
/// carrying the genre Jaccard, the affinity multiplier, and (in Stage
/// 2) the genre multiplier for the frontend tooltip to display.
///
/// Format: `"<existing prefix> | <json>"`. The frontend parser splits
/// on the rightmost ` | ` and tries `JSON.parse` on the right half;
/// candidates without the suffix keep working as plain strings.
///
/// `pre_affinity` is the snapshot taken *before* both `apply_taste_signals`
/// and `apply_genre_signals`; `post_affinity` is the snapshot taken
/// between the two. The current `cand.similarity_score` is the
/// post-genre value. From these three points we extract:
///
/// - `affinity_mult = post_affinity / pre_affinity`
/// - `genre_mult    = post_genre    / post_affinity`
///
/// Best-effort: serialisation failure is silently swallowed so a
/// reason-formatting bug never fails a radio request.
pub(super) fn annotate_reasons(
    candidates: &mut [RadioCandidate],
    pre_affinity: &HashMap<(RadioSource, i64, String), f64>,
    post_affinity: &HashMap<(RadioSource, i64, String), f64>,
    jaccard_by_key: &HashMap<(RadioSource, i64, String), f64>,
) {
    for cand in candidates.iter_mut() {
        let key = (
            cand.source,
            cand.track_id,
            normalize_for_dedup(&cand.artist_name, &cand.title),
        );
        let pre_aff = pre_affinity.get(&key).copied();
        let post_aff = post_affinity.get(&key).copied();
        let affinity_mult = match (pre_aff, post_aff) {
            (Some(p), Some(a)) if p > 0.0 => Some(a / p),
            _ => None,
        };
        let genre_mult = match post_aff {
            Some(a) if a > 0.0 => Some(cand.similarity_score / a),
            _ => None,
        };
        let jaccard = jaccard_by_key.get(&key).copied();

        let mut parts: Vec<String> = Vec::new();
        if let Some(j) = jaccard {
            parts.push(format!("\"genre_jaccard\":{j:.4}"));
        }
        if let Some(m) = affinity_mult {
            parts.push(format!("\"affinity_mult\":{m:.4}"));
        }
        if let Some(m) = genre_mult {
            parts.push(format!("\"genre_mult\":{m:.4}"));
        }
        if parts.is_empty() {
            continue;
        }
        let suffix = format!("{{{}}}", parts.join(","));
        cand.reason = format!("{} | {}", cand.reason, suffix);
    }
}
