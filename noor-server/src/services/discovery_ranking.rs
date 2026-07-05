//! Multi-signal re-rank layer shared by the discovery Sound Space
//! (`/api/discovery/space`) and the multi-seed blend (`/api/discovery/blend/*`).
//!
//! Candidate generation still happens upstream (embedding neighbors via the
//! radio pipeline for space, `score_blend_candidate` for blend). This module
//! takes an already-produced base score and *shapes* it with the signals that
//! are cheap to attach and honest about their own absence:
//!
//! ```text
//! shaped = base * m_genre * m_harmonic * m_energy * m_artist * m_taste
//! ```
//!
//! Every multiplier collapses to exactly `1.0` when its inputs are missing, so
//! external (Last.fm / engine) candidates and unanalyzed library tracks are
//! never penalised for data we simply do not have. The one hard invariant:
//! at coherence `0.5` with every add-on signal absent, `shaped == base` to the
//! bit, so wiring this in front of the existing normalizer cannot shift the
//! default view.
//!
//! Shaping runs BEFORE the per-source normalization / prune in
//! `discovery_space`, so display scores stay in `[0, 1]` with no new clamp
//! regime downstream.

use std::collections::HashMap;

use serde::Deserialize;

use crate::genre::jaccard::weighted_jaccard;
use crate::services::audio_analysis::{
    CamelotRelation, camelot_relation, compute_harmonic_multiplier,
};
use crate::smart::taste_vector::TasteVector;

/// Base genre weight before coherence scaling. The effective weight is
/// `BASE_GENRE_WEIGHT * (0.5 + coherence)`, so a coherent view leans harder on
/// genre agreement and a diverse view relaxes it.
const BASE_GENRE_WEIGHT: f64 = 0.25;
/// Base exponent applied to the raw harmonic multiplier. The primitive returns
/// values as wide as ~0.39..3.96; raising to `alpha < 1` tames that spread so a
/// single key match cannot dominate the blend.
const BASE_HARMONIC_ALPHA: f64 = 0.35;
/// Fixed energy weight. Small on purpose: energy proximity is a nudge, not a
/// driver, and only library-vs-library pairs ever have it on both sides.
const ENERGY_WEIGHT: f64 = 0.10;
/// Fixed same-artist boost. Deliberately tiny: same-artist flooding is a known
/// radio failure mode, so we reward the link without letting one artist own the
/// map.
const ARTIST_BOOST: f64 = 1.12;

/// Coherence band the seed-path candidate generator should use. The routes
/// layer maps this onto `services::radio::RadioBlend` (this module stays free of
/// the radio dependency so it can be unit-tested in isolation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendBand {
    /// High coherence: hug the seed (maps to `RadioBlend::Familiar`).
    Coherent,
    /// Balanced default (maps to `RadioBlend::Mixed`).
    Balanced,
    /// Low coherence: explore outward (maps to `RadioBlend::Adventurous`).
    Diverse,
}

/// Map a `0..1` coherence value to a candidate-generation band. High coherence
/// stays near the seed; low coherence explores.
pub fn blend_band(coherence: f64) -> BlendBand {
    let c = coherence.clamp(0.0, 1.0);
    if c >= 0.67 {
        BlendBand::Coherent
    } else if c >= 0.33 {
        BlendBand::Balanced
    } else {
        BlendBand::Diverse
    }
}

/// Coherence-derived scaling knobs for a single request. Built once per request
/// and reused across every candidate.
#[derive(Debug, Clone, Copy)]
pub struct RankParams {
    genre_weight: f64,
    harmonic_alpha: f64,
    energy_weight: f64,
    artist_boost: f64,
}

impl RankParams {
    /// Derive params from a `0..1` coherence value (default caller value: 0.5).
    pub fn from_coherence(coherence: f64) -> Self {
        let c = coherence.clamp(0.0, 1.0);
        Self {
            genre_weight: BASE_GENRE_WEIGHT * (0.5 + c),
            harmonic_alpha: BASE_HARMONIC_ALPHA * (0.5 + c),
            energy_weight: ENERGY_WEIGHT,
            artist_boost: ARTIST_BOOST,
        }
    }
}

