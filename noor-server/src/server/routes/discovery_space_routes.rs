use crate::db::queries;
use crate::services::discovery_blend as blend;
use crate::services::discovery_ranking as ranking;
use crate::services::discovery_space as ds;
use crate::smart::discovery as discovery_engine;
use crate::{AppEvent, SharedState};
use axum::{extract::State, http::StatusCode, response::Json};
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::{
    build_radio_queue_and_spawn_resolvers, internal, spawn_pending_resolvers_for_queue_items,
    start_first_radio_queue_item,
};

/// Stable synthetic id for an external (Last.fm) candidate that has no resolved
/// Tidal id. Negative i64 keyed off `artist|title` so multiple unresolved hits
/// don't all collapse onto the same `track-0` node on the canvas. Hash collisions
/// are negligible at the ~60-candidate scale of a single radio request.
///
/// TODO(option 2): Replace this with real Tidal-search resolution in `radio.rs`
/// before the candidate leaves the orchestrator - that would also let
/// `DiscoverSidePanel.resolveExternalPlayable` go away. Needs an artist+title ->
/// tidal_id cache (in-memory or a small SQLite table) to avoid hammering the
/// Tidal API on every discovery request.
fn synthetic_external_track_id(artist: &str, title: &str) -> i64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    artist.hash(&mut h);
    "|".hash(&mut h);
    title.hash(&mut h);
    // Force into the negative i64 range so it can't collide with library ids
    // (always positive) or with the legacy `0` placeholder.
    -(((h.finish() & 0x7FFF_FFFF_FFFF_FFFF) | 1) as i64)
}

