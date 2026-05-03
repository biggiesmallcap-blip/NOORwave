/// Background per-seed neighbor computation for DiscoverSpace.
///
/// When `/api/discovery/space` is called with a seed_id, this module fires a
/// non-blocking task that computes up-to-date embedding similarity for that seed
/// against all library tracks, applies the same metadata bonuses as full training,
/// writes the result to `track_neighbors` (replacing only the seed's rows), and
/// broadcasts `AppEvent::DiscoverySpaceRefreshed` so the frontend auto-reloads.
///
/// Prerequisite: full training must have run at least once so embeddings exist.
/// All failure paths are silent no-ops — the DiscoverSpace page degrades gracefully.

use crate::{AppEvent, db};
use crate::db::queries::{
    get_active_embedding_model, get_model_embeddings, get_embedding_track_rows,
    EmbeddingTrackRow, NeighborWriteRow, replace_seed_neighbors,
};
use crate::services::learning::unpack_vector_blob;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tracing::{info, warn};

/// Treat per-seed refresh results as fresh for this long. After it expires, the
/// next visit re-runs the refresh so retraining or stale data don't get pinned.
pub const REFRESH_TTL: Duration = Duration::from_secs(60 * 60 * 6); // 6h
/// How long the loaded embedding/metadata snapshot stays reusable. Short enough
/// that training updates take effect quickly; long enough to amortize across
/// rapid-fire seed clicks.
pub const EMBEDDING_CACHE_TTL: Duration = Duration::from_secs(60);

/// Per-seed entry tracked in `AppState::refreshed_seeds`. `model_id` lets us
/// invalidate when the active embedding model changes (e.g. after retraining).
#[derive(Debug, Clone, Copy)]
pub struct RefreshEntry {
    pub model_id: i64,
    pub at: Instant,
}

/// Cached embedding-table snapshot, keyed implicitly by `model_id`.
/// Stored in `AppState::embedding_cache` so back-to-back seed refreshes don't
/// each pay the full table scan.
pub struct EmbeddingCache {
    pub model_id: i64,
    pub built_at: Instant,
    pub vec_map: Arc<HashMap<i64, Vec<f64>>>,
    pub meta_map: Arc<HashMap<i64, TrackMeta>>,
}

/// `std::sync::Mutex` poisons on panic. We treat poisoning as "ignore old state"
/// rather than panicking the request thread — the worst case is one extra refresh.
fn lock_refreshed<T>(
    m: &Mutex<T>,
) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Returns true if the seed was refreshed under the current model within the TTL.
pub fn is_seed_fresh(
    refreshed: &Mutex<HashMap<i64, RefreshEntry>>,
    seed_id: i64,
    model_id: i64,
) -> bool {
    let guard = lock_refreshed(refreshed);
    match guard.get(&seed_id) {
        Some(entry) => entry.model_id == model_id && entry.at.elapsed() < REFRESH_TTL,
        None => false,
    }
}

// ─── Local metadata struct (mirrors TrackMeta in discovery_trainer) ───────────

pub struct TrackMeta {
    pub artist_lower: Option<String>,
    pub album: Option<String>,
    pub genre_set: HashSet<String>,
    pub bpm: Option<f64>,
    pub energy: Option<f64>,
    pub camelot_key: Option<String>,
    pub play_count: i64,
}

fn build_meta_map(tracks: &[EmbeddingTrackRow]) -> HashMap<i64, TrackMeta> {
    tracks
        .iter()
        .map(|t| {
            let genre_set: HashSet<String> = t
                .genre_paths
                .iter()
                .flat_map(|p| p.split('/'))
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect();
            (
                t.track_id,
                TrackMeta {
                    artist_lower: t.artist_name.as_ref().map(|s| s.to_lowercase()),
                    album: t.album_title.clone(),
                    genre_set,
                    bpm: t.bpm,
                    energy: t.energy,
                    camelot_key: t.camelot_key.clone(),
                    play_count: t.play_count as i64,
                },
            )
        })
        .collect()
}

fn cosine(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let norm_b: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)).clamp(-1.0, 1.0)
}