impl Default for RankParams {
    fn default() -> Self {
        Self::from_coherence(0.5)
    }
}

/// Feature snapshot for the seed (single seed for space; blended anchor union
/// for blend). Every optional field degrades to a neutral multiplier when
/// absent.
#[derive(Debug, Clone, Default)]
pub struct SeedFeatures {
    /// Weighted genre set (`weighted_genre_set` output). Empty = no genre data.
    pub genre_set: HashMap<String, f64>,
    pub camelot: Option<String>,
    pub bpm: Option<f64>,
    pub energy: Option<f64>,
    pub artist_id: Option<i64>,
    /// Lowercased artist name, for the external (no artist_id) compare path.
    pub artist_name_lc: Option<String>,
}

/// Feature snapshot for one candidate.
#[derive(Debug, Clone, Default)]
pub struct CandidateFeatures {
    pub track_id: i64,
    pub is_in_library: bool,
    /// Normalized source: `library` | `lastfm` | `engine` | ... Drives the
    /// why-fallback only.
    pub source: String,
    /// Upstream base score: `similarity_score` (space) or `blend_score` (blend).
    pub base_score: f64,
    pub genre_set: HashMap<String, f64>,
    pub camelot: Option<String>,
    pub bpm: Option<f64>,
    pub energy: Option<f64>,
    pub artist_id: Option<i64>,
    pub artist_name_lc: Option<String>,
    /// Blend only: how many seeds this candidate connects. 0 for the space path.
    pub covered_seed_count: usize,
}

/// Result of shaping one candidate.
#[derive(Debug, Clone)]
pub struct ShapedScore {
    pub score: f64,
    /// Compact human-readable reason (up to two phrases, comma-joined).
    pub why: String,
    /// Stable signal keys behind `why`, for the UI to render chips/icons.
    pub why_signals: Vec<&'static str>,
}

/// Shape a candidate's base score with the available signals and derive its
/// "why related" summary. `taste` is `Some` only on the live-rerank path.
pub fn shape_score(
    seed: &SeedFeatures,
    cand: &CandidateFeatures,
    params: &RankParams,
    taste: Option<&TasteVector>,
) -> ShapedScore {
    let (m_genre, jaccard) = genre_multiplier(seed, cand, params.genre_weight);
    let m_harmonic = harmonic_multiplier(seed, cand, params.harmonic_alpha);
    let m_energy = energy_multiplier(seed, cand, params.energy_weight);
    let same_artist = is_same_artist(seed, cand);
    let m_artist = if same_artist {
        params.artist_boost
    } else {
        1.0
    };
    let m_taste = taste.map_or(1.0, |t| taste_multiplier(t, cand));

    let score = cand.base_score * m_genre * m_harmonic * m_energy * m_artist * m_taste;
    let (why, why_signals) = derive_why(seed, cand, jaccard, same_artist);
    ShapedScore {
        score,
        why,
        why_signals,
    }
}

/// `1 + w*(2j - 1)` clamped to `[1-w, 1+w]`. Neutral when either side has no
/// genre data (so we never call `weighted_jaccard`, which would return 0.0 for
/// an empty-vs-populated pair and unfairly bury the candidate). Returns the raw
/// Jaccard alongside the multiplier so `derive_why` need not recompute it.
fn genre_multiplier(
    seed: &SeedFeatures,
    cand: &CandidateFeatures,
    weight: f64,
) -> (f64, Option<f64>) {
    if seed.genre_set.is_empty() || cand.genre_set.is_empty() {
        return (1.0, None);
    }
    let j = weighted_jaccard(&seed.genre_set, &cand.genre_set);
    let m = (1.0 + weight * (2.0 * j - 1.0)).clamp(1.0 - weight, 1.0 + weight);
    (m, Some(j))
}