#[derive(Debug, Deserialize)]
pub(super) struct DiscoverySpaceRequest {
    mode: Option<String>,
    seed_track_id: Option<i64>,
    prompt: Option<String>,
    limit: Option<i64>,
    /// Coherence-vs-diversity control, 0..1. Omitted -> 0.5, which maps to the
    /// historical behavior (Mixed radio blend, near-identity shaping).
    coherence: Option<f64>,
    /// Client listening-session id; enables the exclude-heard filter (and,
    /// later, session-taste reranking) without any server session state.
    session_id: Option<String>,
    #[serde(default)]
    filters: Option<ranking::SpaceFilters>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DiscoveryBlendRequest {
    seeds: Vec<blend::BlendSeedInput>,
    limit: Option<i64>,
}

fn blend_seed_error_response(error: blend::BlendSeedError) -> (StatusCode, Json<Value>) {
    let message = match error {
        blend::BlendSeedError::Empty => "at least one blend seed is required",
        blend::BlendSeedError::TooMany => "blend supports up to four seeds",
        blend::BlendSeedError::InvalidIdentity => "blend seed is missing a valid identity",
        blend::BlendSeedError::Duplicate => "duplicate blend seeds are not allowed",
    };
    (StatusCode::BAD_REQUEST, Json(json!({ "error": message })))
}

fn parse_discovery_reason_tags(reason_json: Option<&str>, fallback: &str) -> Vec<String> {
    let raw_tags = reason_json
        .and_then(|raw| serde_json::from_str::<Vec<Value>>(raw).ok())
        .unwrap_or_default()
        .iter()
        .filter_map(|value| {
            value
                .get("key")
                .and_then(|key| key.as_str())
                .or_else(|| value.get("label").and_then(|label| label.as_str()))
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    let mut reason_tags = ds::normalize_reason_tags(&raw_tags);
    if reason_tags.is_empty() {
        reason_tags.push(ds::normalize_reason(fallback).to_string());
    }
    reason_tags
}

fn blend_anchor(index: usize, count: usize) -> (f64, f64) {
    match count {
        0 | 1 => (0.0, 0.0),
        2 => {
            if index == 0 {
                (-260.0, 0.0)
            } else {
                (260.0, 0.0)
            }
        }
        _ => {
            let angle = (index as f64 / count as f64) * std::f64::consts::PI * 2.0
                - std::f64::consts::FRAC_PI_2;
            (angle.cos() * 280.0, angle.sin() * 220.0)
        }
    }
}

fn layout_blend_candidate(
    candidate: &mut blend::ScoredBlendCandidate,
    seed_count: usize,
    index: usize,
) {
    if candidate.role == blend::CandidateRole::Seed {
        return;
    }
    if seed_count == 2 {
        let left = candidate
            .per_seed_scores
            .first()
            .map(|score| score.score)
            .unwrap_or(0.0);
        let right = candidate
            .per_seed_scores
            .get(1)
            .map(|score| score.score)
            .unwrap_or(0.0);
        let total = (left + right).max(0.001);
        let t = right / total;
        candidate.x = -260.0 + t * 520.0;
        candidate.y = ((index as f64 * 37.0).sin() * 90.0)
            + (1.0 - candidate.blend_score.clamp(0.0, 1.0)) * 120.0;
        return;
    }

    let mut x = 0.0;
    let mut y = 0.0;
    let mut total = 0.0;
    for (seed_index, seed_score) in candidate.per_seed_scores.iter().enumerate() {
        let (anchor_x, anchor_y) = blend_anchor(seed_index, seed_count);
        let weight = seed_score.score.max(0.0);
        x += anchor_x * weight;
        y += anchor_y * weight;
        total += weight;
    }
    if total > 0.0 {
        candidate.x = x / total + (index as f64 * 19.0).sin() * 45.0;
        candidate.y = y / total + (index as f64 * 23.0).cos() * 45.0;
    } else {
        let angle = (index as f64 / (seed_count.max(1) as f64)) * std::f64::consts::PI * 2.0;
        candidate.x = angle.cos() * 180.0;
        candidate.y = angle.sin() * 140.0;
    }
}

fn resolve_external_blend_seed_anchor(
    conn: &rusqlite::Connection,
    seed: &blend::BlendSeed,
    model_id: Option<i64>,
) -> rusqlite::Result<Option<i64>> {
    if let Some(tidal_id) = seed.tidal_id.filter(|id| *id > 0) {
        if let Some(track_id) = conn
            .query_row(
                "SELECT id FROM tracks WHERE tidal_id = ?1 LIMIT 1",
                params![tidal_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        {
            return Ok(Some(track_id));
        }

        return conn
            .query_row(
                "SELECT COALESCE(c.resolved_track_id, n.library_track_id)
                 FROM external_track_candidates c
                 LEFT JOIN external_track_candidate_neighbors n
                   ON n.candidate_id = c.id
                  AND (?2 IS NULL OR n.model_id = ?2)
                 WHERE c.tidal_id = ?1
                   AND COALESCE(c.resolved_track_id, n.library_track_id) IS NOT NULL
                 ORDER BY
                   CASE WHEN c.resolved_track_id IS NOT NULL THEN 0 ELSE 1 END,
                   n.score DESC,
                   n.rank ASC
                 LIMIT 1",
                params![tidal_id, model_id],
                |row| row.get::<_, i64>(0),
            )
            .optional();
    }

    let artist = seed.artist.as_deref().unwrap_or("").trim();
    let title = seed.title.as_deref().unwrap_or("").trim();
    if artist.is_empty() || title.is_empty() {
        return Ok(None);
    }

    conn.query_row(
        "SELECT COALESCE(c.resolved_track_id, n.library_track_id)
         FROM external_track_candidates c
         LEFT JOIN external_track_candidate_neighbors n
           ON n.candidate_id = c.id
          AND (?3 IS NULL OR n.model_id = ?3)
         WHERE lower(trim(c.title)) = lower(trim(?1))
           AND lower(trim(c.artist_name)) = lower(trim(?2))
           AND COALESCE(c.resolved_track_id, n.library_track_id) IS NOT NULL
         ORDER BY
           CASE WHEN c.resolved_track_id IS NOT NULL THEN 0 ELSE 1 END,
           n.score DESC,
           n.rank ASC
         LIMIT 1",
        params![title, artist, model_id],
        |row| row.get::<_, i64>(0),
    )
    .optional()
}

fn resolve_blend_seed_anchors(
    conn: &rusqlite::Connection,
    seeds: &mut [blend::BlendSeed],
    model_id: Option<i64>,
) -> rusqlite::Result<()> {
    for seed in seeds {
        if seed.kind == blend::BlendSeedKind::Library {
            seed.anchor_track_id = seed.track_id;
        } else if seed.anchor_track_id.is_none() {
            seed.anchor_track_id = resolve_external_blend_seed_anchor(conn, seed, model_id)?;
        }
    }
    Ok(())
}

fn seed_node_from_track(
    seed: &blend::BlendSeed,
    index: usize,
    count: usize,
    row: (
        i64,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
    ),
) -> blend::ScoredBlendCandidate {
    let (x, y) = blend_anchor(index, count);
    blend::ScoredBlendCandidate {
        identity: seed.identity.clone(),
        title: row.1,
        artist_name: row.2.unwrap_or_default(),
        album_title: row.3,
        artwork_url: row.4,
        duration_ms: row.5,
        track_id: Some(row.0),
        tidal_id: seed.tidal_id,
        is_in_library: true,
        source: "library".to_string(),
        reason_tags: vec!["seed".to_string()],
        role: blend::CandidateRole::Seed,
        playability: blend::Playability::Playable,
        per_seed_scores: vec![blend::SeedScore {
            seed_identity: seed.identity.clone(),
            seed_track_id: seed.anchor_track_id.or(seed.track_id),
            score: 1.0,
        }],
        covered_seed_count: 1,
        weighted_seed_proximity: 1.0,
        coverage_bonus: 0.0,
        external_bonus: 0.0,
        library_penalty: 0.0,
        confidence_bonus: 0.0,
        blend_score: 1.0,
        x,
        y,
    }
}

fn seed_node_from_external_seed(
    seed: &blend::BlendSeed,
    index: usize,
    count: usize,
) -> blend::ScoredBlendCandidate {
    let (x, y) = blend_anchor(index, count);
    blend::ScoredBlendCandidate {
        identity: seed.identity.clone(),
        title: seed
            .title
            .clone()
            .unwrap_or_else(|| "TIDAL seed".to_string()),
        artist_name: seed.artist.clone().unwrap_or_default(),
        album_title: None,
        artwork_url: None,
        duration_ms: None,
        track_id: None,
        tidal_id: seed.tidal_id,
        is_in_library: false,
        source: "external".to_string(),
        reason_tags: vec!["seed".to_string()],
        role: blend::CandidateRole::Seed,
        playability: if seed.tidal_id.is_some() {
            blend::Playability::Resolvable
        } else {
            blend::Playability::Pending
        },
        per_seed_scores: vec![blend::SeedScore {
            seed_identity: seed.identity.clone(),
            seed_track_id: seed.anchor_track_id.or(seed.track_id),
            score: 1.0,
        }],
        covered_seed_count: 1,
        weighted_seed_proximity: 1.0,
        coverage_bonus: 0.0,
        external_bonus: 0.0,
        library_penalty: 0.0,
        confidence_bonus: 0.0,
        blend_score: 1.0,
        x,
        y,
    }
}

fn build_discovery_blend_space(
    conn: &rusqlite::Connection,
    seeds: &mut [blend::BlendSeed],
    limit: i64,
    library_cap_ratio: f64,
) -> anyhow::Result<(Vec<blend::ScoredBlendCandidate>, Value)> {
    let model = queries::get_selected_discovery_embedding_model(conn)?;
    resolve_blend_seed_anchors(conn, seeds, model.as_ref().map(|model| model.id))?;
    let seed_count = seeds.len();
    let library_seed_ids = seeds
        .iter()
        .filter_map(|seed| seed.anchor_track_id.or(seed.track_id))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let mut seed_nodes = Vec::new();
    for (index, seed) in seeds.iter().enumerate() {
        if seed.kind == blend::BlendSeedKind::Library && seed.track_id.is_some() {
            let track_id = seed.track_id.unwrap_or_default();
            let row = conn
                .query_row(
                    "SELECT t.id, t.title, ar.name, al.title, al.artwork_url, t.duration_ms
                     FROM tracks t
                     LEFT JOIN artists ar ON t.artist_id = ar.id
                     LEFT JOIN albums al ON t.album_id = al.id
                     WHERE t.id = ?1",
                    params![track_id],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, Option<i64>>(5)?,
                        ))
                    },
                )
                .optional()?;
            if let Some(row) = row {
                seed_nodes.push(seed_node_from_track(seed, index, seed_count, row));
            }
        } else {
            seed_nodes.push(seed_node_from_external_seed(seed, index, seed_count));
        }
    }

    let mut candidate_inputs: HashMap<String, blend::BlendCandidateInput> = HashMap::new();
    if let Some(model) = model {
        let per_seed_limit = limit.max(1).min(200);
        let seed_id_set = library_seed_ids.iter().copied().collect::<HashSet<_>>();
        let seed_identity_set = seeds
            .iter()
            .map(|seed| seed.identity.clone())
            .collect::<HashSet<_>>();
        let library_neighbors = queries::get_track_neighbors_for_seeds(
            conn,
            model.id,
            &library_seed_ids,
            per_seed_limit,
        )?;
        for (seed_id, rows) in library_neighbors {
            for row in rows {
                if seed_id_set.contains(&row.track_id) {
                    continue;
                }
                let identity = format!("library:{}", row.track_id);
                if seed_identity_set.contains(&identity) {
                    continue;
                }
                let reason_tags =
                    parse_discovery_reason_tags(row.reason_json.as_deref(), "library_match");
                let entry = candidate_inputs.entry(identity.clone()).or_insert_with(|| {
                    blend::BlendCandidateInput {
                        identity: identity.clone(),
                        title: row.title.clone(),
                        artist_name: row.artist_name.clone().unwrap_or_default(),
                        album_title: row.album_title.clone(),
                        artwork_url: row.artwork_url.clone(),
                        duration_ms: row.duration_ms,
                        track_id: Some(row.track_id),
                        tidal_id: None,
                        is_in_library: true,
                        confidence: row.confidence,
                        source: "library".to_string(),
                        reason_tags: reason_tags.clone(),
                        per_seed_scores: Vec::new(),
                    }
                });
                entry.per_seed_scores.push((seed_id, row.score));
            }
        }

        for seed_id in &library_seed_ids {
            let rows = queries::get_external_candidate_neighbors(
                conn,
                model.id,
                *seed_id,
                per_seed_limit,
                false,
            )?;
            for row in rows {
                let identity = row
                    .tidal_id
                    .filter(|id| *id > 0)
                    .map(|id| format!("tidal:{id}"))
                    .unwrap_or_else(|| blend::pending_seed_identity(&row.artist_name, &row.title));
                if seed_identity_set.contains(&identity) {
                    continue;
                }
                let reason_tags =
                    parse_discovery_reason_tags(row.reason_json.as_deref(), "external_match");
                let entry = candidate_inputs.entry(identity.clone()).or_insert_with(|| {
                    blend::BlendCandidateInput {
                        identity: identity.clone(),
                        title: row.title.clone(),
                        artist_name: row.artist_name.clone(),
                        album_title: None,
                        artwork_url: None,
                        duration_ms: row.duration_ms,
                        track_id: None,
                        tidal_id: row.tidal_id,
                        is_in_library: false,
                        confidence: 0.7,
                        source: "external".to_string(),
                        reason_tags: reason_tags.clone(),
                        per_seed_scores: Vec::new(),
                    }
                });
                entry.per_seed_scores.push((*seed_id, row.score));
            }
        }
    }

    let mut candidates = candidate_inputs
        .values()
        .map(|candidate| blend::score_blend_candidate(candidate, seeds))
        .collect::<Vec<_>>();
    let resolvable_external_count = candidates
        .iter()
        .filter(|candidate| {
            candidate.role == blend::CandidateRole::ExternalCandidate
                && candidate.playability == blend::Playability::Resolvable
        })
        .count();
    blend::apply_library_guide_cap(
        &mut candidates,
        library_cap_ratio,
        resolvable_external_count < 3,
    );
    for (index, candidate) in candidates.iter_mut().enumerate() {
        layout_blend_candidate(candidate, seed_count, index);
    }
    candidates.truncate(limit as usize);

    let playable_external_count = candidates
        .iter()
        .filter(|candidate| {
            candidate.role == blend::CandidateRole::ExternalCandidate
                && matches!(
                    candidate.playability,
                    blend::Playability::Playable | blend::Playability::Resolvable
                )
        })
        .count();
    let pending_external_count = candidates
        .iter()
        .filter(|candidate| {
            candidate.role == blend::CandidateRole::ExternalCandidate
                && candidate.playability == blend::Playability::Pending
        })
        .count();
    let library_guide_count = candidates
        .iter()
        .filter(|candidate| candidate.role == blend::CandidateRole::LibraryGuide)
        .count();
    let coverage_ratio = if candidates.is_empty() || seed_count == 0 {
        0.0
    } else {
        candidates
            .iter()
            .map(|candidate| candidate.covered_seed_count as f64 / seed_count as f64)
            .sum::<f64>()
            / candidates.len() as f64
    };
    let health = json!({
        "playable_external_count": playable_external_count,
        "pending_external_count": pending_external_count,
        "library_guide_count": library_guide_count,
        "coverage_ratio": coverage_ratio,
    });

    seed_nodes.append(&mut candidates);
    Ok((seed_nodes, health))
}

fn blend_node_json(candidate: &blend::ScoredBlendCandidate) -> Value {
    let track_id = candidate.track_id.unwrap_or_else(|| {
        candidate.tidal_id.unwrap_or_else(|| {
            synthetic_external_track_id(&candidate.artist_name, &candidate.title)
        })
    });
    let score = candidate.blend_score.clamp(0.0, 1.0);
    json!({
        "id": format!("track-{track_id}"),
        "track_id": track_id,
        "title": candidate.title,
        "artist_name": candidate.artist_name,
        "album_title": candidate.album_title,
        "artwork_url": candidate.artwork_url,
        "duration_ms": candidate.duration_ms,
        "similarity_score": score,
        "score": score,
        "raw_score": candidate.weighted_seed_proximity,
        "source": candidate.source,
        "is_in_library": candidate.is_in_library,
        "role": candidate.role,
        "playability": candidate.playability,
        "reason_tags": candidate.reason_tags,
        "primary_reason": candidate.reason_tags.first().cloned().unwrap_or_else(|| "blend".to_string()),
        "per_seed_scores": candidate.per_seed_scores,
        "coverage_bonus": candidate.coverage_bonus,
        "external_bonus": candidate.external_bonus,
        "library_penalty": candidate.library_penalty,
        "final_blend_score": candidate.blend_score,
        "is_seed": candidate.role == blend::CandidateRole::Seed,
        "x": candidate.x,
        "y": candidate.y,
        "vx": 0.0,
        "vy": 0.0,
        "radius": if candidate.role == blend::CandidateRole::Seed { 20.0 } else { 8.0 + score * 16.0 },
        "opacity": 0.0,
        "layout": {
            "x": candidate.x,
            "y": candidate.y,
            "radius_hint": if candidate.role == blend::CandidateRole::Seed { 20.0 } else { 8.0 + score * 16.0 },
            "distance_from_seed": (1.0 - score).clamp(0.0, 1.0),
        },
    })
}

fn blend_edge_json(
    seed: &blend::ScoredBlendCandidate,
    candidate: &blend::ScoredBlendCandidate,
) -> Vec<Value> {
    let from_track_id = seed.track_id.unwrap_or_else(|| {
        seed.tidal_id
            .unwrap_or_else(|| synthetic_external_track_id(&seed.artist_name, &seed.title))
    });
    let to_track_id = candidate.track_id.unwrap_or_else(|| {
        candidate.tidal_id.unwrap_or_else(|| {
            synthetic_external_track_id(&candidate.artist_name, &candidate.title)
        })
    });
    candidate
        .per_seed_scores
        .iter()
        .filter(|score| score.seed_identity == seed.identity && score.score > 0.0)
        .map(|score| {
            json!({
                "id": format!("blend-{from_track_id}-{to_track_id}"),
                "from_id": from_track_id,
                "to_id": to_track_id,
                "from_track_id": from_track_id,
                "to_track_id": to_track_id,
                "type": "blend",
                "reason": "blend",
                "primary_reason": "blend",
                "reason_tags": ["blend"],
                "weight": score.score,
                "confidence": 0.7,
                "source": "blend",
                "support_count": null,
                "behavioral_score": 0.0,
                "audio_score": score.score,
                "metadata_score": 0.0,
            })
        })
        .collect()
}

fn blend_candidates_to_radio(
    candidates: &[blend::ScoredBlendCandidate],
) -> Vec<crate::services::radio::RadioCandidate> {
    candidates
        .iter()
        .filter(|candidate| {
            candidate.role != blend::CandidateRole::Seed
                && matches!(
                    candidate.playability,
                    blend::Playability::Playable | blend::Playability::Resolvable
                )
        })
        .map(|candidate| crate::services::radio::RadioCandidate {
            track_id: candidate
                .track_id
                .or(candidate.tidal_id)
                .unwrap_or_default(),
            tidal_track_id: candidate.tidal_id,
            title: candidate.title.clone(),
            artist_name: candidate.artist_name.clone(),
            album_title: candidate.album_title.clone(),
            artwork_url: candidate.artwork_url.clone(),
            duration_ms: candidate.duration_ms,
            isrc: None,
            is_in_library: candidate.is_in_library,
            source: if candidate.is_in_library {
                crate::services::radio::RadioSource::Library
            } else {
                crate::services::radio::RadioSource::Engine
            },
            reason: "Blend discovery".to_string(),
            similarity_score: candidate.blend_score.clamp(0.0, 1.0),
            confidence: Some(candidate.blend_score.clamp(0.0, 1.0)),
            candidate_in_degree_percentile: None,
            support_count: Some(candidate.covered_seed_count as i64),
            primary_reason: Some(
                candidate
                    .reason_tags
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "blend".to_string()),
            ),
        })
        .collect()
}

pub(super) async fn get_discovery_blend_space(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryBlendRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut seeds =
        blend::validate_and_normalize_seeds(&payload.seeds).map_err(blend_seed_error_response)?;
    let limit = payload.limit.unwrap_or(60).max(1).min(200);
    let db = {
        let state_guard = state.read().await;
        state_guard.db.clone()
    };
    let (candidates, health) = db
        .with_conn(|conn| build_discovery_blend_space(conn, &mut seeds, limit, 0.25))
        .map_err(internal)?;
    let seed_nodes = candidates
        .iter()
        .filter(|candidate| candidate.role == blend::CandidateRole::Seed)
        .collect::<Vec<_>>();
    let tracks = candidates.iter().map(blend_node_json).collect::<Vec<_>>();
    let mut edges = Vec::new();
    for seed in seed_nodes {
        for candidate in candidates
            .iter()
            .filter(|candidate| candidate.role != blend::CandidateRole::Seed)
        {
            edges.extend(blend_edge_json(seed, candidate));
        }
    }

    Ok(Json(json!({
        "tracks": tracks,
        "edges": edges,
        "artists": [],
        "blend_seeds": seeds,
        "health": health,
        "diagnostics": {
            "node_count": tracks.len(),
            "edge_count": edges.len(),
            "source_counts": {},
            "reason_counts": {},
        },
        "generated_at": chrono::Utc::now().to_rfc3339(),
    })))
}

pub(super) async fn add_discovery_blend_to_queue(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryBlendRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut seeds =
        blend::validate_and_normalize_seeds(&payload.seeds).map_err(blend_seed_error_response)?;
    let limit = payload.limit.unwrap_or(60).max(1).min(200);
    let db = {
        let state_guard = state.read().await;
        state_guard.db.clone()
    };
    let (candidates, health) = db
        .with_conn(|conn| build_discovery_blend_space(conn, &mut seeds, limit, 0.15))
        .map_err(internal)?;
    let tracks = blend_candidates_to_radio(&candidates);
    if tracks.is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "blend has no playable discoveries" })),
        ));
    }
    let build = db
        .with_conn(move |conn| {
            Ok(crate::server::radio_pipeline::append_radio_queue_from_candidates(conn, tracks)?)
        })
        .map_err(internal)?;
    let pending_item_ids = build.pending_item_ids;
    let pending_count = pending_item_ids.len();
    spawn_pending_resolvers_for_queue_items(&state, &db, pending_item_ids, "blend_add").await;
    {
        let state_guard = state.read().await;
        let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
    }
    Ok(Json(json!({
        "first_playable": match build.first_item {
            Some((queue_item_id, Some(track_id))) => json!({
                "type": "library",
                "queue_item_id": queue_item_id,
                "track_id": track_id,
            }),
            Some((queue_item_id, None)) => json!({
                "type": "pending",
                "queue_item_id": queue_item_id,
                "track_id": null,
            }),
            None => json!(null),
        },
        "pending_count": pending_count,
        "health": health,
    })))
}

