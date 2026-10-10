use crate::db::models::{AudioDjProfileKey, AudioDspFeatures};
use crate::db::queries;
use crate::services::audio_analysis::{CamelotRelation, camelot_relation, tempo_delta};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use std::cmp::Ordering;

const READY_PROFILE_CONFIDENCE: f64 = 0.65;
const PARTIAL_PROFILE_CONFIDENCE: f64 = 0.4;

#[derive(Debug, Clone)]
pub(crate) struct GeneratedCandidate<T> {
    pub(crate) item: T,
    pub(crate) track_id: Option<i64>,
    pub(crate) tidal_id: Option<i64>,
    pub(crate) policy: GeneratedCandidatePolicy,
}

#[derive(Debug, Clone)]
pub(crate) struct GeneratedCandidatePolicy {
    pub(crate) score_multiplier: f64,
    pub(crate) reasons: Vec<&'static str>,
}

impl Default for GeneratedCandidatePolicy {
    fn default() -> Self {
        Self {
            score_multiplier: 1.0,
            reasons: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RankedGeneratedCandidate<T> {
    pub(crate) item: T,
    pub(crate) score: f64,
    pub(crate) reasons: Vec<&'static str>,
}

#[derive(Debug, Clone, Default)]
struct DjQueueFacts {
    features: Option<AudioDspFeatures>,
    profile_confidence: Option<f64>,
    safe_crossfade_only: bool,
    phrase_count: usize,
    drop_count: usize,
}

#[derive(Debug, Clone)]
struct ScoredCandidate<T> {
    ordinal: usize,
    ranked: RankedGeneratedCandidate<T>,
}

/// Callers hand candidates over best first, so input rank stands in for
/// relevance: 1.0 for the first, 0.5 for the eleventh.
const RANK_RELEVANCE_DECAY: f64 = 0.1;
/// DJ fit nudges relevance instead of replacing it: the raw fit (x0.4 to x3.3)
/// is tamed the way discovery_ranking tames harmonic fit, so a strong fit can
/// lift a candidate a few places but never past clearly better matches.
const DJ_FIT_EXPONENT: f64 = 0.35;
const DJ_FIT_MIN: f64 = 0.8;
const DJ_FIT_MAX: f64 = 1.25;

fn rank_relevance(ordinal: usize) -> f64 {
    1.0 / (1.0 + RANK_RELEVANCE_DECAY * ordinal as f64)
}

pub(crate) fn dj_fit_multiplier(fit: f64) -> f64 {
    fit.max(0.0)
        .powf(DJ_FIT_EXPONENT)
        .clamp(DJ_FIT_MIN, DJ_FIT_MAX)
}

/// True when transitions are mixed (crossfade or the DJ engine), the only
/// time key and tempo fit should touch queue order.
pub(crate) fn mixing_active(conn: &Connection) -> bool {
    let crossfade_ms: i64 = conn
        .query_row(
            "SELECT COALESCE(crossfade_ms, 0) FROM playback_state WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    crossfade_ms > 0 || queries::is_dj_engine_enabled(conn).unwrap_or(false)
}

fn passthrough<T>(candidates: Vec<GeneratedCandidate<T>>) -> Vec<RankedGeneratedCandidate<T>> {
    candidates
        .into_iter()
        .map(|candidate| RankedGeneratedCandidate {
            item: candidate.item,
            score: candidate.policy.score_multiplier,
            reasons: candidate.policy.reasons,
        })
        .collect()
}

/// Order candidates by relevance (input rank) x caller policy, nudged by DJ
/// fit against the seed only while mixing. Without mixing no DJ facts are
/// loaded and no DJ reasons are reported.
pub(crate) fn rank_generated_candidates<T>(
    conn: &Connection,
    seed_track_id: i64,
    candidates: Vec<GeneratedCandidate<T>>,
    mixing: bool,
) -> Result<Vec<RankedGeneratedCandidate<T>>> {
    if candidates.len() <= 1 {
        return Ok(passthrough(candidates));
    }

    let seed = if mixing {
        Some(load_facts_for_track(conn, seed_track_id)?)
    } else {
        None
    };
    let mut scored = candidates
        .into_iter()
        .enumerate()
        .map(|(ordinal, candidate)| {
            let mut score = rank_relevance(ordinal) * candidate.policy.score_multiplier;
            let mut reasons = Vec::new();
            if let Some(seed) = seed.as_ref() {
                let facts = load_facts(conn, candidate.track_id, candidate.tidal_id)?;
                let (fit, fit_reasons) = score_facts(seed, &facts);
                score *= dj_fit_multiplier(fit);
                reasons = fit_reasons;
            }
            reasons.extend(candidate.policy.reasons);
            Ok(ScoredCandidate {
                ordinal,
                ranked: RankedGeneratedCandidate {
                    item: candidate.item,
                    score,
                    reasons,
                },
            })
        })
        .collect::<Result<Vec<_>>>()?;

    scored.sort_by(|left, right| {
        right
            .ranked
            .score
            .partial_cmp(&left.ranked.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.ordinal.cmp(&right.ordinal))
    });

    Ok(scored.into_iter().map(|entry| entry.ranked).collect())
}

/// Like `rank_generated_candidates`, but while mixing each pick is scored
/// against the previous pick instead of the seed, so the run flows track to
/// track. Callers pass the queue tail as the seed.
pub(crate) fn rank_generated_candidates_chain<T>(
    conn: &Connection,
    seed_track_id: i64,
    candidates: Vec<GeneratedCandidate<T>>,
    mixing: bool,
) -> Result<Vec<RankedGeneratedCandidate<T>>> {
    if candidates.len() <= 1 || !mixing {
        return rank_generated_candidates(conn, seed_track_id, candidates, false);
    }

    let mut previous = load_facts_for_track(conn, seed_track_id)?;
    let mut remaining = candidates
        .into_iter()
        .enumerate()
        .map(|(ordinal, candidate)| {
            let facts = load_facts(conn, candidate.track_id, candidate.tidal_id)?;
            Ok((ordinal, candidate.item, facts, candidate.policy))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut ranked = Vec::with_capacity(remaining.len());

    let score_of = |previous: &DjQueueFacts,
                    ordinal: usize,
                    facts: &DjQueueFacts,
                    policy: &GeneratedCandidatePolicy| {
        let (fit, reasons) = score_facts(previous, facts);
        (
            rank_relevance(ordinal) * policy.score_multiplier * dj_fit_multiplier(fit),
            reasons,
        )
    };

    while !remaining.is_empty() {
        let mut best_index = 0usize;
        let mut best_score = f64::NEG_INFINITY;
        for (idx, (ordinal, _, facts, policy)) in remaining.iter().enumerate() {
            let (score, _) = score_of(&previous, *ordinal, facts, policy);
            let score_order = score.partial_cmp(&best_score).unwrap_or(Ordering::Equal);
            let is_better = score_order == Ordering::Greater
                || (score_order == Ordering::Equal && *ordinal < remaining[best_index].0);
            if is_better {
                best_index = idx;
                best_score = score;
            }
        }

        let (ordinal, item, facts, policy) = remaining.remove(best_index);
        let (score, mut reasons) = score_of(&previous, ordinal, &facts, &policy);
        reasons.extend(policy.reasons);
        previous = facts;
        ranked.push(RankedGeneratedCandidate {
            item,
            score,
            reasons,
        });
    }

    Ok(ranked)
}

pub(crate) fn append_dj_reason(existing: &str, score: f64, reasons: &[&'static str]) -> String {
    if reasons.is_empty() {
        return existing.to_string();
    }
    format!(
        "{} | dj: {} | {{\"dj_score\":{:.4}}}",
        existing,
        reasons
            .iter()
            .take(3)
            .copied()
            .collect::<Vec<_>>()
            .join(", "),
        score
    )
}

fn load_facts(
    conn: &Connection,
    track_id: Option<i64>,
    tidal_id: Option<i64>,
) -> Result<DjQueueFacts> {
    let local_track_id = match (track_id, tidal_id) {
        (Some(track_id), _) if track_id > 0 => Some(track_id),
        (_, Some(tidal_id)) if tidal_id > 0 => local_track_id_for_tidal_id(conn, tidal_id)?,
        _ => None,
    };
    if let Some(track_id) = local_track_id {
        return load_facts_for_track(conn, track_id);
    }
    if let Some(tidal_id) = tidal_id.filter(|id| *id > 0) {
        return load_facts_for_key(
            conn,
            AudioDjProfileKey {
                media_ref_kind: "tidal_track".to_string(),
                media_ref_id: tidal_id.to_string(),
            },
            None,
        );
    }
    Ok(DjQueueFacts::default())
}

fn load_facts_for_track(conn: &Connection, track_id: i64) -> Result<DjQueueFacts> {
    let features = queries::get_audio_dsp_features(conn, track_id)
        .ok()
        .flatten();
    let profile = queries::get_audio_dj_profile_for_track(conn, track_id)
        .ok()
        .flatten();
    let library_key = AudioDjProfileKey {
        media_ref_kind: "library_track".to_string(),
        media_ref_id: track_id.to_string(),
    };
    let correction = queries::get_audio_dj_profile_correction(conn, &library_key)
        .ok()
        .flatten()
        .or_else(|| {
            tidal_id_for_track(conn, track_id)
                .ok()
                .flatten()
                .and_then(|tidal_id| {
                    queries::get_audio_dj_profile_correction(
                        conn,
                        &AudioDjProfileKey {
                            media_ref_kind: "tidal_track".to_string(),
                            media_ref_id: tidal_id.to_string(),
                        },
                    )
                    .ok()
                    .flatten()
                })
        });
    Ok(DjQueueFacts {
        features,
        profile_confidence: profile.as_ref().map(|profile| profile.profile_confidence),
        safe_crossfade_only: correction
            .as_ref()
            .is_some_and(|correction| correction.safe_crossfade_only),
        phrase_count: profile
            .as_ref()
            .map(|profile| profile.phrase_boundaries_blob.len() / 4)
            .unwrap_or_default(),
        drop_count: profile
            .as_ref()
            .map(|profile| profile.drop_blob.len() / 12)
            .unwrap_or_default(),
    })
}

fn load_facts_for_key(
    conn: &Connection,
    key: AudioDjProfileKey,
    track_id: Option<i64>,
) -> Result<DjQueueFacts> {
    let features = track_id.and_then(|id| queries::get_audio_dsp_features(conn, id).ok().flatten());
    let profile = queries::get_audio_dj_profile(conn, &key).ok().flatten();
    let correction = queries::get_audio_dj_profile_correction(conn, &key)
        .ok()
        .flatten();
    Ok(DjQueueFacts {
        features,
        profile_confidence: profile.as_ref().map(|profile| profile.profile_confidence),
        safe_crossfade_only: correction
            .as_ref()
            .is_some_and(|correction| correction.safe_crossfade_only),
        phrase_count: profile
            .as_ref()
            .map(|profile| profile.phrase_boundaries_blob.len() / 4)
            .unwrap_or_default(),
        drop_count: profile
            .as_ref()
            .map(|profile| profile.drop_blob.len() / 12)
            .unwrap_or_default(),
    })
}

fn local_track_id_for_tidal_id(conn: &Connection, tidal_id: i64) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT id FROM tracks WHERE tidal_id = ?1 LIMIT 1",
        params![tidal_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

fn tidal_id_for_track(conn: &Connection, track_id: i64) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT tidal_id FROM tracks WHERE id = ?1",
        params![track_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

fn score_facts(seed: &DjQueueFacts, candidate: &DjQueueFacts) -> (f64, Vec<&'static str>) {
    let mut score = 1.0;
    let mut reasons = Vec::new();

    if let (Some(seed_features), Some(candidate_features)) =
        (seed.features.as_ref(), candidate.features.as_ref())
    {
        if let (Some(seed_bpm), Some(candidate_bpm)) = (seed_features.bpm, candidate_features.bpm) {
            match tempo_fit(seed_bpm, candidate_bpm) {
                TempoFit::InsideNudge => {
                    score *= 1.55;
                    reasons.push("tempo inside 3 percent");
                }
                TempoFit::Near => {
                    score *= 1.18;
                    reasons.push("near tempo");
                }
                TempoFit::Wide => {
                    score *= 0.72;
                    reasons.push("wide tempo");
                }
            }
        }

        if let (Some(seed_key), Some(candidate_key)) = (
            seed_features.camelot_key.as_deref(),
            candidate_features.camelot_key.as_deref(),
        ) {
            match camelot_relation(seed_key, candidate_key) {
                CamelotRelation::Compatible => {
                    score *= 1.45;
                    reasons.push("harmonic match");
                }
                CamelotRelation::Adjacent => {
                    score *= 1.18;
                    reasons.push("adjacent key");
                }
                CamelotRelation::Clash => {
                    score *= 0.72;
                    reasons.push("key clash");
                }
            }
        }
    }

    if candidate.safe_crossfade_only {
        score *= 0.55;
        reasons.push("safe-only profile");
    } else if let Some(confidence) = candidate.profile_confidence {
        if confidence >= READY_PROFILE_CONFIDENCE {
            score *= 1.28;
            reasons.push("ready DJ profile");
        } else if confidence >= PARTIAL_PROFILE_CONFIDENCE {
            score *= 1.08;
            reasons.push("partial DJ profile");
        }
    }

    if candidate.drop_count > 0 {
        score *= 1.08;
        reasons.push("drop marker");
    }
    if candidate.phrase_count >= 4 {
        score *= 1.05;
        reasons.push("phrase markers");
    }

    (score, reasons)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TempoFit {
    InsideNudge,
    Near,
    Wide,
}

fn tempo_fit(seed_bpm: f64, candidate_bpm: f64) -> TempoFit {
    match tempo_delta(Some(seed_bpm), Some(candidate_bpm)) {
        Some(delta) if delta <= 0.03 => TempoFit::InsideNudge,
        Some(delta) if delta <= 0.08 => TempoFit::Near,
        _ => TempoFit::Wide,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::AudioDspFeatures;

    fn features(bpm: f64, key: &str) -> AudioDspFeatures {
        AudioDspFeatures {
            track_id: 0,
            bpm: Some(bpm),
            key_signature: None,
            camelot_key: Some(key.to_string()),
            loudness_lufs: None,
            energy: None,
            danceability: None,
            beat_strength: None,
            spectral_centroid: None,
            stereo_width: None,
            is_instrumental: false,
            analysis_source: "test".to_string(),
            analysis_offset_ms: 0,
            samples_analyzed: None,
            analyzed_at: "2026-01-01T00:00:00Z".to_string(),
            analysis_version: "test".to_string(),
        }
    }

    #[test]
    fn score_prefers_inside_cap_bpm_and_compatible_camelot() {
        let seed = DjQueueFacts {
            features: Some(features(124.0, "8A")),
            ..DjQueueFacts::default()
        };
        let compatible = DjQueueFacts {
            features: Some(features(126.0, "8A")),
            profile_confidence: Some(0.8),
            phrase_count: 16,
            drop_count: 1,
            ..DjQueueFacts::default()
        };
        let clash = DjQueueFacts {
            features: Some(features(145.0, "3B")),
            profile_confidence: Some(0.8),
            ..DjQueueFacts::default()
        };

        let (compatible_score, compatible_reasons) = score_facts(&seed, &compatible);
        let (clash_score, clash_reasons) = score_facts(&seed, &clash);

        assert!(compatible_score > clash_score);
        assert!(compatible_reasons.contains(&"tempo inside 3 percent"));
        assert!(compatible_reasons.contains(&"harmonic match"));
        assert!(clash_reasons.contains(&"wide tempo"));
        assert!(clash_reasons.contains(&"key clash"));
    }

    #[test]
    fn score_keeps_missing_bpm_and_key_neutral() {
        let seed = DjQueueFacts {
            features: Some(features(124.0, "8A")),
            ..DjQueueFacts::default()
        };
        let missing = DjQueueFacts {
            profile_confidence: Some(0.8),
            ..DjQueueFacts::default()
        };
        let ready = DjQueueFacts {
            features: Some(features(124.5, "8A")),
            profile_confidence: Some(0.8),
            ..DjQueueFacts::default()
        };

        let (missing_score, missing_reasons) = score_facts(&seed, &missing);
        let (ready_score, _) = score_facts(&seed, &ready);

        assert!(missing_score > 1.0);
        assert!(ready_score > missing_score);
        assert!(!missing_reasons.contains(&"wide tempo"));
        assert!(!missing_reasons.contains(&"key clash"));
    }

    #[test]
    fn rank_generated_candidates_keeps_equal_scores_stable() {
        let conn = Connection::open_in_memory().expect("db");
        let ranked = rank_generated_candidates(
            &conn,
            1,
            vec![
                GeneratedCandidate {
                    item: "first",
                    track_id: None,
                    tidal_id: None,
                    policy: Default::default(),
                },
                GeneratedCandidate {
                    item: "second",
                    track_id: None,
                    tidal_id: None,
                    policy: Default::default(),
                },
            ],
            true,
        )
        .expect("rank");

        assert_eq!(ranked[0].item, "first");
        assert_eq!(ranked[1].item, "second");
    }

    #[test]
    fn rank_generated_candidates_uses_tidal_id_for_local_facts() {
        let conn = Connection::open_in_memory().expect("db");
        conn.execute_batch(
            "
            CREATE TABLE tracks (id INTEGER PRIMARY KEY, tidal_id INTEGER);
            CREATE TABLE audio_dsp_features (
                track_id INTEGER PRIMARY KEY,
                bpm REAL,
                key_signature TEXT,
                camelot_key TEXT,
                loudness_lufs REAL,
                energy REAL,
                danceability REAL,
                beat_strength REAL,
                spectral_centroid REAL,
                stereo_width REAL,
                is_instrumental INTEGER NOT NULL DEFAULT 0,
                analysis_source TEXT NOT NULL DEFAULT 'test',
                analysis_offset_ms INTEGER NOT NULL DEFAULT 0,
                samples_analyzed INTEGER,
                analyzed_at TEXT NOT NULL DEFAULT '2026-01-01T00:00:00Z',
                analysis_version TEXT NOT NULL DEFAULT 'test'
            );
            ",
        )
        .expect("schema");
        conn.execute(
            "INSERT INTO tracks (id, tidal_id) VALUES (2, 9002), (3, 9003)",
            [],
        )
        .expect("tracks");
        for (track_id, bpm, key) in [(1, 124.0, "8A"), (2, 125.0, "8A"), (3, 145.0, "3B")] {
            conn.execute(
                "INSERT INTO audio_dsp_features (track_id, bpm, camelot_key) VALUES (?1, ?2, ?3)",
                params![track_id, bpm, key],
            )
            .expect("features");
        }

        let ranked = rank_generated_candidates(
            &conn,
            1,
            vec![
                GeneratedCandidate {
                    item: "clash",
                    track_id: Some(3),
                    tidal_id: Some(9003),
                    policy: Default::default(),
                },
                GeneratedCandidate {
                    item: "tidal-fit",
                    track_id: None,
                    tidal_id: Some(9002),
                    policy: Default::default(),
                },
            ],
            true,
        )
        .expect("rank");

        assert_eq!(ranked[0].item, "tidal-fit");
        assert!(ranked[0].reasons.contains(&"tempo inside 3 percent"));
        assert!(ranked[0].reasons.contains(&"harmonic match"));
    }

    #[test]
    fn rank_generated_candidates_chain_scores_each_pick_against_previous_pick() {
        let conn = Connection::open_in_memory().expect("db");
        conn.execute_batch(
            "
            CREATE TABLE tracks (id INTEGER PRIMARY KEY, tidal_id INTEGER);
            CREATE TABLE audio_dsp_features (
                track_id INTEGER PRIMARY KEY,
                bpm REAL,
                key_signature TEXT,
                camelot_key TEXT,
                loudness_lufs REAL,
                energy REAL,
                danceability REAL,
                beat_strength REAL,
                spectral_centroid REAL,
                stereo_width REAL,
                is_instrumental INTEGER NOT NULL DEFAULT 0,
                analysis_source TEXT NOT NULL DEFAULT 'test',
                analysis_offset_ms INTEGER NOT NULL DEFAULT 0,
                samples_analyzed INTEGER,
                analyzed_at TEXT NOT NULL DEFAULT '2026-01-01T00:00:00Z',
                analysis_version TEXT NOT NULL DEFAULT 'test'
            );
            ",
        )
        .expect("schema");
        for (track_id, bpm, key) in [
            (1, 120.0, "1A"),
            (2, 121.0, "2A"),
            (3, 122.0, "6A"),
            (4, 122.0, "3A"),
        ] {
            conn.execute(
                "INSERT INTO audio_dsp_features (track_id, bpm, camelot_key) VALUES (?1, ?2, ?3)",
                params![track_id, bpm, key],
            )
            .expect("features");
        }

        let ranked = rank_generated_candidates_chain(
            &conn,
            1,
            vec![
                GeneratedCandidate {
                    item: "first-fit",
                    track_id: Some(2),
                    tidal_id: None,
                    policy: Default::default(),
                },
                GeneratedCandidate {
                    item: "seed-favored-clash-after-first",
                    track_id: Some(3),
                    tidal_id: None,
                    policy: Default::default(),
                },
                GeneratedCandidate {
                    item: "chain-fit",
                    track_id: Some(4),
                    tidal_id: None,
                    policy: Default::default(),
                },
            ],
            true,
        )
        .expect("rank");

        assert_eq!(
            ranked.into_iter().map(|row| row.item).collect::<Vec<_>>(),
            vec!["first-fit", "chain-fit", "seed-favored-clash-after-first"]
        );
    }

    #[test]
    fn rank_generated_candidates_chain_keeps_equal_scores_stable() {
        let conn = Connection::open_in_memory().expect("db");
        let ranked = rank_generated_candidates_chain(
            &conn,
            1,
            vec![
                GeneratedCandidate {
                    item: "first",
                    track_id: None,
                    tidal_id: None,
                    policy: Default::default(),
                },
                GeneratedCandidate {
                    item: "second",
                    track_id: None,
                    tidal_id: None,
                    policy: Default::default(),
                },
                GeneratedCandidate {
                    item: "third",
                    track_id: None,
                    tidal_id: None,
                    policy: Default::default(),
                },
            ],
            true,
        )
        .expect("rank");

        assert_eq!(
            ranked.into_iter().map(|row| row.item).collect::<Vec<_>>(),
            vec!["first", "second", "third"]
        );
    }

    fn dsp_conn(rows: &[(i64, f64, &str)]) -> Connection {
        let conn = Connection::open_in_memory().expect("db");
        conn.execute_batch(
            "
            CREATE TABLE tracks (id INTEGER PRIMARY KEY, tidal_id INTEGER);
            CREATE TABLE audio_dsp_features (
                track_id INTEGER PRIMARY KEY,
                bpm REAL,
                key_signature TEXT,
                camelot_key TEXT,
                loudness_lufs REAL,
                energy REAL,
                danceability REAL,
                beat_strength REAL,
                spectral_centroid REAL,
                stereo_width REAL,
                is_instrumental INTEGER NOT NULL DEFAULT 0,
                analysis_source TEXT NOT NULL DEFAULT 'test',
                analysis_offset_ms INTEGER NOT NULL DEFAULT 0,
                samples_analyzed INTEGER,
                analyzed_at TEXT NOT NULL DEFAULT '2026-01-01T00:00:00Z',
                analysis_version TEXT NOT NULL DEFAULT 'test'
            );
            ",
        )
        .expect("schema");
        for (track_id, bpm, key) in rows {
            conn.execute(
                "INSERT INTO audio_dsp_features (track_id, bpm, camelot_key) VALUES (?1, ?2, ?3)",
                params![track_id, bpm, key],
            )
            .expect("features");
        }
        conn
    }

    fn candidate(item: &'static str, track_id: i64) -> GeneratedCandidate<&'static str> {
        GeneratedCandidate {
            item,
            track_id: Some(track_id),
            tidal_id: None,
            policy: Default::default(),
        }
    }

    #[test]
    fn without_mixing_relevance_order_stands_and_no_dj_reasons() {
        // Track 3 clashes with the seed, track 2 fits it perfectly; with
        // mixing off the caller's relevance order wins anyway.
        let conn = dsp_conn(&[(1, 124.0, "8A"), (2, 125.0, "8A"), (3, 145.0, "3B")]);
        let ranked = rank_generated_candidates(
            &conn,
            1,
            vec![candidate("clash", 3), candidate("fit", 2)],
            false,
        )
        .expect("rank");
        assert_eq!(
            ranked.iter().map(|row| row.item).collect::<Vec<_>>(),
            vec!["clash", "fit"]
        );
        assert!(ranked.iter().all(|row| row.reasons.is_empty()));
    }

    #[test]
    fn while_mixing_a_strong_fit_moves_up_only_a_few_places() {
        // Rank 0 clashes with the seed; rank 10 is a perfect fit; the rest
        // have no analysis (neutral). The fit lifts rank 10 but cannot take
        // it past clearly more relevant candidates.
        let conn = dsp_conn(&[(1, 124.0, "8A"), (100, 145.0, "3B"), (110, 124.0, "8A")]);
        let items: [&'static str; 11] = [
            "r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r9", "r10",
        ];
        let candidates = items
            .iter()
            .enumerate()
            .map(|(rank, item)| candidate(item, 100 + rank as i64))
            .collect::<Vec<_>>();
        let ranked = rank_generated_candidates(&conn, 1, candidates, true).expect("rank");
        let position = |item: &str| ranked.iter().position(|row| row.item == item).unwrap();
        assert!(position("r10") < 10, "the fit should help");
        assert!(position("r10") >= 5, "but not jump the top of the list");
        assert!(position("r0") <= 2, "a clash costs rank 0 at most a little");
    }
}