/// Reuse `compute_harmonic_multiplier` verbatim, then tame its wide range with
/// `pow(alpha)` and a symmetric clamp. The primitive returns exactly `1.0` when
/// either key/tempo pair is missing, which we pass through untouched.
fn harmonic_multiplier(seed: &SeedFeatures, cand: &CandidateFeatures, alpha: f64) -> f64 {
    let h = compute_harmonic_multiplier(
        seed.camelot.as_deref(),
        cand.camelot.as_deref(),
        seed.bpm,
        cand.bpm,
    );
    if (h - 1.0).abs() < 1e-9 {
        return 1.0;
    }
    h.powf(alpha).clamp(0.70, 1.40)
}

/// `1 + w*(1 - 2|de|)` clamped to `[1-w, 1+w]`; neutral unless both sides have
/// energy. Identical energy -> `1+w`, opposite -> `1-w`.
fn energy_multiplier(seed: &SeedFeatures, cand: &CandidateFeatures, weight: f64) -> f64 {
    match (seed.energy, cand.energy) {
        (Some(se), Some(ce)) => {
            (1.0 + weight * (1.0 - 2.0 * (se - ce).abs())).clamp(1.0 - weight, 1.0 + weight)
        }
        _ => 1.0,
    }
}

/// Same-artist test. Trust `artist_id` when both sides carry it (authoritative,
/// even if names happen to collide); fall back to a lowercased name compare only
/// when an id is missing (the external candidate case).
fn is_same_artist(seed: &SeedFeatures, cand: &CandidateFeatures) -> bool {
    match (seed.artist_id, cand.artist_id) {
        (Some(a), Some(b)) => a == b,
        _ => match (&seed.artist_name_lc, &cand.artist_name_lc) {
            (Some(a), Some(b)) => !a.is_empty() && a == b,
            _ => false,
        },
    }
}

/// Fold session like/skip signal into the score. Asymmetric coefficients
/// (negatives bite harder) match the automix convention. Only ever applied on
/// the rerank path where a session `TasteVector` exists.
fn taste_multiplier(taste: &TasteVector, cand: &CandidateFeatures) -> f64 {
    let a_term = cand
        .artist_id
        .and_then(|aid| taste.artist_affinity.get(&aid))
        .map_or(0.0, |s| 0.08 * s.pos - 0.12 * s.neg);

    // Take the strongest matching genre so a track tagged with many segments is
    // not counted many times.
    let mut g_pos = 0.0_f64;
    let mut g_neg = 0.0_f64;
    for key in cand.genre_set.keys() {
        if let Some(s) = taste.genre_affinity.get(&key.to_lowercase()) {
            g_pos = g_pos.max(s.pos);
            g_neg = g_neg.max(s.neg);
        }
    }
    let g_term = 0.06 * g_pos - 0.09 * g_neg;

    let mut m = (1.0 + a_term + g_term).clamp(0.6, 1.3);
    if taste.skipped_track_ids.contains(&cand.track_id) {
        m *= 0.2;
    }
    m
}