// Bonuses match weights used in `discovery_trainer`. Returned tags are paired
// with their contribution so the caller can pick the largest one as
// `primary_reason` (insertion order would otherwise give a misleading reason).
fn metadata_bonus(seed: &TrackMeta, cand: &TrackMeta) -> (f64, Vec<(&'static str, f64)>) {
    let mut score = 0.0f64;
    let mut tags: Vec<(&'static str, f64)> = Vec::new();

    if let (Some(a), Some(b)) = (&seed.artist_lower, &cand.artist_lower) {
        if a == b {
            score += 0.20;
            tags.push(("artist_affinity", 0.20));
        }
    }

    if !seed.genre_set.is_empty()
        && !cand.genre_set.is_empty()
        && seed.genre_set.intersection(&cand.genre_set).next().is_some()
    {
        score += 0.18;
        tags.push(("genre_branch", 0.18));
    }

    if let (Some(a), Some(b)) = (&seed.album, &cand.album) {
        if a == b {
            score += 0.12;
            tags.push(("album_context", 0.12));
        }
    }

    if let (Some(a), Some(b)) = (seed.bpm, cand.bpm) {
        let diff = (a - b).abs();
        if diff <= 3.0 {
            score += 0.15;
            tags.push(("bpm_match", 0.15));
        } else if diff <= 8.0 {
            score += 0.08;
            tags.push(("bpm_match", 0.08));
        }
    }

    if let (Some(a), Some(b)) = (&seed.camelot_key, &cand.camelot_key) {
        if a == b {
            score += 0.14;
            tags.push(("harmonic_match", 0.14));
        } else {
            let parse_key = |k: &str| -> Option<(i64, String)> {
                let num_end = k.find(|c: char| c.is_alphabetic())?;
                let n = k[..num_end].parse::<i64>().ok()?;
                Some((n, k[num_end..].to_string()))
            };
            if let (Some((an, asuf)), Some((bn, bsuf))) = (parse_key(a), parse_key(b)) {
                let raw_diff = (an - bn).abs();
                let wheel_diff = raw_diff.min(12 - raw_diff);
                if wheel_diff <= 1 && asuf == bsuf {
                    score += 0.10;
                    tags.push(("harmonic_match", 0.10));
                } else if an == bn && asuf != bsuf {
                    score += 0.08;
                    tags.push(("harmonic_match", 0.08));
                }
            }
        }
    }

    if let (Some(a), Some(b)) = (seed.energy, cand.energy) {
        if (a - b).abs() <= 0.1 {
            score += 0.08;
            tags.push(("energy_match", 0.08));
        }
    }

    (score, tags)
}

/// Compute neighbors for a single seed track using existing embeddings, write
/// them to `track_neighbors`, and notify the frontend via WebSocket event.
/// Fire-and-forget: caller spawns this with `tokio::spawn`.
pub async fn refresh_seed_neighbors(
    db: db::Database,
    event_tx: broadcast::Sender<AppEvent>,
    seed_id: i64,
    refreshed_seeds: Arc<Mutex<HashMap<i64, RefreshEntry>>>,
    embedding_cache: Arc<tokio::sync::Mutex<Option<EmbeddingCache>>>,
) {
    let tx = event_tx.clone();
    let send_progress = move |stage: &str, progress: f32| {
        let _ = tx.send(AppEvent::DiscoverySpaceRefreshProgress {
            seed_track_id: seed_id,
            stage: stage.to_string(),
            progress,
        });
    };

    // Try the cache first (async lock, OK to hold across await — short).
    let cached = {
        let guard = embedding_cache.lock().await;
        guard.as_ref().and_then(|c| {
            if c.built_at.elapsed() < EMBEDDING_CACHE_TTL {
                Some((c.model_id, Arc::clone(&c.vec_map), Arc::clone(&c.meta_map)))
            } else {
                None
            }
        })
    };

    let send_progress_for_blocking = send_progress.clone();
    let cache_for_store = Arc::clone(&embedding_cache);

    let result: Result<Option<(i64, Arc<HashMap<i64, Vec<f64>>>, Arc<HashMap<i64, TrackMeta>>)>, anyhow::Error> =
        tokio::task::spawn_blocking(move || {
            db.with_conn(|conn| {
                let Some(model) = get_active_embedding_model(conn)? else {
                    info!("[neighbor_refresh] no active model — skipping seed {seed_id}");
                    return Ok::<_, anyhow::Error>(None);
                };

                let (vec_map, meta_map) = if let Some((cached_id, vm, mm)) = cached {
                    if cached_id == model.id {
                        (vm, mm)
                    } else {
                        send_progress_for_blocking("loading", 0.1);
                        let embeddings = get_model_embeddings(conn, model.id)?;
                        if embeddings.is_empty() {
                            info!("[neighbor_refresh] model {} has no embeddings", model.id);
                            return Ok(None);
                        }
                        let vm: HashMap<i64, Vec<f64>> = embeddings
                            .iter()
                            .map(|e| (e.track_id, unpack_vector_blob(&e.vector_blob)))
                            .collect();
                        let all_tracks = get_embedding_track_rows(conn)?;
                        let mm = build_meta_map(&all_tracks);
                        (Arc::new(vm), Arc::new(mm))
                    }
                } else {
                    send_progress_for_blocking("loading", 0.1);
                    let embeddings = get_model_embeddings(conn, model.id)?;
                    if embeddings.is_empty() {
                        info!("[neighbor_refresh] model {} has no embeddings", model.id);
                        return Ok(None);
                    }
                    let vm: HashMap<i64, Vec<f64>> = embeddings
                        .iter()
                        .map(|e| (e.track_id, unpack_vector_blob(&e.vector_blob)))
                        .collect();
                    let all_tracks = get_embedding_track_rows(conn)?;
                    let mm = build_meta_map(&all_tracks);
                    (Arc::new(vm), Arc::new(mm))
                };

                let Some(seed_vec) = vec_map.get(&seed_id) else {
                    warn!("[neighbor_refresh] seed {seed_id} not in embedding table — skipping");
                    return Ok(None);
                };
                let seed_vec = seed_vec.clone();

                let seed_meta = meta_map.get(&seed_id);
                let seed_play_count = seed_meta.map(|m| m.play_count).unwrap_or(0);

                send_progress_for_blocking("computing", 0.4);

                // Score all candidates. Periodic progress pings keep the spinner alive
                // for large libraries (50k+ tracks).
                let total_cands = vec_map.len().saturating_sub(1).max(1) as f32;
                let progress_step = (total_cands / 8.0).max(1.0) as usize;
                let mut scored: Vec<(i64, f64, f64, Vec<(&'static str, f64)>)> = Vec::with_capacity(vec_map.len());
                let mut idx: usize = 0;
                for (cand_id, cand_vec) in vec_map.iter() {
                    if *cand_id == seed_id { continue; }
                    let sim = cosine(&seed_vec, cand_vec);
                    let (bonus, tags) =
                        if let (Some(sm), Some(cm)) = (seed_meta, meta_map.get(cand_id)) {
                            metadata_bonus(sm, cm)
                        } else {
                            (0.0, vec![])
                        };
                    scored.push((*cand_id, (sim + bonus).clamp(0.0, 1.0), sim, tags));
                    idx += 1;
                    if idx % progress_step == 0 {
                        let frac = (idx as f32 / total_cands).min(1.0);
                        send_progress_for_blocking("computing", 0.4 + frac * 0.4);
                    }
                }

                scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                scored.truncate(64);

                if scored.is_empty() {
                    return Ok(Some((model.id, vec_map, meta_map)));
                }

                send_progress_for_blocking("saving", 0.85);

                let rows: Vec<NeighborWriteRow> = scored
                    .into_iter()
                    .enumerate()
                    .map(|(rank, (cand_id, total, sim, tags))| {
                        // Pick the highest-weighted matched tag as primary_reason
                        // (insertion order would mislead — e.g. small album bonus
                        // beating a larger BPM bonus).
                        let primary_reason = tags
                            .iter()
                            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                            .map(|(k, _)| (*k).to_string());
                        let reason_json = if tags.is_empty() {
                            None
                        } else {
                            Some(
                                serde_json::to_string(
                                    &tags
                                        .iter()
                                        .map(|(k, _)| serde_json::json!({"key": k}))
                                        .collect::<Vec<_>>(),
                                )
                                .unwrap_or_default(),
                            )
                        };
                        let cand_play = meta_map.get(&cand_id).map(|m| m.play_count).unwrap_or(0);
                        // Confidence proxies how strongly we trust the audio match.
                        // Floor at 0.4 so refreshed neighbors don't trip the cold-start
                        // gate (`< 0.3`) used downstream in `routes.rs`.
                        let confidence = (0.40 + sim.clamp(0.0, 1.0) * 0.55).clamp(0.0, 1.0);
                        NeighborWriteRow {
                            track_id: seed_id,
                            neighbor_track_id: cand_id,
                            rank: (rank + 1) as i32,
                            score: total,
                            behavioral_score: 0.0,
                            audio_score: sim,
                            metadata_score: total - sim,
                            reason_json,
                            primary_reason,
                            confidence,
                            support_count: 0,
                            candidate_in_degree: 0,
                            candidate_in_degree_percentile: 0.5,
                            play_count_seed: seed_play_count,
                            play_count_candidate: cand_play,
                        }
                    })
                    .collect();

                let n = rows.len();
                replace_seed_neighbors(conn, model.id, seed_id, &rows)?;
                info!("[neighbor_refresh] wrote {n} neighbors for seed {seed_id} (model {})", model.id);

                Ok(Some((model.id, vec_map, meta_map)))
            })
        })
        .await
        .unwrap_or_else(|e| Err(anyhow::anyhow!("task panic: {e}")));

    match result {
        Ok(Some((model_id, vec_map, meta_map))) => {
            // Refresh cache so subsequent seeds reuse the same embedding snapshot.
            {
                let mut guard = cache_for_store.lock().await;
                let stale = guard.as_ref().is_none_or(|c| c.built_at.elapsed() >= EMBEDDING_CACHE_TTL || c.model_id != model_id);
                if stale {
                    *guard = Some(EmbeddingCache {
                        model_id,
                        built_at: Instant::now(),
                        vec_map,
                        meta_map,
                    });
                }
            }
            lock_refreshed(&refreshed_seeds).insert(
                seed_id,
                RefreshEntry { model_id, at: Instant::now() },
            );
            let _ = event_tx.send(AppEvent::DiscoverySpaceRefreshed { seed_track_id: seed_id });
        }
        Ok(None) => { /* no-op: cold path or empty model */ }
        Err(e) => warn!("[neighbor_refresh] error for seed {seed_id}: {e}"),
    }
}