pub(super) async fn play_discovery_blend(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryBlendRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    play_discovery_blend_inner(state, payload, "blend_play").await
}

pub(super) async fn make_discovery_blend_radio(
    State(state): State<SharedState>,
    Json(mut payload): Json<DiscoveryBlendRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    payload.limit = Some(payload.limit.unwrap_or(200).max(120).min(200));
    play_discovery_blend_inner(state, payload, "blend_radio").await
}

async fn play_discovery_blend_inner(
    state: SharedState,
    payload: DiscoveryBlendRequest,
    context: &'static str,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut seeds =
        blend::validate_and_normalize_seeds(&payload.seeds).map_err(blend_seed_error_response)?;
    let limit = payload.limit.unwrap_or(60).max(1).min(200);
    let db = {
        let state_guard = state.read().await;
        state_guard.db.clone()
    };
    let (candidates, health) = db
        .with_conn(|conn| build_discovery_blend_space(conn, &mut seeds, limit, 0.15))
        .map_err(internal)?;
    let tracks = blend_candidates_to_radio(&candidates);
    if tracks.is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "blend has no playable discoveries" })),
        ));
    }
    let (first_playable, pending_count) =
        build_radio_queue_and_spawn_resolvers(&state, &db, None, tracks, context).await?;
    let snapshot = start_first_radio_queue_item(&state).await?;
    Ok(Json(json!({
        "first_playable": first_playable,
        "pending_count": pending_count,
        "health": health,
        "state": snapshot.state,
        "queue": snapshot.queue,
    })))
}