/// Derive up to two "why related" phrases from the signals that actually fired,
/// in priority order: key+tempo, genre, artist, energy, then a source-based
/// fallback so every candidate says something.
fn derive_why(
    seed: &SeedFeatures,
    cand: &CandidateFeatures,
    jaccard: Option<f64>,
    same_artist: bool,
) -> (String, Vec<&'static str>) {
    let mut hits: Vec<(String, &'static str)> = Vec::new();

    let key_compatible = match (seed.camelot.as_deref(), cand.camelot.as_deref()) {
        (Some(a), Some(b)) => camelot_relation(a, b) == CamelotRelation::Compatible,
        _ => false,
    };
    let bpm_diff = match (seed.bpm, cand.bpm) {
        (Some(a), Some(b)) => Some((a - b).abs()),
        _ => None,
    };

    if key_compatible && bpm_diff.is_some_and(|d| d < 10.0) {
        hits.push(("same key, close BPM".to_string(), "key_bpm"));
    } else if key_compatible {
        hits.push(("compatible key".to_string(), "key"));
    } else if bpm_diff.is_some_and(|d| d < 5.0) {
        hits.push(("matching tempo".to_string(), "bpm"));
    }

    if let Some(j) = jaccard {
        if j >= 0.5 {
            hits.push(("shared genre cluster".to_string(), "genre_strong"));
        } else if j >= 0.25 {
            hits.push(("related genre".to_string(), "genre"));
        }
    }

    if same_artist {
        hits.push(("same artist".to_string(), "artist"));
    }

    if let (Some(se), Some(ce)) = (seed.energy, cand.energy) {
        if (se - ce).abs() <= 0.15 {
            hits.push(("matching energy".to_string(), "energy"));
        }
    }

    if hits.is_empty() {
        if cand.source == "lastfm" {
            hits.push(("Last.fm listeners agree".to_string(), "lastfm"));
        } else if cand.covered_seed_count >= 2 {
            hits.push((
                format!("bridges {} seeds", cand.covered_seed_count),
                "bridge",
            ));
        } else {
            hits.push(("embedding close".to_string(), "embedding"));
        }
    }

    hits.truncate(2);
    let why = hits
        .iter()
        .map(|(phrase, _)| phrase.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let signals = hits.iter().map(|(_, key)| *key).collect::<Vec<_>>();
    (why, signals)
}

/// User-set constraints on the candidate set. Applied after generation and
/// before pruning. The seed itself is always exempt (filtered by the caller).
/// Deserializes straight from the request body; every field is optional so an
/// absent `filters` object reproduces the unfiltered behavior.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SpaceFilters {
    pub bpm_min: Option<f64>,
    pub bpm_max: Option<f64>,
    pub energy_min: Option<f64>,
    pub energy_max: Option<f64>,
    /// Keep only candidates whose Camelot key is not a hard clash with the seed.
    /// No-op when the seed itself is unanalyzed. When active, candidates lacking
    /// a Camelot key are dropped (that is what "only" means; the UI warns that
    /// this hides unanalyzed and external tracks).
    pub key_compatible_only: bool,
    pub year_min: Option<i64>,
    pub year_max: Option<i64>,
    pub exclude_in_library: bool,
    pub exclude_heard_session: bool,
}

impl SpaceFilters {
    /// True when no constraint is set, so the caller can skip the per-candidate
    /// pass (and the supporting queries) entirely.
    pub fn is_noop(&self) -> bool {
        self.bpm_min.is_none()
            && self.bpm_max.is_none()
            && self.energy_min.is_none()
            && self.energy_max.is_none()
            && !self.key_compatible_only
            && self.year_min.is_none()
            && self.year_max.is_none()
            && !self.exclude_in_library
            && !self.exclude_heard_session
    }
}

/// Decide whether a candidate survives the filters. Missing bpm / energy / year
/// always PASS (dropping them would erase every external candidate). The
/// key-compatible filter is the one exception: when the seed has a key, a
/// candidate with no key is dropped.
pub fn passes_filters(
    filters: &SpaceFilters,
    cand: &CandidateFeatures,
    seed_camelot: Option<&str>,
    year: Option<i64>,
    heard_in_session: bool,
) -> bool {
    if let Some(b) = cand.bpm {
        if filters.bpm_min.is_some_and(|min| b < min) || filters.bpm_max.is_some_and(|max| b > max)
        {
            return false;
        }
    }
    if let Some(e) = cand.energy {
        if filters.energy_min.is_some_and(|min| e < min)
            || filters.energy_max.is_some_and(|max| e > max)
        {
            return false;
        }
    }
    if filters.key_compatible_only {
        if let Some(sc) = seed_camelot {
            match cand.camelot.as_deref() {
                Some(cc) => {
                    if camelot_relation(sc, cc) == CamelotRelation::Clash {
                        return false;
                    }
                }
                None => return false,
            }
        }
    }
    if let Some(y) = year {
        if filters.year_min.is_some_and(|min| y < min)
            || filters.year_max.is_some_and(|max| y > max)
        {
            return false;
        }
    }
    if filters.exclude_in_library && cand.is_in_library {
        return false;
    }
    if filters.exclude_heard_session && heard_in_session {
        return false;
    }
    true
}

/// One recorded feedback event, pre-resolved to the fields the taste builder
/// needs. `genres` are lowercased genre names.
// The feedback/taste trio below wires in with the rerank endpoint; the allows
// go away with it.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct FeedbackRow {
    pub candidate_track_id: i64,
    pub action: FeedbackAction,
    pub artist_id: Option<i64>,
    pub genres: Vec<String>,
}

