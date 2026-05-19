use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const EXTERNAL_BONUS: f64 = 0.15;
const LIBRARY_PENALTY: f64 = 0.12;
const COVERAGE_BONUS_MAX: f64 = 0.25;
const CONFIDENCE_BONUS_MAX: f64 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendSeedKind {
    Library,
    Tidal,
    Pending,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlendSeedInput {
    pub kind: BlendSeedKind,
    #[serde(default)]
    pub track_id: Option<i64>,
    #[serde(default)]
    pub tidal_id: Option<i64>,
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub weight: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BlendSeed {
    pub kind: BlendSeedKind,
    pub identity: String,
    pub track_id: Option<i64>,
    pub tidal_id: Option<i64>,
    pub artist: Option<String>,
    pub title: Option<String>,
    pub weight: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SeedScore {
    pub seed_identity: String,
    pub seed_track_id: Option<i64>,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlendCandidateInput {
    pub identity: String,
    pub title: String,
    pub artist_name: String,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub track_id: Option<i64>,
    pub tidal_id: Option<i64>,
    pub is_in_library: bool,
    pub confidence: f64,
    pub source: String,
    pub reason_tags: Vec<String>,
    pub per_seed_scores: Vec<(i64, f64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateRole {
    Seed,
    ExternalCandidate,
    LibraryGuide,
    Route,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Playability {
    Playable,
    Resolvable,
    Pending,
    #[allow(dead_code)]
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScoredBlendCandidate {
    pub identity: String,
    pub title: String,
    pub artist_name: String,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub track_id: Option<i64>,
    pub tidal_id: Option<i64>,
    pub is_in_library: bool,
    pub source: String,
    pub reason_tags: Vec<String>,
    pub role: CandidateRole,
    pub playability: Playability,
    pub per_seed_scores: Vec<SeedScore>,
    pub covered_seed_count: usize,
    pub weighted_seed_proximity: f64,
    pub coverage_bonus: f64,
    pub external_bonus: f64,
    pub library_penalty: f64,
    pub confidence_bonus: f64,
    pub blend_score: f64,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlendSeedError {
    Empty,
    TooMany,
    InvalidIdentity,
    Duplicate,
}

pub fn validate_and_normalize_seeds(
    seeds: &[BlendSeedInput],
) -> Result<Vec<BlendSeed>, BlendSeedError> {
    if seeds.is_empty() {
        return Err(BlendSeedError::Empty);
    }
    if seeds.len() > 4 {
        return Err(BlendSeedError::TooMany);
    }

    let use_equal_weights = seeds.iter().any(|seed| {
        seed.weight
            .is_none_or(|weight| !weight.is_finite() || weight <= 0.0)
    });
    let total_weight = seeds
        .iter()
        .filter_map(|seed| seed.weight)
        .filter(|weight| weight.is_finite() && *weight > 0.0)
        .sum::<f64>();
    let equal_weight = 1.0 / seeds.len() as f64;

    let mut seen = HashSet::new();
    let mut normalized = Vec::with_capacity(seeds.len());
    for seed in seeds {
        let identity = seed_identity(seed)?;
        if !seen.insert(identity.clone()) {
            return Err(BlendSeedError::Duplicate);
        }
        let weight = if use_equal_weights || total_weight <= 0.0 {
            equal_weight
        } else {
            seed.weight.unwrap_or(0.0) / total_weight
        };
        normalized.push(BlendSeed {
            kind: seed.kind,
            identity,
            track_id: seed.track_id.filter(|id| *id > 0),
            tidal_id: seed.tidal_id.filter(|id| *id > 0),
            artist: seed.artist.clone(),
            title: seed.title.clone(),
            weight,
        });
    }

    Ok(normalized)
}

fn seed_identity(seed: &BlendSeedInput) -> Result<String, BlendSeedError> {
    match seed.kind {
        BlendSeedKind::Library => seed
            .track_id
            .filter(|id| *id > 0)
            .map(|id| format!("library:{id}"))
            .ok_or(BlendSeedError::InvalidIdentity),
        BlendSeedKind::Tidal => seed
            .tidal_id
            .filter(|id| *id > 0)
            .map(|id| format!("tidal:{id}"))
            .ok_or(BlendSeedError::InvalidIdentity),
        BlendSeedKind::Pending => {
            let artist = seed.artist.as_deref().unwrap_or("").trim();
            let title = seed.title.as_deref().unwrap_or("").trim();
            if artist.is_empty() || title.is_empty() {
                return Err(BlendSeedError::InvalidIdentity);
            }
            Ok(format!(
                "pending:{}:{}",
                normalize_identity_part(artist),
                normalize_identity_part(title)
            ))
        }
    }
}

fn normalize_identity_part(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn score_blend_candidate(
    candidate: &BlendCandidateInput,
    seeds: &[BlendSeed],
) -> ScoredBlendCandidate {
    let mut per_seed_scores = Vec::new();
    let mut covered_seed_count = 0usize;
    let mut weighted_seed_proximity = 0.0;

    for seed in seeds {
        let score = seed
            .track_id
            .and_then(|track_id| {
                candidate
                    .per_seed_scores
                    .iter()
                    .find(|(seed_id, _)| *seed_id == track_id)
                    .map(|(_, score)| score.clamp(0.0, 1.0))
            })
            .unwrap_or(0.0);
        if score > 0.0 {
            covered_seed_count += 1;
        }
        weighted_seed_proximity += seed.weight * score;
        per_seed_scores.push(SeedScore {
            seed_identity: seed.identity.clone(),
            seed_track_id: seed.track_id,
            score,
        });
    }

    let coverage_ratio = if seeds.is_empty() {
        0.0
    } else {
        covered_seed_count as f64 / seeds.len() as f64
    };
    let coverage_bonus = coverage_ratio * COVERAGE_BONUS_MAX;
    let external_bonus = if candidate.is_in_library {
        0.0
    } else {
        EXTERNAL_BONUS
    };
    let library_penalty = if candidate.is_in_library {
        LIBRARY_PENALTY
    } else {
        0.0
    };
    let confidence_bonus = candidate.confidence.clamp(0.0, 1.0) * CONFIDENCE_BONUS_MAX;
    let blend_score = weighted_seed_proximity + coverage_bonus + external_bonus + confidence_bonus
        - library_penalty;

    ScoredBlendCandidate {
        identity: candidate.identity.clone(),
        title: candidate.title.clone(),
        artist_name: candidate.artist_name.clone(),
        album_title: candidate.album_title.clone(),
        artwork_url: candidate.artwork_url.clone(),
        duration_ms: candidate.duration_ms,
        track_id: candidate.track_id,
        tidal_id: candidate.tidal_id,
        is_in_library: candidate.is_in_library,
        source: candidate.source.clone(),
        reason_tags: candidate.reason_tags.clone(),
        role: if candidate.is_in_library {
            CandidateRole::LibraryGuide
        } else {
            CandidateRole::ExternalCandidate
        },
        playability: if candidate.is_in_library {
            Playability::Playable
        } else if candidate.tidal_id.is_some_and(|id| id > 0) {
            Playability::Resolvable
        } else {
            Playability::Pending
        },
        per_seed_scores,
        covered_seed_count,
        weighted_seed_proximity,
        coverage_bonus,
        external_bonus,
        library_penalty,
        confidence_bonus,
        blend_score,
        x: 0.0,
        y: 0.0,
    }
}

pub fn apply_library_guide_cap(
    candidates: &mut Vec<ScoredBlendCandidate>,
    cap_ratio: f64,
    sparse_external: bool,
) {
    candidates.sort_by(|a, b| {
        b.blend_score
            .partial_cmp(&a.blend_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    if sparse_external {
        return;
    }

    let external_count = candidates
        .iter()
        .filter(|candidate| candidate.role == CandidateRole::ExternalCandidate)
        .count();
    let allowed_libraries = ((external_count as f64 * cap_ratio) / (1.0 - cap_ratio))
        .floor()
        .max(1.0) as usize;

    let mut kept_libraries = 0usize;
    candidates.retain(|candidate| {
        if candidate.role != CandidateRole::LibraryGuide {
            return true;
        }
        kept_libraries += 1;
        kept_libraries <= allowed_libraries
    });
}

#[cfg(test)]
fn scored_result(identity: &str, role: CandidateRole, blend_score: f64) -> ScoredBlendCandidate {
    ScoredBlendCandidate {
        identity: identity.to_string(),
        title: identity.to_string(),
        artist_name: "Artist".to_string(),
        album_title: None,
        artwork_url: None,
        duration_ms: None,
        track_id: if role == CandidateRole::LibraryGuide {
            Some(1)
        } else {
            None
        },
        tidal_id: if role == CandidateRole::ExternalCandidate {
            Some(900)
        } else {
            None
        },
        is_in_library: role == CandidateRole::LibraryGuide,
        source: "test".to_string(),
        reason_tags: vec![],
        role,
        playability: Playability::Playable,
        per_seed_scores: vec![],
        covered_seed_count: 0,
        weighted_seed_proximity: 0.0,
        coverage_bonus: 0.0,
        external_bonus: 0.0,
        library_penalty: 0.0,
        confidence_bonus: 0.0,
        blend_score,
        x: 0.0,
        y: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library_seed(track_id: i64, weight: Option<f64>) -> BlendSeedInput {
        BlendSeedInput {
            kind: BlendSeedKind::Library,
            track_id: Some(track_id),
            tidal_id: None,
            artist: None,
            title: None,
            weight,
        }
    }

    fn candidate(
        identity: &str,
        is_in_library: bool,
        per_seed_scores: Vec<(i64, f64)>,
    ) -> BlendCandidateInput {
        BlendCandidateInput {
            identity: identity.to_string(),
            title: identity.to_string(),
            artist_name: "Artist".to_string(),
            album_title: None,
            artwork_url: None,
            duration_ms: None,
            track_id: if is_in_library { Some(100) } else { None },
            tidal_id: if is_in_library { None } else { Some(900) },
            is_in_library,
            confidence: 0.8,
            source: "test".to_string(),
            reason_tags: vec!["external_match".to_string()],
            per_seed_scores,
        }
    }

    #[test]
    fn validate_seeds_rejects_empty_duplicate_and_too_many() {
        assert!(validate_and_normalize_seeds(&[]).is_err());
        assert!(
            validate_and_normalize_seeds(&[library_seed(1, None), library_seed(1, None),]).is_err()
        );
        assert!(
            validate_and_normalize_seeds(&[
                library_seed(1, None),
                library_seed(2, None),
                library_seed(3, None),
                library_seed(4, None),
                library_seed(5, None),
            ])
            .is_err()
        );
    }

    #[test]
    fn validate_seeds_normalizes_invalid_weights_to_equal_weights() {
        let seeds = validate_and_normalize_seeds(&[
            library_seed(10, Some(-1.0)),
            library_seed(20, Some(0.0)),
            library_seed(30, None),
        ])
        .expect("valid seeds");

        assert_eq!(seeds.len(), 3);
        assert!((seeds[0].weight - 1.0 / 3.0).abs() < 0.0001);
        assert!((seeds[1].weight - 1.0 / 3.0).abs() < 0.0001);
        assert!((seeds[2].weight - 1.0 / 3.0).abs() < 0.0001);
    }

    #[test]
    fn coverage_bonus_prefers_candidate_connected_to_multiple_balanced_seeds() {
        let seeds =
            validate_and_normalize_seeds(&[library_seed(1, Some(0.5)), library_seed(2, Some(0.5))])
                .expect("valid seeds");
        let single_seed =
            score_blend_candidate(&candidate("single", false, vec![(1, 0.92)]), &seeds);
        let covered = score_blend_candidate(
            &candidate("covered", false, vec![(1, 0.62), (2, 0.62)]),
            &seeds,
        );

        assert!(covered.blend_score > single_seed.blend_score);
        assert_eq!(covered.covered_seed_count, 2);
    }

    #[test]
    fn biased_weight_can_prefer_strong_anchor_candidate() {
        let seeds =
            validate_and_normalize_seeds(&[library_seed(1, Some(0.8)), library_seed(2, Some(0.2))])
                .expect("valid seeds");
        let strong_anchor =
            score_blend_candidate(&candidate("strong", false, vec![(1, 0.95)]), &seeds);
        let weak_covered = score_blend_candidate(
            &candidate("weak", false, vec![(1, 0.45), (2, 0.45)]),
            &seeds,
        );

        assert!(strong_anchor.blend_score > weak_covered.blend_score);
    }

    #[test]
    fn external_bonus_and_library_penalty_promote_external_candidate() {
        let seeds =
            validate_and_normalize_seeds(&[library_seed(1, Some(0.5)), library_seed(2, Some(0.5))])
                .expect("valid seeds");
        let external = score_blend_candidate(
            &candidate("external", false, vec![(1, 0.7), (2, 0.7)]),
            &seeds,
        );
        let library = score_blend_candidate(
            &candidate("library", true, vec![(1, 0.7), (2, 0.7)]),
            &seeds,
        );

        assert!(external.blend_score > library.blend_score);
        assert!(external.external_bonus > 0.0);
        assert!(library.library_penalty > 0.0);
    }

    #[test]
    fn library_guide_cap_keeps_external_candidates_primary() {
        let mut scored = vec![
            scored_result("ext-1", CandidateRole::ExternalCandidate, 0.9),
            scored_result("ext-2", CandidateRole::ExternalCandidate, 0.8),
            scored_result("lib-1", CandidateRole::LibraryGuide, 0.95),
            scored_result("lib-2", CandidateRole::LibraryGuide, 0.94),
            scored_result("lib-3", CandidateRole::LibraryGuide, 0.93),
        ];

        apply_library_guide_cap(&mut scored, 0.25, false);

        let libraries = scored
            .iter()
            .filter(|candidate| candidate.role == CandidateRole::LibraryGuide)
            .count();
        assert_eq!(libraries, 1);
    }
}