pub(super) async fn get_discovery_space(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoverySpaceRequest>,
) -> Result<Json<Value>, StatusCode> {
    let mode = payload.mode.unwrap_or_else(|| "radio".to_string());
    let limit = payload.limit.unwrap_or(60).max(1).min(200);
    let seed_id = payload.seed_track_id.unwrap_or(0);
    let prompt = payload.prompt.as_deref().unwrap_or("").trim().to_string();
    let coherence = payload.coherence.unwrap_or(0.5).clamp(0.0, 1.0);
    let rank_params = ranking::RankParams::from_coherence(coherence);
    let filters = payload.filters.unwrap_or_default();
    let session_id = payload.session_id;

    let mut state_guard = state.read().await;

    #[derive(Debug)]
    struct SpaceTrack {
        track_id: i64,
        title: String,
        artist_name: String,
        album_title: Option<String>,
        artwork_url: Option<String>,
        duration_ms: Option<i64>,
        similarity_score: f64,
        source: String,
        energy: Option<f64>,
        danceability: Option<f64>,
        bpm: Option<f64>,
        key_signature: Option<String>,
        camelot_key: Option<String>,
        is_instrumental: Option<bool>,
        loudness_lufs: Option<f64>,
        skip_rate: Option<f64>,
        completion_avg: Option<f64>,
        cohort_id: Option<String>,
        cohort_label: Option<String>,
        top_genre: Option<String>,
        top_genre_source: Option<String>,
        top_genre_confidence: Option<f64>,
        last_played_at: Option<String>,
        play_count: i64,
        is_in_library: bool,
        radio_source: Option<String>, // "library" | "lastfm" | "engine"
        radio_reason: Option<String>,
        // v1.5 fields
        confidence: f64,
        support_count: i64,
        primary_reason: String,
        reason_tags: Vec<String>,
        genres: Vec<String>,
        in_degree_pctile: f64,
    }

    // -- 1. Decide track set based on inputs ----------------------------------
    //
    //   prompt set   -> rank_candidates (text/genre/affinity scoring)
    //   seed_id set  -> radio_from_neighbors (embedding graph)
    //   neither      -> most-played fallback

    let mut space_tracks: Vec<SpaceTrack> = if !prompt.is_empty() {
        // Prompt path: run the full discovery scoring engine against the library
        let p = prompt.clone();
        let lim = limit;
        state_guard
            .db
            .with_conn(move |conn| {
                let request = discovery_engine::DiscoveryPreviewRequest {
                    prompt: p.clone(),
                    mode: "mood".to_string(),
                    services: vec!["tidal".to_string()],
                    limit: lim as usize,
                };
                let context = discovery_engine::DiscoveryContext {
                    overview: queries::get_analytics_overview(conn)?,
                    behavior: queries::get_behavior_metrics(conn)?,
                    recent_listens: queries::get_recent_listens(conn, 12)?,
                    top_artists: queries::get_top_artists_by_history(conn, 6)?,
                    top_genres: queries::get_top_genres_by_history(conn, 6)?,
                    track_genres: queries::get_track_genre_paths_with_fallback(conn)?
                        .into_iter()
                        .map(|(id, rows)| (id, queries::ResolvedGenre::paths_only(&rows)))
                        .collect(),
                };
                let candidates = queries::get_discovery_candidate_tracks(conn, lim * 4)?;
                let preview = discovery_engine::build_preview(&request, &context, &candidates);
                Ok(preview
                    .results
                    .into_iter()
                    .map(|r| SpaceTrack {
                        track_id: r.track_id,
                        title: r.title,
                        artist_name: r.artist_name.as_deref().unwrap_or("").to_string(),
                        album_title: r.album_title,
                        artwork_url: r.artwork_url,
                        duration_ms: r.duration_ms,
                        similarity_score: (r.score as f64 / 99.0).clamp(0.0, 1.0),
                        source: r.service,
                        energy: None,
                        danceability: None,
                        bpm: None,
                        key_signature: None,
                        camelot_key: None,
                        is_instrumental: None,
                        loudness_lufs: None,
                        skip_rate: None,
                        completion_avg: None,
                        cohort_id: None,
                        cohort_label: None,
                        top_genre: None,
                        top_genre_source: None,
                        top_genre_confidence: None,
                        last_played_at: None,
                        play_count: 0,
                        is_in_library: true,
                        radio_source: None,
                        radio_reason: None,
                        confidence: 1.0,
                        support_count: 0,
                        primary_reason: "unknown".to_string(),
                        reason_tags: vec![],
                        genres: vec![],
                        in_degree_pctile: 0.5,
                    })
                    .collect::<Vec<_>>())
            })
            .unwrap_or_default()
    } else if seed_id > 0 {
        let db = state_guard.db.clone();
        let lastfm = crate::metadata::lastfm::LastFmClient::load(
            state_guard.http_client.clone(),
            &state_guard.db,
        );
        let lastfm_similar_cache = state_guard.lastfm_similar_cache.clone();
        drop(state_guard);

        // Coherence picks the candidate-generation band; only this call site
        // changes, the radio endpoints keep their own blend selection.
        let radio_blend = match ranking::blend_band(coherence) {
            ranking::BlendBand::Coherent => crate::services::radio::RadioBlend::Familiar,
            ranking::BlendBand::Balanced => crate::services::radio::RadioBlend::Mixed,
            ranking::BlendBand::Diverse => crate::services::radio::RadioBlend::Adventurous,
        };
        let queue = crate::services::radio::orchestrate_song(
            &db,
            lastfm.as_ref(),
            Some(&lastfm_similar_cache),
            seed_id,
            radio_blend,
            limit as usize,
            &[],
        )
        .await
        .ok();

        state_guard = state.read().await;

        if let Some(queue) = queue {
            queue
                .tracks
                .into_iter()
                .map(|c| SpaceTrack {
                    track_id: if c.is_in_library {
                        c.track_id
                    } else {
                        c.tidal_track_id.unwrap_or_else(|| {
                            synthetic_external_track_id(&c.artist_name, &c.title)
                        })
                    },
                    title: c.title,
                    artist_name: c.artist_name,
                    album_title: c.album_title,
                    artwork_url: c.artwork_url,
                    duration_ms: c.duration_ms,
                    similarity_score: c.similarity_score,
                    source: match c.source {
                        crate::services::radio::RadioSource::Library => "tidal".to_string(),
                        _ => "external".to_string(),
                    },
                    energy: None,
                    danceability: None,
                    bpm: None,
                    key_signature: None,
                    camelot_key: None,
                    is_instrumental: None,
                    loudness_lufs: None,
                    skip_rate: None,
                    completion_avg: None,
                    cohort_id: None,
                    cohort_label: None,
                    top_genre: None,
                    top_genre_source: None,
                    top_genre_confidence: None,
                    last_played_at: None,
                    play_count: 0,
                    is_in_library: c.is_in_library,
                    radio_source: Some(match c.source {
                        crate::services::radio::RadioSource::Library => "library".to_string(),
                        crate::services::radio::RadioSource::Lastfm => "lastfm".to_string(),
                        crate::services::radio::RadioSource::Engine => "engine".to_string(),
                    }),
                    radio_reason: Some(c.reason),
                    confidence: c
                        .confidence
                        .unwrap_or(if c.is_in_library { 1.0 } else { 0.5 }),
                    support_count: c.support_count.unwrap_or(0),
                    primary_reason: ds::normalize_reason(c.primary_reason.as_deref().unwrap_or(""))
                        .to_string(),
                    reason_tags: c
                        .primary_reason
                        .as_deref()
                        .map(|r| vec![ds::normalize_reason(r).to_string()])
                        .unwrap_or_default(),
                    genres: vec![],
                    in_degree_pctile: c.candidate_in_degree_percentile.unwrap_or(0.5),
                })
                .collect()
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    // -- 1b. Prepend the seed track itself when in seed mode (so canvas has center) --
    if seed_id > 0 && prompt.is_empty() {
        // Avoid duplicating if it somehow ended up in the candidate list.
        let already_present = space_tracks.iter().any(|t| t.track_id == seed_id);
        if !already_present {
            let seed_track_opt: Option<(i64, String, Option<String>, Option<String>, Option<String>, Option<i64>, Option<String>)> =
                state_guard.db.with_conn(|conn| {
                    Ok(conn.query_row(
                        "SELECT t.id, t.title, ar.name, al.title, al.artwork_url, t.duration_ms, t.source
                         FROM tracks t
                         LEFT JOIN artists ar ON t.artist_id = ar.id
                         LEFT JOIN albums al ON t.album_id = al.id
                         WHERE t.id = ?1",
                        rusqlite::params![seed_id],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, Option<String>>(2)?,
                                row.get::<_, Option<String>>(3)?,
                                row.get::<_, Option<String>>(4)?,
                                row.get::<_, Option<i64>>(5)?,
                                row.get::<_, Option<String>>(6)?,
                            ))
                        },
                    ).ok())
                }).unwrap_or(None);

            if let Some((id, title, artist, album, artwork, dur, src)) = seed_track_opt {
                space_tracks.insert(
                    0,
                    SpaceTrack {
                        track_id: id,
                        title,
                        artist_name: artist.unwrap_or_default(),
                        album_title: album,
                        artwork_url: artwork,
                        duration_ms: dur,
                        similarity_score: 1.0,
                        source: src.unwrap_or_else(|| "tidal".to_string()),
                        energy: None,
                        danceability: None,
                        bpm: None,
                        key_signature: None,
                        camelot_key: None,
                        is_instrumental: None,
                        loudness_lufs: None,
                        skip_rate: None,
                        completion_avg: None,
                        cohort_id: None,
                        cohort_label: None,
                        top_genre: None,
                        top_genre_source: None,
                        top_genre_confidence: None,
                        last_played_at: None,
                        play_count: 0,
                        is_in_library: true,
                        radio_source: None,
                        radio_reason: None,
                        confidence: 1.0,
                        support_count: 0,
                        primary_reason: "unknown".to_string(),
                        reason_tags: vec![],
                        genres: vec![],
                        in_degree_pctile: 0.5,
                    },
                );
            }
        }
    }

    // -- 2. Fill remainder from most-played library tracks --------------------
    // Only fill when browsing without a seed. In seed mode the radio candidates
    // ARE the map - padding with unrelated most-played tracks creates a cloud of
    // disconnected blue dots with no edges and falsely-cold-start labels.
    if seed_id > 0 && prompt.is_empty() && (space_tracks.len() as i64) < limit {
        let remaining = limit - space_tracks.len() as i64;
        let external_rows = state_guard
            .db
            .with_conn(|conn| {
                let Some(model) = queries::get_selected_discovery_embedding_model(conn)? else {
                    return Ok(Vec::new());
                };
                queries::get_external_candidate_neighbors(conn, model.id, seed_id, remaining, true)
            })
            .unwrap_or_default();
        let mut present_ids = space_tracks
            .iter()
            .map(|track| track.track_id)
            .collect::<HashSet<_>>();
        for row in external_rows {
            let Some(tidal_id) = row.tidal_id.filter(|id| *id > 0) else {
                continue;
            };
            if !present_ids.insert(tidal_id) {
                continue;
            }
            let raw_tags = row
                .reason_json
                .as_deref()
                .and_then(|raw| serde_json::from_str::<Vec<Value>>(raw).ok())
                .unwrap_or_default()
                .iter()
                .filter_map(|value| {
                    value
                        .get("key")
                        .and_then(|key| key.as_str())
                        .or_else(|| value.get("label").and_then(|label| label.as_str()))
                        .map(str::to_string)
                })
                .collect::<Vec<_>>();
            let mut reason_tags = ds::normalize_reason_tags(&raw_tags);
            if reason_tags.is_empty() {
                reason_tags.push(ds::normalize_reason("external_match").to_string());
            }
            let primary_reason = reason_tags
                .first()
                .cloned()
                .unwrap_or_else(|| ds::normalize_reason("external_match").to_string());

            space_tracks.push(SpaceTrack {
                track_id: tidal_id,
                title: row.title,
                artist_name: row.artist_name,
                album_title: None,
                artwork_url: None,
                duration_ms: row.duration_ms,
                similarity_score: row.score.clamp(0.0, 1.0),
                source: "external".to_string(),
                energy: None,
                danceability: None,
                bpm: None,
                key_signature: None,
                camelot_key: None,
                is_instrumental: None,
                loudness_lufs: None,
                skip_rate: None,
                completion_avg: None,
                cohort_id: None,
                cohort_label: None,
                top_genre: None,
                top_genre_source: None,
                top_genre_confidence: None,
                last_played_at: None,
                play_count: 0,
                is_in_library: false,
                radio_source: Some("engine".to_string()),
                radio_reason: Some("external_match".to_string()),
                confidence: 0.7,
                support_count: 1,
                primary_reason,
                reason_tags,
                genres: vec![],
                in_degree_pctile: 0.5,
            });
        }
    }

    let seeded_ids: HashSet<i64> = space_tracks.iter().map(|t| t.track_id).collect();
    let remaining = limit - space_tracks.len() as i64;
    if remaining > 0 && seed_id == 0 {
        let fallback = state_guard
            .db
            .with_conn(|conn| {
                let mut stmt = conn.prepare(
                "SELECT t.id, t.title, a.name, al.title, al.artwork_url, t.duration_ms, t.source
                 FROM tracks t
                 LEFT JOIN artists a ON t.artist_id = a.id
                 LEFT JOIN albums al ON t.album_id = al.id
                 ORDER BY t.play_count DESC, t.date_added DESC
                 LIMIT ?1",
            )?;
                let rows = stmt.query_map([limit], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                })?;
                let mut result = Vec::new();
                for r in rows {
                    result.push(r?);
                }
                Ok(result)
            })
            .unwrap_or_default();

        for (id, title, artist, album, artwork, dur, src) in fallback {
            if !seeded_ids.contains(&id) {
                space_tracks.push(SpaceTrack {
                    track_id: id,
                    title,
                    artist_name: artist.unwrap_or_default(),
                    album_title: album,
                    artwork_url: artwork,
                    duration_ms: dur,
                    similarity_score: 0.5,
                    source: src.unwrap_or_else(|| "tidal".to_string()),
                    energy: None,
                    danceability: None,
                    bpm: None,
                    key_signature: None,
                    camelot_key: None,
                    is_instrumental: None,
                    loudness_lufs: None,
                    skip_rate: None,
                    completion_avg: None,
                    cohort_id: None,
                    cohort_label: None,
                    top_genre: None,
                    top_genre_source: None,
                    top_genre_confidence: None,
                    last_played_at: None,
                    play_count: 0,
                    is_in_library: true,
                    radio_source: None,
                    radio_reason: None,
                    confidence: 1.0,
                    support_count: 0,
                    primary_reason: "unknown".to_string(),
                    reason_tags: vec![],
                    genres: vec![],
                    in_degree_pctile: 0.5,
                });
            }
        }
        space_tracks.truncate(limit as usize);
    }

    // -- 3. Fetch DSP features for all collected track IDs --------------------
    if !space_tracks.is_empty() {
        let ids_csv: String = space_tracks
            .iter()
            .filter(|t| t.is_in_library)
            .map(|t| t.track_id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        if ids_csv.is_empty() {
            // No library tracks present (pure external response) - nothing to enrich.
        } else {
            type DspRow = (
                Option<f64>,    // energy
                Option<f64>,    // danceability
                Option<f64>,    // bpm
                Option<String>, // key_signature
                Option<String>, // camelot_key
                Option<i64>,    // is_instrumental (0/1)
                Option<f64>,    // loudness_lufs
            );
            let dsp_map: std::collections::HashMap<i64, DspRow> = state_guard
                .db
                .with_conn(|conn| {
                    let sql = format!(
                        "SELECT track_id, energy, danceability, bpm, key_signature, camelot_key,
                            is_instrumental, loudness_lufs
                     FROM audio_dsp_features WHERE track_id IN ({ids_csv})"
                    );
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map([], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, Option<f64>>(1)?,
                            row.get::<_, Option<f64>>(2)?,
                            row.get::<_, Option<f64>>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, Option<String>>(5)?,
                            row.get::<_, Option<i64>>(6)?,
                            row.get::<_, Option<f64>>(7)?,
                        ))
                    })?;
                    let mut map = std::collections::HashMap::new();
                    for r in rows {
                        let (id, energy, dance, bpm, key, camelot, instr, lufs) = r?;
                        map.insert(id, (energy, dance, bpm, key, camelot, instr, lufs));
                    }
                    Ok(map)
                })
                .unwrap_or_default();

            for t in &mut space_tracks {
                if let Some((energy, dance, bpm, key, camelot, instr, lufs)) =
                    dsp_map.get(&t.track_id)
                {
                    t.energy = *energy;
                    t.danceability = *dance;
                    t.bpm = *bpm;
                    t.key_signature = key.clone();
                    t.camelot_key = camelot.clone();
                    t.is_instrumental = instr.map(|v| v != 0);
                    t.loudness_lufs = *lufs;
                }
            }
        }
    }

    // -- 3b. Aggregate skip-rate + completion-avg from listen_history ---------
    if !space_tracks.is_empty() {
        let ids_csv: String = space_tracks
            .iter()
            .filter(|t| t.is_in_library)
            .map(|t| t.track_id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        if ids_csv.is_empty() {
            // No library tracks present (pure external response) - nothing to enrich.
        } else {
            let listen_map: std::collections::HashMap<i64, (Option<f64>, Option<f64>)> = state_guard.db.with_conn(|conn| {
                let sql = format!(
                    "SELECT lh.track_id,
                            AVG(CASE WHEN lh.completed = 1 THEN 0.0 ELSE 1.0 END) AS skip_rate,
                            AVG(
                                CASE
                                    WHEN t.duration_ms IS NULL OR t.duration_ms = 0 THEN NULL
                                    ELSE MIN(1.0, CAST(lh.duration_listened_ms AS REAL) / CAST(t.duration_ms AS REAL))
                                END
                            ) AS completion_avg
                     FROM listen_history lh
                     JOIN tracks t ON t.id = lh.track_id
                     WHERE lh.track_id IN ({ids_csv})
                     GROUP BY lh.track_id"
                );
                let mut stmt = conn.prepare(&sql)?;
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<f64>>(1)?,
                        row.get::<_, Option<f64>>(2)?,
                    ))
                })?;
                let mut map = std::collections::HashMap::new();
                for r in rows {
                    let (id, skip, comp) = r?;
                    map.insert(id, (skip, comp));
                }
                Ok(map)
            }).unwrap_or_default();

            for t in &mut space_tracks {
                if let Some((skip, comp)) = listen_map.get(&t.track_id) {
                    // Preserve Option semantics - None means "no listen data" (distinct from 0.0).
                    t.skip_rate = *skip;
                    t.completion_avg = *comp;
                }
            }
        }
    }

    // -- 3c. Backfill last_played_at + play_count from tracks table -----------
    if !space_tracks.is_empty() {
        let ids_csv: String = space_tracks
            .iter()
            .filter(|t| t.is_in_library)
            .map(|t| t.track_id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        if ids_csv.is_empty() {
            // No library tracks present (pure external response) - nothing to enrich.
        } else {
            let track_meta: std::collections::HashMap<i64, (Option<String>, i64)> = state_guard
                .db
                .with_conn(|conn| {
                    let sql = format!(
                        "SELECT id, last_played_at, play_count FROM tracks WHERE id IN ({ids_csv})"
                    );
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map([], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                        ))
                    })?;
                    let mut map = std::collections::HashMap::new();
                    for r in rows {
                        let (id, last, plays) = r?;
                        map.insert(id, (last, plays));
                    }
                    Ok(map)
                })
                .unwrap_or_default();

            for t in &mut space_tracks {
                if let Some((last, plays)) = track_meta.get(&t.track_id) {
                    t.last_played_at = last.clone();
                    t.play_count = *plays;
                }
            }
        }
    }

    // -- 3d. Top-genre with source + confidence (highest confidence per track) -
    if !space_tracks.is_empty() {
        let ids_csv: String = space_tracks
            .iter()
            .filter(|t| t.is_in_library)
            .map(|t| t.track_id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        if ids_csv.is_empty() {
            // No library tracks present (pure external response) - nothing to enrich.
        } else {
            // genre_map: track_id -> (top_name, top_source, top_conf, all_names)
            type GenreEntry = (String, Option<String>, Option<f64>, Vec<String>);
            let genre_map: std::collections::HashMap<i64, GenreEntry> = state_guard
                .db
                .with_conn(|conn| {
                    let sql = format!(
                        "SELECT tg.track_id, g.name, tg.source, tg.confidence
                     FROM track_genres tg
                     JOIN genres g ON g.id = tg.genre_id
                     WHERE tg.track_id IN ({ids_csv})
                     ORDER BY tg.track_id, COALESCE(tg.confidence, 0) DESC"
                    );
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map([], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<f64>>(3)?,
                        ))
                    })?;
                    let mut map: std::collections::HashMap<i64, GenreEntry> =
                        std::collections::HashMap::new();
                    for r in rows {
                        let (id, name, source, conf) = r?;
                        let entry = map
                            .entry(id)
                            .or_insert_with(|| (name.clone(), source.clone(), conf, vec![]));
                        entry.3.push(name);
                    }
                    Ok(map)
                })
                .unwrap_or_default();

            for t in &mut space_tracks {
                if let Some((name, source, conf, all_genres)) = genre_map.get(&t.track_id) {
                    t.top_genre = Some(name.clone());
                    t.top_genre_source = source.clone();
                    t.top_genre_confidence = *conf;
                    t.genres = all_genres.clone();
                }
            }
        }
    }

    // -- 3e. Cohort assignment per track (90-day window) ----------------------
    if !space_tracks.is_empty() {
        let track_ids: Vec<i64> = space_tracks
            .iter()
            .filter(|t| t.is_in_library)
            .map(|t| t.track_id)
            .collect();

        if track_ids.is_empty() {
            // No library tracks - skip cohort assignment.
        } else {
            let cohort_map: std::collections::HashMap<i64, (String, String)> = state_guard
                .db
                .with_conn(|conn| queries::get_track_cohort_assignments(conn, &track_ids, 90))
                .unwrap_or_default();

            for t in &mut space_tracks {
                if let Some((id, label)) = cohort_map.get(&t.track_id) {
                    t.cohort_id = Some(id.clone());
                    t.cohort_label = Some(label.clone());
                }
            }
        }
    }

    // -- 3f. Multi-signal shaping (seed mode) ----------------------------------
    // Shape each candidate's base score with the signals enriched above (genre
    // Jaccard, Camelot/BPM, energy, same-artist) so normalization and pruning
    // downstream rank on the blend, and derive the per-node "why related".
    // Prompt/browse paths have no seed to be coherent with, so they skip this
    // and keep their base scores untouched.
    let seed_camelot: Option<String> = if seed_id > 0 && prompt.is_empty() {
        space_tracks
            .iter()
            .find(|t| t.track_id == seed_id)
            .and_then(|t| t.camelot_key.clone())
    } else {
        None
    };
    let mut shaped_by_track: HashMap<i64, ranking::ShapedScore> = HashMap::new();
    if seed_id > 0 && prompt.is_empty() {
        // RadioCandidate carries no artist_id, so same-artist detection runs on
        // the lowercased name for library and external candidates alike.
        let seed_features = space_tracks
            .iter()
            .find(|t| t.track_id == seed_id)
            .map(|t| ranking::SeedFeatures {
                genre_set: crate::genre::jaccard::weighted_genre_set(&t.genres),
                camelot: t.camelot_key.clone(),
                bpm: t.bpm,
                energy: t.energy,
                artist_id: None,
                artist_name_lc: Some(t.artist_name.to_lowercase()).filter(|s| !s.is_empty()),
            })
            .unwrap_or_default();
        for t in space_tracks.iter().filter(|t| t.track_id != seed_id) {
            let cand = ranking::CandidateFeatures {
                track_id: t.track_id,
                is_in_library: t.is_in_library,
                source: ds::normalize_source(&t.source).to_string(),
                base_score: t.similarity_score,
                genre_set: crate::genre::jaccard::weighted_genre_set(&t.genres),
                camelot: t.camelot_key.clone(),
                bpm: t.bpm,
                energy: t.energy,
                artist_id: None,
                artist_name_lc: Some(t.artist_name.to_lowercase()).filter(|s| !s.is_empty()),
                covered_seed_count: 0,
            };
            shaped_by_track.insert(
                t.track_id,
                ranking::shape_score(&seed_features, &cand, &rank_params, None),
            );
        }
    }

    // -- 3g. User filters (seed exempt) ----------------------------------------
    let mut filter_dropped_count = 0usize;
    let mut era_filter_coverage: Option<f64> = None;
    if !filters.is_noop() {
        let era_active = filters.year_min.is_some() || filters.year_max.is_some();
        let year_map: HashMap<i64, i64> = if era_active {
            let candidate_ids: Vec<i64> = space_tracks
                .iter()
                .filter(|t| t.is_in_library && t.track_id != seed_id)
                .map(|t| t.track_id)
                .collect();
            state_guard
                .db
                .with_conn(|conn| queries::get_album_years_for_tracks(conn, &candidate_ids))
                .unwrap_or_default()
        } else {
            HashMap::new()
        };
        if era_active {
            let denom = space_tracks
                .iter()
                .filter(|t| t.track_id != seed_id)
                .count();
            era_filter_coverage = Some(if denom == 0 {
                0.0
            } else {
                year_map.len() as f64 / denom as f64
            });
        }
        let heard: HashSet<i64> = if filters.exclude_heard_session {
            state_guard
                .db
                .with_conn(|conn| queries::get_session_heard_track_ids(conn, session_id.as_deref()))
                .unwrap_or_default()
        } else {
            HashSet::new()
        };
        let before = space_tracks.len();
        space_tracks.retain(|t| {
            if t.track_id == seed_id {
                return true;
            }
            let cand = ranking::CandidateFeatures {
                track_id: t.track_id,
                is_in_library: t.is_in_library,
                base_score: t.similarity_score,
                camelot: t.camelot_key.clone(),
                bpm: t.bpm,
                energy: t.energy,
                ..Default::default()
            };
            ranking::passes_filters(
                &filters,
                &cand,
                seed_camelot.as_deref(),
                year_map.get(&t.track_id).copied(),
                heard.contains(&t.track_id),
            )
        });
        filter_dropped_count = before - space_tracks.len();
    }

    // -- 4. Build typed edges (v1.5) ------------------------------------------
    // Typed to feed the pruner and serialized after pruning. Old callers receive
    // extra fields they can ignore; all existing fields are preserved.
    struct FullEdge {
        from_track_id: i64,
        to_track_id: i64,
        weight: f64,
        confidence: f64,
        primary_reason: String,
        reason_tags: Vec<String>,
        source: String,
        support_count: Option<i64>,
        behavioral_score: f64,
        audio_score: f64,
        metadata_score: f64,
    }

    // Library<->library edges come from `track_neighbors`. We always run this
    // query when there's more than one library track in the result set so the
    // map shows the full neighbor graph, regardless of whether external tracks
    // are present.
    let mut typed_edges: Vec<FullEdge> = {
        let track_id_set: HashSet<i64> = space_tracks
            .iter()
            .filter(|t| t.is_in_library)
            .map(|t| t.track_id)
            .collect();
        if track_id_set.len() > 1 {
            let ids_csv: String = track_id_set
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            state_guard
                .db
                .with_conn(|conn| {
                    let sql = format!(
                        "SELECT n.track_id, n.neighbor_track_id, n.score,
                            n.behavioral_score, n.audio_score, n.metadata_score,
                            n.reason_json, n.confidence, n.support_count
                     FROM track_neighbors n
                     WHERE n.track_id IN ({ids_csv}) AND n.neighbor_track_id IN ({ids_csv})
                     ORDER BY n.score DESC
                     LIMIT 300"
                    );
                    let mut stmt = conn.prepare(&sql)?;
                    let rows = stmt.query_map([], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, f64>(2)?,
                            row.get::<_, f64>(3)?,
                            row.get::<_, f64>(4)?,
                            row.get::<_, f64>(5)?,
                            row.get::<_, Option<String>>(6)?,
                            row.get::<_, Option<f64>>(7)?,
                            row.get::<_, Option<i64>>(8)?,
                        ))
                    })?;
                    let mut result = Vec::new();
                    for r in rows {
                        result.push(r?);
                    }
                    Ok(result)
                })
                .unwrap_or_default()
                .into_iter()
                .map(
                    |(
                        from_id,
                        to_id,
                        score,
                        behavioral,
                        audio,
                        metadata,
                        reason_json,
                        confidence,
                        support_count,
                    )| {
                        let parsed: Vec<Value> = reason_json
                            .as_deref()
                            .and_then(|s| serde_json::from_str::<Vec<Value>>(s).ok())
                            .unwrap_or_default();
                        let raw_tags: Vec<String> = parsed
                            .iter()
                            .filter_map(|v| {
                                v.get("key")
                                    .and_then(|k| k.as_str())
                                    .or_else(|| v.get("label").and_then(|l| l.as_str()))
                                    .map(|s| s.to_string())
                            })
                            .collect();
                        let reason_tags = ds::normalize_reason_tags(&raw_tags);
                        let primary_reason = reason_tags
                            .first()
                            .cloned()
                            .unwrap_or_else(|| "unknown".to_string());
                        FullEdge {
                            from_track_id: from_id,
                            to_track_id: to_id,
                            weight: score.clamp(0.0, 1.0),
                            confidence: confidence.unwrap_or(0.5),
                            primary_reason,
                            reason_tags,
                            source: "library".to_string(),
                            support_count,
                            behavioral_score: behavioral,
                            audio_score: audio,
                            metadata_score: metadata,
                        }
                    },
                )
                .collect()
        } else {
            vec![]
        }
    };

    // External (non-library) tracks aren't in `track_neighbors`, so synthesize
    // a seed->external edge per external track. This runs alongside the library
    // edges above so users see both their library graph and the external links.
    if seed_id > 0 && prompt.is_empty() {
        for t in space_tracks.iter().filter(|t| !t.is_in_library) {
            let reason = ds::normalize_reason("external_match");
            typed_edges.push(FullEdge {
                from_track_id: seed_id,
                to_track_id: t.track_id,
                weight: t.similarity_score,
                confidence: t.confidence,
                primary_reason: reason.to_string(),
                reason_tags: vec![reason.to_string()],
                source: ds::normalize_source(&t.source).to_string(),
                support_count: Some(t.support_count),
                behavioral_score: 0.0,
                audio_score: 0.0,
                metadata_score: t.similarity_score,
            });
        }
    }

    // -- 5. Score normalization (per source group) -----------------------------
    let score_candidates: Vec<ds::ScoreCandidate> = space_tracks
        .iter()
        .map(|t| ds::ScoreCandidate {
            track_id: t.track_id,
            raw_score: shaped_by_track
                .get(&t.track_id)
                .map(|s| s.score)
                .unwrap_or(t.similarity_score),
            source: ds::normalize_source(&t.source).to_string(),
        })
        .collect();
    let norm_scores = ds::normalize_scores_by_source(&score_candidates);

    // -- 6. Within-set in-degree stats ----------------------------------------
    let prune_edges: Vec<ds::PruneEdge> = typed_edges
        .iter()
        .map(|e| ds::PruneEdge {
            from_track_id: e.from_track_id,
            to_track_id: e.to_track_id,
            weight: e.weight,
            confidence: e.confidence,
        })
        .collect();
    let track_ids_for_deg: Vec<i64> = space_tracks.iter().map(|t| t.track_id).collect();
    let in_deg_stats = ds::compute_in_degree_stats(&track_ids_for_deg, &prune_edges);
    for t in &mut space_tracks {
        if let Some((_, pctile)) = in_deg_stats.get(&t.track_id) {
            t.in_degree_pctile = *pctile;
        }
    }

    // -- 7. Graph pruning ------------------------------------------------------
    let prune_nodes: Vec<ds::PruneNode> = space_tracks
        .iter()
        .map(|t| ds::PruneNode {
            track_id: t.track_id,
            score: norm_scores.get(&t.track_id).copied().unwrap_or_else(|| {
                shaped_by_track
                    .get(&t.track_id)
                    .map(|s| s.score)
                    .unwrap_or(t.similarity_score)
                    .clamp(0.0, 1.0)
            }),
            is_seed: t.track_id == seed_id,
            primary_reason: t.primary_reason.clone(),
            in_degree_pctile: t.in_degree_pctile,
        })
        .collect();
    let prune_result = ds::prune_graph(
        prune_nodes,
        prune_edges,
        seed_id,
        &ds::PruneConfig::for_coherence(coherence),
    );
    let surviving_ids: HashSet<i64> = prune_result.node_ids.iter().copied().collect();

    // Filter space_tracks to survivors; preserve original order.
    space_tracks.retain(|t| surviving_ids.contains(&t.track_id));

    // -- 8. Serialize nodes with v1.5 fields ----------------------------------
    let total = space_tracks.len().max(1);
    let track_nodes: Vec<Value> = space_tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let norm_score = norm_scores.get(&t.track_id).copied().unwrap_or_else(|| {
                shaped_by_track
                    .get(&t.track_id)
                    .map(|s| s.score)
                    .unwrap_or(t.similarity_score)
                    .clamp(0.0, 1.0)
            });
            // Library tracks are only truly cold-start if confidence is very low -
            // support_count may be 0 simply because the neighbor table hasn't been
            // calculated yet, which doesn't mean there's no behavioral data.
            let is_cold_start = !t.is_in_library && (t.support_count == 0 || t.confidence < 0.3);
            let normalized_source = ds::normalize_source(&t.source);
            let cluster_key = t
                .genres
                .first()
                .or(t.top_genre.as_ref())
                .map(|s| s.as_str())
                .unwrap_or("unknown");

            let (x, y) = match mode.as_str() {
                "energy_arc" => {
                    let energy = t.energy.unwrap_or(0.5);
                    let jitter_x = (i as f64 * 17.3).sin() * 60.0;
                    let jitter_y = (i as f64 * 31.7).cos() * 200.0;
                    ((energy - 0.5) * 800.0 + jitter_x, jitter_y)
                }
                "harmonic" => {
                    if let Some(ref ck) = t.camelot_key {
                        let num = ck
                            .chars()
                            .take_while(|c| c.is_ascii_digit())
                            .collect::<String>()
                            .parse::<f64>()
                            .unwrap_or(1.0);
                        let is_a = ck.contains('A');
                        let angle = ((num - 1.0) / 12.0) * std::f64::consts::PI * 2.0
                            + if is_a { 0.0 } else { 0.26 };
                        let r = 200.0 + (i as f64 * 23.0).sin() * 80.0;
                        (angle.cos() * r, angle.sin() * r)
                    } else {
                        let angle = (i as f64 / total as f64) * std::f64::consts::PI * 2.0;
                        (angle.cos() * 350.0, angle.sin() * 350.0)
                    }
                }
                _ => {
                    let angle = (i as f64 / total as f64) * std::f64::consts::PI * 2.0;
                    let r =
                        80.0 + (1.0 - t.similarity_score) * 300.0 + (i as f64 * 37.0).sin() * 50.0;
                    (angle.cos() * r, angle.sin() * r)
                }
            };
            let node_radius = 5.0 + t.similarity_score * 20.0 + t.energy.unwrap_or(0.5) * 5.0;
            let in_deg = in_deg_stats
                .get(&t.track_id)
                .map(|(d, _)| *d as i64)
                .unwrap_or(0);
            let layout_obj = json!({
                "x": x, "y": y,
                "radius_hint": node_radius,
                "cluster_key": cluster_key,
                "distance_from_seed": (1.0 - norm_score).clamp(0.0, 1.0),
            });
            // Build node object in two halves to avoid json! macro recursion limit.
            let mut node_obj = json!({
                "track_id": t.track_id,
                "title": t.title,
                "artist_name": t.artist_name,
                "album_title": t.album_title,
                "artwork_url": t.artwork_url,
                "duration_ms": t.duration_ms,
                "similarity_score": t.similarity_score,
                "energy": t.energy,
                "danceability": t.danceability,
                "bpm": t.bpm,
                "key_signature": t.key_signature,
                "camelot_key": t.camelot_key,
                "is_instrumental": t.is_instrumental,
                "loudness_lufs": t.loudness_lufs,
                "skip_rate": t.skip_rate,
                "completion_avg": t.completion_avg,
                "cohort_id": t.cohort_id,
                "cohort_label": t.cohort_label,
                "top_genre": t.top_genre,
                "top_genre_source": t.top_genre_source,
                "top_genre_confidence": t.top_genre_confidence,
                "last_played_at": t.last_played_at,
                "play_count": t.play_count,
                "is_in_library": t.is_in_library,
                "source": normalized_source,
                "radio_source": t.radio_source,
                "radio_reason": t.radio_reason,
                "x": x, "y": y, "vx": 0.0, "vy": 0.0,
                "radius": node_radius,
                "opacity": 0.0,
            });
            let shaped = shaped_by_track.get(&t.track_id);
            let v15 = json!({
                "id": format!("track-{}", t.track_id),
                "score": norm_score,
                "raw_score": t.similarity_score,
                "shaped_score": shaped.map(|s| s.score),
                "why": shaped.map(|s| s.why.clone()).unwrap_or_default(),
                "why_signals": shaped.map(|s| s.why_signals.clone()).unwrap_or_default(),
                "confidence": t.confidence,
                "support_count": t.support_count,
                "is_cold_start": is_cold_start,
                "primary_reason": t.primary_reason,
                "reason_tags": t.reason_tags,
                "genres": t.genres,
                "is_seed": t.track_id == seed_id,
                "candidate_in_degree": in_deg,
                "candidate_in_degree_percentile": t.in_degree_pctile,
                "layout": layout_obj,
            });
            if let (Some(obj), Some(ext)) = (node_obj.as_object_mut(), v15.as_object()) {
                obj.extend(ext.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
            node_obj
        })
        .collect();

    // -- 9. Serialize edges with v1.5 fields ----------------------------------
    let edge_nodes: Vec<Value> = typed_edges
        .iter()
        .filter(|e| {
            surviving_ids.contains(&e.from_track_id) && surviving_ids.contains(&e.to_track_id)
        })
        .map(|e| {
            let edge_id = format!("{}-{}-{}", e.from_track_id, e.to_track_id, e.primary_reason);
            json!({
                // -- Existing fields --
                "from_id": e.from_track_id,
                "to_id": e.to_track_id,
                "type": &e.primary_reason,
                "weight": e.weight,
                "reason_tags": &e.reason_tags,
                "behavioral_score": e.behavioral_score,
                "audio_score": e.audio_score,
                "metadata_score": e.metadata_score,
                // -- v1.5 fields --
                "id": edge_id,
                "from_track_id": e.from_track_id,
                "to_track_id": e.to_track_id,
                "reason": &e.primary_reason,
                "primary_reason": &e.primary_reason,
                "confidence": e.confidence,
                "source": &e.source,
                "support_count": e.support_count,
            })
        })
        .collect();

    // -- 10. Diagnostics -------------------------------------------------------
    let mut source_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut reason_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut conf_sum = 0.0f64;
    let mut pctile_sum = 0.0f64;
    for node in &track_nodes {
        let src = node["source"].as_str().unwrap_or("engine").to_string();
        *source_counts.entry(src).or_insert(0) += 1;
        let reason = node["primary_reason"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();
        *reason_counts.entry(reason).or_insert(0) += 1;
        conf_sum += node["confidence"].as_f64().unwrap_or(0.5);
        pctile_sum += node["candidate_in_degree_percentile"]
            .as_f64()
            .unwrap_or(0.0);
    }
    let n_nodes = track_nodes.len().max(1) as f64;
    let diagnostics = json!({
        "node_count": track_nodes.len(),
        "edge_count": edge_nodes.len(),
        "source_counts": source_counts,
        "reason_counts": reason_counts,
        "avg_confidence": conf_sum / n_nodes,
        "avg_in_degree_percentile": pctile_sum / n_nodes,
        "raw_candidate_count": prune_result.raw_node_count,
        "raw_edge_count": prune_result.raw_edge_count,
        "pruned_node_count": prune_result.pruned_node_count,
        "pruned_edge_count": prune_result.pruned_edge_count,
        "hub_suppressed_count": prune_result.hub_suppressed_count,
        "low_confidence_edge_dropped_count": prune_result.low_confidence_edge_dropped_count,
        "coherence": coherence,
        "filter_dropped_count": filter_dropped_count,
        "era_filter_coverage": era_filter_coverage,
        "rerank_applied": false,
    });

    // -- 11. Background seed-neighbor refresh (DiscoverSpace only) ------------
    // Fire-and-forget: computes embedding similarity for this seed, writes to
    // track_neighbors, then sends DiscoverySpaceRefreshed so the map auto-reloads.
    // `refreshed_seeds` is a TTL'd map keyed by (seed_id -> model_id, instant) so
    // entries expire and re-training invalidates them automatically.
    if seed_id > 0 && prompt.is_empty() {
        let guard = state.read().await;
        // Best-effort: read current model_id outside the spawned task so we can
        // skip the spawn entirely when this seed is fresh under the same model.
        let active_model_id: Option<i64> = guard
            .db
            .with_conn(|conn| {
                Ok(crate::db::queries::get_selected_discovery_embedding_model(conn)?.map(|m| m.id))
            })
            .unwrap_or(None);
        let already_fresh = match active_model_id {
            Some(mid) => crate::services::neighbor_refresh::is_seed_fresh(
                &guard.refreshed_seeds,
                seed_id,
                mid,
            ),
            None => true, // no model -> nothing to do anyway
        };
        if !already_fresh {
            let db2 = guard.db.clone();
            let tx = guard.event_tx.clone();
            let refreshed = Arc::clone(&guard.refreshed_seeds);
            let cache = Arc::clone(&guard.embedding_cache);
            drop(guard);
            tokio::spawn(crate::services::neighbor_refresh::refresh_seed_neighbors(
                db2, tx, seed_id, refreshed, cache,
            ));
        }
    }

    Ok(Json(json!({
        "tracks": track_nodes,
        "edges": edge_nodes,
        "artists": [],
        "diagnostics": diagnostics,
        "seed_track_id": if seed_id > 0 { Some(seed_id) } else { None },
        "generated_at": chrono::Utc::now().to_rfc3339(),
    })))
}