/// The three feedback actions the discovery surface records.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackAction {
    Like,
    Skip,
    Dismiss,
}

impl FeedbackAction {
    /// Parse the wire string. Returns `None` for anything outside the allowlist
    /// so the handler can reject it with a 400.
    #[allow(dead_code)]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "like" => Some(Self::Like),
            "skip" => Some(Self::Skip),
            "dismiss" => Some(Self::Dismiss),
            _ => None,
        }
    }
}

/// Build a session `TasteVector` from recorded feedback. Likes add positive
/// artist/genre signal; skips add negative signal and suppress the track;
/// dismiss suppresses the track only (the user hid that specific track, which is
/// not necessarily a vote against its artist or genre). Reuses the canonical
/// type - no fork.
#[allow(dead_code)]
pub fn build_session_taste(rows: &[FeedbackRow]) -> TasteVector {
    let mut tv = TasteVector::default();
    for row in rows {
        match row.action {
            FeedbackAction::Like => {
                if let Some(aid) = row.artist_id {
                    tv.artist_affinity.entry(aid).or_default().pos += 1.0;
                }
                for g in &row.genres {
                    tv.genre_affinity.entry(g.to_lowercase()).or_default().pos += 1.0;
                }
            }
            FeedbackAction::Skip => {
                if let Some(aid) = row.artist_id {
                    tv.artist_affinity.entry(aid).or_default().neg += 1.0;
                }
                for g in &row.genres {
                    tv.genre_affinity.entry(g.to_lowercase()).or_default().neg += 1.0;
                }
                tv.skipped_track_ids.insert(row.candidate_track_id);
            }
            FeedbackAction::Dismiss => {
                tv.skipped_track_ids.insert(row.candidate_track_id);
            }
        }
    }
    tv
}

#[cfg(test)]
mod tests {
    use super::*;

    fn genre_set(paths: &[&str]) -> HashMap<String, f64> {
        crate::genre::jaccard::weighted_genre_set(
            &paths.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
    }

    fn bare_candidate(base: f64) -> CandidateFeatures {
        CandidateFeatures {
            track_id: 1,
            is_in_library: false,
            source: "engine".to_string(),
            base_score: base,
            ..Default::default()
        }
    }

    #[test]
    fn identity_at_balanced_coherence_with_no_signals() {
        // The load-bearing invariant: default view must not shift.
        let params = RankParams::from_coherence(0.5);
        let seed = SeedFeatures::default();
        let cand = bare_candidate(0.734);
        let shaped = shape_score(&seed, &cand, &params, None);
        assert_eq!(shaped.score, 0.734);
    }

    #[test]
    fn identity_holds_across_all_coherence_values_when_signals_absent() {
        for c in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let params = RankParams::from_coherence(c);
            let shaped = shape_score(
                &SeedFeatures::default(),
                &bare_candidate(0.5),
                &params,
                None,
            );
            assert!((shaped.score - 0.5).abs() < 1e-12, "coherence {c} drifted");
        }
    }

    #[test]
    fn blend_band_maps_coherence_to_generation_band() {
        assert_eq!(blend_band(0.9), BlendBand::Coherent);
        assert_eq!(blend_band(0.67), BlendBand::Coherent);
        assert_eq!(blend_band(0.5), BlendBand::Balanced);
        assert_eq!(blend_band(0.33), BlendBand::Balanced);
        assert_eq!(blend_band(0.1), BlendBand::Diverse);
        // Out-of-range clamps rather than panicking.
        assert_eq!(blend_band(-1.0), BlendBand::Diverse);
        assert_eq!(blend_band(2.0), BlendBand::Coherent);
    }

    #[test]
    fn genre_multiplier_rewards_overlap_and_clamps() {
        let params = RankParams::from_coherence(0.5); // genre_weight = 0.25
        let seed = SeedFeatures {
            genre_set: genre_set(&["Electronic > House"]),
            ..Default::default()
        };
        // Identical genre -> j = 1.0 -> multiplier at the +weight clamp.
        let same = CandidateFeatures {
            genre_set: genre_set(&["Electronic > House"]),
            ..bare_candidate(1.0)
        };
        let (m, j) = genre_multiplier(&seed, &same, params.genre_weight);
        assert_eq!(j, Some(1.0));
        assert!((m - 1.25).abs() < 1e-9);

        // Disjoint genre -> j = 0.0 -> multiplier at the -weight clamp.
        let other = CandidateFeatures {
            genre_set: genre_set(&["Metal > Thrash"]),
            ..bare_candidate(1.0)
        };
        let (m2, j2) = genre_multiplier(&seed, &other, params.genre_weight);
        assert_eq!(j2, Some(0.0));
        assert!((m2 - 0.75).abs() < 1e-9);
    }

    #[test]
    fn genre_multiplier_neutral_when_either_side_empty() {
        let params = RankParams::from_coherence(0.5);
        let seed = SeedFeatures {
            genre_set: genre_set(&["Electronic > House"]),
            ..Default::default()
        };
        let no_genre = bare_candidate(1.0);
        let (m, j) = genre_multiplier(&seed, &no_genre, params.genre_weight);
        assert_eq!(m, 1.0);
        assert_eq!(j, None);
    }

    #[test]
    fn harmonic_multiplier_neutral_without_key_or_bpm() {
        let m = harmonic_multiplier(
            &SeedFeatures::default(),
            &bare_candidate(1.0),
            BASE_HARMONIC_ALPHA,
        );
        assert_eq!(m, 1.0);
    }

    #[test]
    fn harmonic_multiplier_boosts_compatible_and_clamps_range() {
        let seed = SeedFeatures {
            camelot: Some("8A".to_string()),
            bpm: Some(124.0),
            ..Default::default()
        };
        // Same key, near tempo -> raw ~3.96 -> shaped, clamped to 1.40 ceiling.
        let close = CandidateFeatures {
            camelot: Some("8A".to_string()),
            bpm: Some(125.0),
            ..bare_candidate(1.0)
        };
        let m = harmonic_multiplier(&seed, &close, 0.35);
        assert!(
            m > 1.0 && m <= 1.40,
            "expected boost within ceiling, got {m}"
        );

        // Hard clash, far tempo -> raw ~0.39 -> shaped, floored at 0.70.
        let clash = CandidateFeatures {
            camelot: Some("3B".to_string()),
            bpm: Some(180.0),
            ..bare_candidate(1.0)
        };
        let m2 = harmonic_multiplier(&seed, &clash, 0.35);
        assert!(
            m2 >= 0.70 && m2 < 1.0,
            "expected penalty above floor, got {m2}"
        );
    }

    #[test]
    fn energy_multiplier_symmetry_and_neutrality() {
        let seed = SeedFeatures {
            energy: Some(0.5),
            ..Default::default()
        };
        let identical = CandidateFeatures {
            energy: Some(0.5),
            ..bare_candidate(1.0)
        };
        assert!((energy_multiplier(&seed, &identical, ENERGY_WEIGHT) - 1.10).abs() < 1e-9);

        let opposite = CandidateFeatures {
            energy: Some(1.0),
            ..bare_candidate(1.0)
        };
        // |de| = 0.5 -> 1 + 0.1*(1 - 1) = 1.0
        assert!((energy_multiplier(&seed, &opposite, ENERGY_WEIGHT) - 1.0).abs() < 1e-9);

        // Missing on candidate side -> neutral.
        assert_eq!(
            energy_multiplier(&seed, &bare_candidate(1.0), ENERGY_WEIGHT),
            1.0
        );
    }