pub(super) async fn get_discovery_space_meta(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;

    let total_tracks: i64 = state
        .db
        .with_conn(|conn| {
            conn.query_row("SELECT COUNT(*) FROM tracks", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(Into::into)
        })
        .unwrap_or(0);

    let model_row: Option<(String, String, Option<String>, i64)> = state
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT model_key, status, trained_at, dimension
             FROM embedding_models
             WHERE is_active = 1
             ORDER BY trained_at IS NULL, trained_at DESC
             LIMIT 1",
            )?;
            let mut rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?;
            match rows.next() {
                Some(r) => Ok(Some(r?)),
                None => Ok(None),
            }
        })
        .ok()
        .flatten();

    let (model_key, model_status, trained_at, vector_dim, embedding_count) = match &model_row {
        Some((key, status, trained, dim)) => {
            let count: i64 = state
                .db
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT COUNT(*) FROM track_embeddings te
                     JOIN embedding_models em ON em.id = te.model_id
                     WHERE em.model_key = ?1",
                        rusqlite::params![key],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(Into::into)
                })
                .unwrap_or(0);
            (
                Some(key.clone()),
                Some(status.clone()),
                trained.clone(),
                Some(*dim),
                count,
            )
        }
        None => (None, None, None, None, 0),
    };

    let coverage = if total_tracks > 0 {
        embedding_count as f64 / total_tracks as f64
    } else {
        0.0
    };

    Ok(Json(json!({
        "model_key": model_key,
        "model_status": model_status,
        "trained_at": trained_at,
        "vector_dim": vector_dim,
        "neighbor_coverage": coverage,
        "track_count_with_embeddings": embedding_count,
        "track_count_total": total_tracks,
    })))
}