    #[test]
    fn same_artist_prefers_id_then_name() {
        let seed = SeedFeatures {
            artist_id: Some(7),
            artist_name_lc: Some("aphex twin".to_string()),
            ..Default::default()
        };
        let same_id = CandidateFeatures {
            artist_id: Some(7),
            ..bare_candidate(1.0)
        };
        assert!(is_same_artist(&seed, &same_id));

        // Different id with a colliding name is NOT the same artist.
        let diff_id = CandidateFeatures {
            artist_id: Some(8),
            artist_name_lc: Some("aphex twin".to_string()),
            ..bare_candidate(1.0)
        };
        assert!(!is_same_artist(&seed, &diff_id));

        // External (no id) matches on name.
        let external = CandidateFeatures {
            artist_id: None,
            artist_name_lc: Some("aphex twin".to_string()),
            ..bare_candidate(1.0)
        };
        assert!(is_same_artist(&seed, &external));
    }

    #[test]
    fn taste_multiplier_asymmetry_and_hard_skip() {
        let liked = build_session_taste(&[FeedbackRow {
            candidate_track_id: 100,
            action: FeedbackAction::Like,
            artist_id: Some(42),
            genres: vec!["house".to_string()],
        }]);
        let cand = CandidateFeatures {
            track_id: 200,
            artist_id: Some(42),
            genre_set: genre_set(&["Electronic > House"]),
            ..bare_candidate(1.0)
        };
        let m = taste_multiplier(&liked, &cand);
        assert!(m > 1.0, "liked artist+genre should lift, got {m}");

        // A skipped track id collapses hard regardless of affinity.
        let skipped = build_session_taste(&[FeedbackRow {
            candidate_track_id: 200,
            action: FeedbackAction::Skip,
            artist_id: Some(42),
            genres: vec!["house".to_string()],
        }]);
        let m2 = taste_multiplier(&skipped, &cand);
        assert!(m2 < 0.3, "skipped track should be suppressed, got {m2}");
    }

    #[test]
    fn why_prioritises_key_bpm_then_genre() {
        let seed = SeedFeatures {
            camelot: Some("8A".to_string()),
            bpm: Some(120.0),
            genre_set: genre_set(&["Electronic > House"]),
            ..Default::default()
        };
        let cand = CandidateFeatures {
            camelot: Some("8A".to_string()),
            bpm: Some(122.0),
            genre_set: genre_set(&["Electronic > House"]),
            ..bare_candidate(1.0)
        };
        let shaped = shape_score(&seed, &cand, &RankParams::from_coherence(0.5), None);
        assert_eq!(shaped.why_signals.first(), Some(&"key_bpm"));
        assert!(shaped.why_signals.len() <= 2);
        assert!(shaped.why.contains("same key, close BPM"));
    }

    #[test]
    fn why_falls_back_to_source_when_no_signal_fires() {
        let lastfm = CandidateFeatures {
            source: "lastfm".to_string(),
            ..bare_candidate(1.0)
        };
        let shaped = shape_score(
            &SeedFeatures::default(),
            &lastfm,
            &RankParams::from_coherence(0.5),
            None,
        );
        assert_eq!(shaped.why_signals, vec!["lastfm"]);

        let bridge = CandidateFeatures {
            source: "engine".to_string(),
            covered_seed_count: 3,
            ..bare_candidate(1.0)
        };
        let shaped2 = shape_score(
            &SeedFeatures::default(),
            &bridge,
            &RankParams::from_coherence(0.5),
            None,
        );
        assert_eq!(shaped2.why_signals, vec!["bridge"]);
        assert!(shaped2.why.contains("bridges 3 seeds"));
    }

    #[test]
    fn filters_pass_missing_signals_but_key_only_drops_unanalyzed() {
        let seed_key = Some("8A");
        // Candidate with no bpm/energy passes a bpm/energy range filter.
        let ranged = SpaceFilters {
            bpm_min: Some(120.0),
            bpm_max: Some(130.0),
            energy_min: Some(0.4),
            ..Default::default()
        };
        let no_dsp = bare_candidate(1.0);
        assert!(passes_filters(&ranged, &no_dsp, seed_key, None, false));

        // key_compatible_only with a seed key drops a candidate that has no key.
        let key_only = SpaceFilters {
            key_compatible_only: true,
            ..Default::default()
        };
        assert!(!passes_filters(&key_only, &no_dsp, seed_key, None, false));
        // ... but is a no-op when the seed itself has no key.
        assert!(passes_filters(&key_only, &no_dsp, None, None, false));

        // A clashing key is dropped; a compatible one passes.
        let clash = CandidateFeatures {
            camelot: Some("3B".to_string()),
            ..bare_candidate(1.0)
        };
        assert!(!passes_filters(&key_only, &clash, seed_key, None, false));
        let compatible = CandidateFeatures {
            camelot: Some("9A".to_string()),
            ..bare_candidate(1.0)
        };
        assert!(passes_filters(
            &key_only,
            &compatible,
            seed_key,
            None,
            false
        ));
    }

    #[test]
    fn filters_year_null_passes_and_range_bounds() {
        let era = SpaceFilters {
            year_min: Some(1990),
            year_max: Some(1999),
            ..Default::default()
        };
        let cand = bare_candidate(1.0);
        // Null year passes (album year is often absent).
        assert!(passes_filters(&era, &cand, None, None, false));
        assert!(passes_filters(&era, &cand, None, Some(1995), false));
        assert!(!passes_filters(&era, &cand, None, Some(2005), false));
        assert!(!passes_filters(&era, &cand, None, Some(1980), false));
    }

    #[test]
    fn filters_exclude_library_and_heard() {
        let lib = CandidateFeatures {
            is_in_library: true,
            ..bare_candidate(1.0)
        };
        let excl_lib = SpaceFilters {
            exclude_in_library: true,
            ..Default::default()
        };
        assert!(!passes_filters(&excl_lib, &lib, None, None, false));

        let excl_heard = SpaceFilters {
            exclude_heard_session: true,
            ..Default::default()
        };
        assert!(!passes_filters(
            &excl_heard,
            &bare_candidate(1.0),
            None,
            None,
            true
        ));
        assert!(passes_filters(
            &excl_heard,
            &bare_candidate(1.0),
            None,
            None,
            false
        ));
    }

    #[test]
    fn feedback_action_parse_allowlist() {
        assert_eq!(FeedbackAction::parse("like"), Some(FeedbackAction::Like));
        assert_eq!(FeedbackAction::parse("skip"), Some(FeedbackAction::Skip));
        assert_eq!(
            FeedbackAction::parse("dismiss"),
            Some(FeedbackAction::Dismiss)
        );
        assert_eq!(FeedbackAction::parse("delete"), None);
        assert_eq!(FeedbackAction::parse(""), None);
    }

    #[test]
    fn session_taste_accumulates_and_dismiss_is_track_only() {
        let tv = build_session_taste(&[
            FeedbackRow {
                candidate_track_id: 1,
                action: FeedbackAction::Like,
                artist_id: Some(10),
                genres: vec!["House".to_string()],
            },
            FeedbackRow {
                candidate_track_id: 2,
                action: FeedbackAction::Skip,
                artist_id: Some(20),
                genres: vec!["Metal".to_string()],
            },
            FeedbackRow {
                candidate_track_id: 3,
                action: FeedbackAction::Dismiss,
                artist_id: Some(30),
                genres: vec!["Jazz".to_string()],
            },
        ]);
        assert!(tv.artist_affinity.get(&10).unwrap().pos > 0.0);
        assert!(tv.artist_affinity.get(&20).unwrap().neg > 0.0);
        // Genre keys are lowercased.
        assert!(tv.genre_affinity.contains_key("house"));
        assert!(tv.skipped_track_ids.contains(&2));
        // Dismiss suppresses the track but leaves no artist/genre vote.
        assert!(tv.skipped_track_ids.contains(&3));
        assert!(!tv.artist_affinity.contains_key(&30));
        assert!(!tv.genre_affinity.contains_key("jazz"));
    }
}
