use crate::AppEvent;
use crate::db::{
    Database,
    models::{
        DiscoveryNeighborReason, DiscoveryPreview, DiscoveryPreviewResult, DiscoveryProfilePreview,
        DiscoveryRadioResult, DiscoveryReason,
    },
    queries::{self, EmbeddingTrackRow, TrackSimilarityResult},
};
use crate::services::discovery::DiscoveryCandidateTrack;
use crate::services::discovery_trainer::{
    TrainerInput, TrainerSequenceGroup, TrainingProgressUpdate, run_discovery_training,
};
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::broadcast::Sender;
use tokio::sync::mpsc;

const MODEL_KEY: &str = "discovery-fusion-v1";

// Training intensity tier. Drives the cost/quality knobs the user picked in
// settings. Higher tiers train a richer model at the cost of CPU time and
// peak RAM; lower tiers stay responsive on weaker hardware. Persisted as a
// string in `server_config["discovery_intensity"]` so it survives restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryIntensity {
    Max,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy)]
pub struct IntensityParams {
    pub dimension: i32,
    pub top_k: usize,
    pub window_size: usize,
    pub include_audio_proxy: bool,
}

impl DiscoveryIntensity {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "max" => DiscoveryIntensity::Max,
            "low" => DiscoveryIntensity::Low,
            _ => DiscoveryIntensity::Medium,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            DiscoveryIntensity::Max => "max",
            DiscoveryIntensity::Medium => "medium",
            DiscoveryIntensity::Low => "low",
        }
    }

    // Cost/quality trade-offs per tier:
    //   Max    — 96-dim fused, top-64 neighbors, 8-track context window. The
    //            full model. Best radio quality; on a 30k-track library this
    //            is the ~6-12 minute run that motivated the safeguards.
    //   Medium — 64-dim, top-32, window 5. Roughly 50% of Max's wall time.
    //            Indistinguishable in subjective radio quality for libraries
    //            under ~20k tracks; the default.
    //   Low    — 48-dim, top-24, window 3, **skips the audio-proxy stage**.
    //            Pure behavioral co-occurrence. Trains in roughly a quarter
    //            of Max's time. Picks degrade for cold tracks (no metadata
    //            contribution to fusion), but the engine stays usable on
    //            modest hardware.
    pub fn params(self) -> IntensityParams {
        match self {
            DiscoveryIntensity::Max => IntensityParams {
                dimension: 96,
                top_k: 64,
                window_size: 8,
                include_audio_proxy: true,
            },
            DiscoveryIntensity::Medium => IntensityParams {
                dimension: 64,
                top_k: 32,
                window_size: 5,
                include_audio_proxy: true,
            },
            DiscoveryIntensity::Low => IntensityParams {
                dimension: 48,
                top_k: 24,
                window_size: 3,
                include_audio_proxy: false,
            },
        }
    }
}

pub fn load_discovery_intensity(db: &Database) -> DiscoveryIntensity {
    db.with_conn(|conn| {
        let raw: Option<String> = conn
            .query_row(
                "SELECT value FROM server_config WHERE key = 'discovery_intensity'",
                [],
                |row| row.get(0),
            )
            .ok();
        Ok(raw)
    })
    .ok()
    .flatten()
    .map(|s| DiscoveryIntensity::parse(&s))
    .unwrap_or(DiscoveryIntensity::Medium)
}

pub fn set_discovery_intensity(db: &Database, intensity: DiscoveryIntensity) -> Result<()> {
    db.with_conn(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO server_config (key, value) VALUES ('discovery_intensity', ?1)",
            rusqlite::params![intensity.as_str()],
        )?;
        Ok(())
    })
}

#[derive(Debug, Clone)]
pub struct ActiveLearningModel {
    pub model_id: i64,
    pub model_key: String,
    #[allow(dead_code)]
    pub family: String,
    /// Vector dimension for this trained model. Authoritative for any code
    /// that allocates buffers compared against `vectors` — the legacy 96d
    /// constant is wrong on Medium (64) and Low (48) intensity tiers.
    pub dimension: usize,
    pub vectors: HashMap<i64, Vec<f64>>,
}

pub async fn start_training(
    db: Database,
    event_tx: Sender<AppEvent>,
    full_mode: bool,
    rebuild_audio: bool,
    cancel: Arc<AtomicBool>,
) -> Result<()> {
    let intensity = load_discovery_intensity(&db);
    let intensity_params = intensity.params();
    let (model, run) = db.with_conn(|conn| {
        let config_json = serde_json::json!({
            "mode": if full_mode { "full" } else { "incremental" },
            "rebuild_audio": rebuild_audio,
            "dimension": intensity_params.dimension,
            "top_k": intensity_params.top_k,
            "intensity": intensity.as_str(),
            "trainer": "rust",
        })
        .to_string();
        let model = queries::upsert_embedding_model(
            conn,
            MODEL_KEY,
            "fusion",
            intensity_params.dimension,
            "training",
            Some(&config_json),
        )?;
        let run = queries::create_training_run(conn, Some(model.id), "corpus", "running")?;
        Ok((model, run))
    })?;

    // If cancel is requested at any stage boundary, mark the run as cancelled
    // and skip remaining persistence + model activation. Callers MUST `return Ok(())`
    // when this returns `Ok(true)` — otherwise a later stage may double-finish the run.
    let bail_if_cancelled = |stage: &str| -> Result<bool> {
        if cancel.load(Ordering::Relaxed) {
            tracing::info!(
                target: "noor.discovery.training",
                run_id = run.id,
                stage = stage,
                "discovery training cancelled by user"
            );
            db.with_conn(|conn| queries::finish_training_run(conn, run.id, "cancelled"))?;
            return Ok(true);
        }
        Ok(false)
    };

    db.with_conn(|conn| {
        queries::update_training_run_progress(conn, run.id, "corpus", "running", 0.05, None, 0)
    })?;

    // Backfill listen_history columns added in MIGRATION_023, exactly once per
    // database lifetime. The trainer is the natural trigger — sequence-aware
    // features depend on session_id and transition_from_track_id, so we do
    // this before the corpus build runs.
    if let Some(report) =
        db.with_conn(|conn| crate::services::listen_history_backfill::run_if_needed(conn))?
    {
        tracing::info!(
            target: "noor.discovery.training",
            rows_updated = report.rows_updated,
            sessions_created = report.sessions_created,
            already_populated = report.already_populated,
            "backfilled listen_history columns",
        );
    }

    // Build trainer input directly from DB (no JSON round-trip). The intensity
    // tier overrides dimension / top_k / window_size and decides whether to
    // include the audio-proxy stage (Low skips it).
    let input = db.with_conn(|conn| build_trainer_input(conn, intensity_params, full_mode))?;

    db.with_conn(|conn| {
        queries::update_training_run_progress(conn, run.id, "behavioral", "running", 0.2, None, 0)
    })?;

    // Progress channel — broadcasts to WebSocket + logs to tracing
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<TrainingProgressUpdate>();
    let run_id = run.id;
    let db_clone = db.clone();
    let event_tx_clone = event_tx.clone();
    let log_task = tokio::spawn(async move {
        while let Some(update) = progress_rx.recv().await {
            tracing::info!(target: "noor.discovery.training", run_id, %update.message, "training progress");

            // Broadcast to WebSocket
            let _ = event_tx_clone.send(AppEvent::TrainingProgress {
                stage: update.stage.clone(),
                progress: update.progress,
                message: update.message.clone(),
                current_track_id: update.current_track_id,
                current_track_title: update.current_track_title,
                tracks_done: update.tracks_done,
                tracks_total: update.tracks_total,
            });

            // Update DB progress
            let _ = db_clone.with_conn(|conn| {
                queries::update_training_run_progress(
                    conn,
                    run_id,
                    &update.stage,
                    "running",
                    update.progress as f64,
                    None,
                    0,
                )
            });
        }
    });

    // Run the trainer directly — no subprocess
    let progress_tx_clone = progress_tx.clone();
    let cancel_for_trainer = cancel.clone();
    let output = tokio::task::spawn_blocking(move || {
        run_discovery_training(input, Some(&progress_tx_clone), Some(&cancel_for_trainer))
    })
    .await
    .context("discovery trainer panicked")?;

    // Wait for progress logging to finish
    drop(progress_tx);
    let _ = log_task.await;

    if bail_if_cancelled("audio")? {
        return Ok(());
    }
    db.with_conn(|conn| {
        queries::update_training_run_progress(conn, run.id, "audio", "running", 0.55, None, 0)
    })?;

    let audio_features = output
        .audio_features
        .iter()
        .map(|(&track_id, feature)| {
            (
                track_id,
                feature.feature_version.clone(),
                pack_vector_f64(&feature.vector),
                feature.clip_start_ms,
                feature.clip_duration_ms,
            )
        })
        .collect::<Vec<_>>();

    if bail_if_cancelled("audio_features")? {
        return Ok(());
    }
    db.with_conn(|conn| queries::replace_track_audio_features(conn, &audio_features))?;
    db.with_conn(|conn| {
        queries::update_training_run_progress(conn, run.id, "fusion", "running", 0.72, None, 0)
    })?;

    let embeddings = output
        .fusion_embeddings
        .iter()
        .map(|(&track_id, vector)| {
            let norm = l2_norm(vector);
            (track_id, pack_vector_f64(vector), norm)
        })
        .collect::<Vec<_>>();
    if bail_if_cancelled("fusion")? {
        return Ok(());
    }
    db.with_conn(|conn| queries::replace_track_embeddings(conn, model.id, &embeddings))?;

    let neighbors = output
        .neighbors
        .iter()
        .map(|neighbor| {
            let reason_objects = neighbor
                .reason_tags
                .iter()
                .map(|key| DiscoveryNeighborReason {
                    key: key.clone(),
                    label: reason_label(key).to_string(),
                    weight: 1.0,
                })
                .collect::<Vec<_>>();
            let reason_json = serde_json::to_string(&reason_objects).ok();
            queries::NeighborWriteRow {
                track_id: neighbor.track_id,
                neighbor_track_id: neighbor.neighbor_track_id,
                rank: neighbor.rank,
                score: neighbor.score,
                behavioral_score: neighbor.behavioral_score,
                audio_score: neighbor.audio_score,
                metadata_score: neighbor.metadata_score,
                reason_json,
                primary_reason: neighbor.primary_reason.clone(),
                confidence: neighbor.confidence,
                support_count: neighbor.support_count,
                candidate_in_degree: neighbor.candidate_in_degree,
                candidate_in_degree_percentile: neighbor.candidate_in_degree_percentile,
                play_count_seed: neighbor.play_count_seed,
                play_count_candidate: neighbor.play_count_candidate,
            }
        })
        .collect::<Vec<_>>();
    if bail_if_cancelled("neighbors")? {
        return Ok(());
    }
    db.with_conn(|conn| {
        queries::update_training_run_progress(conn, run.id, "neighbors", "running", 0.88, None, 0)?;
        queries::replace_track_neighbors(conn, model.id, &neighbors)
    })?;

    // Persist per-reason hit-rate diagnostics. The trainer's primary_reason
    // tags drive these rows; the next iteration of metadata-bonus tuning reads
    // them to decide whether harmonic_match's 0.14 weight is justified or
    // whether genre_branch should outrank it.
    let reason_rows: Vec<queries::ReasonHitRateRow> = output
        .reason_hit_rates
        .iter()
        .map(|r| queries::ReasonHitRateRow {
            primary_reason: r.primary_reason.clone(),
            impressions: r.impressions,
            hits: r.hits,
            hit_rate: r.hit_rate,
            mean_rank: r.mean_rank,
            mrr_contribution: r.mrr_contribution,
            insufficient_data: r.insufficient_data,
        })
        .collect();
    db.with_conn(|conn| queries::replace_discovery_diagnostics(conn, model.id, &reason_rows))?;

    let metrics_json = serde_json::to_string(&output.metrics)?;
    let coverage = output.metrics.get("coverage_ratio").copied().unwrap_or(0.0);
    let recall = output.metrics.get("recall_at_10").copied().unwrap_or(0.0);
    // Thresholds scale with how much real playback signal exists. The strict
    // recall@10 gate is only meaningful when the held-out set is big enough
    // for the metric to be stable — `build_trainer_input` carves held-out
    // pairs from playback_transitions and playlist sequences, so a user with
    // a couple of plays has ≤10 held-out pairs and recall@10 collapses to
    // noise (0% or 14%, neither carries information).
    //
    // Three tiers:
    //   - 0 real plays         → coverage ≥ 0.5 (cold start, recall ignored)
    //   - 1 ≤ plays < 50       → coverage ≥ 0.7 (warm, recall too noisy to gate on)
    //   - ≥ 50 real plays      → coverage ≥ 0.85 ∧ recall ≥ 0.15 (full gate)
    //
    // 50 is the rough point where held-out has ~10+ pairs and a single hit
    // no longer flips the metric by 10pp.
    //
    // Real-play counts MUST come from playback_transitions / listen_history
    // only. Library-derived sequences (album / artist / genre / playlist /
    // favorites) reflect what's been synced, not what's been listened to.
    let playback_seqs = output
        .metrics
        .get("sequence_count.playback_transitions")
        .copied()
        .unwrap_or(0.0);
    let listen_seqs = output
        .metrics
        .get("sequence_count.listen_history")
        .copied()
        .unwrap_or(0.0);
    let real_play_seqs = playback_seqs + listen_seqs;
    let should_activate = if real_play_seqs >= 50.0 {
        coverage >= 0.85 && recall >= 0.15
    } else if real_play_seqs >= 1.0 {
        coverage >= 0.7
    } else {
        coverage >= 0.5
    };
    if bail_if_cancelled("evaluate")? {
        return Ok(());
    }
    db.with_conn(|conn| {
        queries::update_training_run_progress(conn, run.id, "evaluate", "running", 0.96, None, 0)?;
        queries::update_embedding_model_metrics(conn, model.id, "ready", Some(&metrics_json))?;
        if should_activate {
            queries::activate_embedding_model(conn, model.id)?;
        }
        queries::finish_training_run(conn, run.id, "completed")
    })?;

    Ok(())
}

pub fn load_active_learning_model(db: &Database) -> Result<Option<ActiveLearningModel>> {
    db.with_conn(|conn| {
        let Some(model) = queries::get_active_embedding_model(conn)? else {
            return Ok(None);
        };
        let vectors = queries::get_model_embeddings(conn, model.id)?
            .into_iter()
            .map(|row| (row.track_id, unpack_vector_blob(&row.vector_blob)))
            .collect::<HashMap<_, _>>();
        Ok(Some(ActiveLearningModel {
            model_id: model.id,
            model_key: model.model_key,
            family: model.family,
            dimension: model.dimension.max(0) as usize,
            vectors,
        }))
    })
}

pub fn radio_from_neighbors(
    db: &Database,
    seed_track_id: i64,
    exclude_ids: &[i64],
    limit: i64,
    creativity: f64,
) -> Result<Option<Vec<DiscoveryRadioResult>>> {
    let Some(active) = load_active_learning_model(db)? else {
        return Ok(None);
    };
    db.with_conn(|conn| {
        let neighbors = queries::get_track_neighbors(
            conn,
            active.model_id,
            seed_track_id,
            limit * 3,
            exclude_ids,
        )?;
        if neighbors.is_empty() {
            return Ok(Some(Vec::new()));
        }

        let mut rows = neighbors
            .into_iter()
            .map(|neighbor| {
                let adjusted = neighbor.score * (1.0 - creativity.clamp(0.0, 1.0) * 0.35);
                let reasons = parse_reason_tags(neighbor.reason_json.as_deref());
                DiscoveryRadioResult {
                    track_id: neighbor.track_id,
                    title: neighbor.title,
                    artist_name: neighbor.artist_name,
                    album_title: neighbor.album_title,
                    artwork_url: neighbor.artwork_url,
                    duration_ms: neighbor.duration_ms,
                    best_quality: neighbor.best_quality,
                    similarity_score: neighbor.score,
                    adjusted_score: adjusted,
                    co_listen_score: neighbor.behavioral_score,
                    co_album_score: neighbor.metadata_score,
                    co_artist_score: neighbor.audio_score,
                    genre_proximity: neighbor.metadata_score,
                    reason_tags: reasons,
                    model_key: Some(active.model_key.clone()),
                    source_mode: "embedding".to_string(),
                    confidence: neighbor.confidence,
                    support_count: neighbor.support_count,
                    candidate_in_degree: neighbor.candidate_in_degree,
                    candidate_in_degree_percentile: neighbor.candidate_in_degree_percentile,
                    primary_reason: neighbor.primary_reason,
                }
            })
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| {
            right
                .adjusted_score
                .partial_cmp(&left.adjusted_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        rows.truncate(limit.max(1) as usize);
        Ok(Some(rows))
    })
}

pub fn build_prompt_preview(
    db: &Database,
    prompt: &str,
    mode: &str,
    services: &[String],
    limit: usize,
    recent_tracks: &[TrackSimilarityResult],
) -> Result<Option<DiscoveryPreview>> {
    let Some(active) = load_active_learning_model(db)? else {
        return Ok(None);
    };
    db.with_conn(|conn| {
        let candidates = queries::get_discovery_candidate_tracks(conn, 120)?;
        let tracks = queries::get_embedding_track_rows(conn)?;
        let track_map = tracks
            .into_iter()
            .map(|row| (row.track_id, row))
            .collect::<HashMap<_, _>>();
        let anchor_ids = resolve_prompt_anchor_ids(prompt, &candidates, &track_map);
        if anchor_ids.is_empty() {
            return Ok(None);
        }
        let centroid = centroid_for_ids(&active, &anchor_ids);
        let results = rank_preview_candidates(&active, &candidates, &anchor_ids, &centroid, limit);
        let summary = format!(
            "Learned {} discovery anchored by {} library seed(s).",
            mode,
            anchor_ids.len()
        );
        let profile = DiscoveryProfilePreview {
            prompt: prompt.trim().to_string(),
            mode: mode.to_string(),
            services: services.to_vec(),
            prompt_terms: tokenize(prompt),
            prompt_genres: Vec::new(),
            top_artists: anchor_ids.iter().filter_map(|id| track_map.get(id).and_then(|row| row.artist_name.clone())).take(3).collect(),
            top_genres: Vec::new(),
            recent_tracks: recent_tracks.iter().take(5).map(|row| row.title.clone()).collect(),
            favorite_ratio: 0.0,
            completion_rate: 0.0,
            summary,
        };
        let reasons = vec![
            DiscoveryReason {
                label: "Anchor tracks".to_string(),
                detail: format!("Prompt resolved to {} anchor tracks in your library.", anchor_ids.len()),
                weight: 82,
            },
            DiscoveryReason {
                label: "Embedding centroid".to_string(),
                detail: "Results come from the learned neighborhood around those anchors, not raw text hits.".to_string(),
                weight: 77,
            },
        ];
        Ok(Some(DiscoveryPreview {
            profile,
            reasons,
            results,
        }))
    })
}

pub fn compute_external_embedding_scores(
    db: &Database,
    prompt: &str,
    candidates: &[DiscoveryCandidateTrack],
) -> Result<HashMap<String, f64>> {
    let Some(active) = load_active_learning_model(db)? else {
        return Ok(HashMap::new());
    };
    db.with_conn(|conn| {
        let library_candidates = queries::get_discovery_candidate_tracks(conn, 120)?;
        let track_rows = queries::get_embedding_track_rows(conn)?;
        let track_map = track_rows
            .into_iter()
            .map(|row| (row.track_id, row))
            .collect::<HashMap<_, _>>();
        let anchor_ids = resolve_prompt_anchor_ids(prompt, &library_candidates, &track_map);
        if anchor_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let centroid = centroid_for_ids(&active, &anchor_ids);
        let mut scores = HashMap::new();
        for candidate in candidates {
            let proxy = external_candidate_proxy_vector(candidate, active.dimension);
            scores.insert(
                candidate.provider_track_id.clone(),
                cosine_similarity(&centroid, &proxy),
            );
        }
        Ok(scores)
    })
}

pub fn inject_query_seeds_from_neighbors(
    db: &Database,
    seed_track_id: i64,
    limit: usize,
) -> Result<Vec<String>> {
    let Some(active) = load_active_learning_model(db)? else {
        return Ok(Vec::new());
    };
    db.with_conn(|conn| {
        let neighbors =
            queries::get_track_neighbors(conn, active.model_id, seed_track_id, limit as i64, &[])?;
        let mut queries = Vec::new();
        for neighbor in neighbors {
            if let Some(artist) = neighbor.artist_name {
                queries.push(artist);
            }
            queries.push(neighbor.title);
            if let Some(album) = neighbor.album_title {
                queries.push(album);
            }
        }
        queries.sort();
        queries.dedup();
        Ok(queries)
    })
}

fn build_trainer_input(
    conn: &rusqlite::Connection,
    intensity: IntensityParams,
    full_mode: bool,
) -> Result<TrainerInput> {
    let tracks = queries::get_embedding_track_rows(conn)?;
    let transition_sequences = queries::get_playback_transition_sequences(conn)?;
    let listen_sequences = queries::get_listen_history_sequences(conn, 45)?;
    let playlist_sequences = queries::get_playlist_sequences(conn)?;
    let album_sequences = queries::get_album_sequences(conn)?;
    let artist_sequences = queries::get_artist_sequences(conn)?;
    let genre_sequences = queries::get_genre_sequences(conn)?;
    let favorite_ids = queries::get_favorite_track_ids(conn)?;
    let favorite_sequences = favorite_ids
        .chunks(8)
        .filter(|chunk| chunk.len() > 1)
        .map(|chunk| chunk.to_vec())
        .collect::<Vec<_>>();

    let heldout_pairs = transition_sequences
        .iter()
        .chain(playlist_sequences.iter())
        .enumerate()
        .filter_map(|(index, sequence)| {
            if index % 10 == 0 && sequence.len() >= 2 {
                Some((sequence[0], sequence[1]))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    // Incremental Refresh: try to hydrate cached audio features from the prior
    // run. We only reuse rows whose stored vector dim matches the current
    // intensity tier — flipping Max→Medium changes the vector size, in which
    // case the cache is invalid and we recompute. None when full_mode is true,
    // when intensity skips audio entirely (Low), or when no cache rows match.
    let cached_audio_features = if !full_mode && intensity.include_audio_proxy {
        let expected_dim = intensity.dimension as usize;
        let rows = queries::get_cached_audio_features(conn)?;
        let map: HashMap<i64, crate::services::discovery_trainer::TrainerAudioFeature> = rows
            .into_iter()
            .filter_map(|row| {
                let vector = unpack_vector_blob(&row.vector_blob);
                if vector.len() != expected_dim {
                    return None;
                }
                Some((
                    row.track_id,
                    crate::services::discovery_trainer::TrainerAudioFeature {
                        vector,
                        clip_start_ms: row.clip_start_ms,
                        clip_duration_ms: row.clip_duration_ms,
                        feature_version: row.feature_version,
                    },
                ))
            })
            .collect();
        if map.is_empty() { None } else { Some(map) }
    } else {
        None
    };

    Ok(TrainerInput {
        seed: 13,
        dimension: intensity.dimension as usize,
        window_size: intensity.window_size,
        min_count: 1,
        top_k: intensity.top_k,
        include_audio_proxy: intensity.include_audio_proxy,
        tracks,
        sequences: vec![
            TrainerSequenceGroup {
                label: "playback_transitions".to_string(),
                weight: 1.6,
                sequences: transition_sequences,
            },
            TrainerSequenceGroup {
                label: "listen_history".to_string(),
                weight: 1.3,
                sequences: listen_sequences,
            },
            TrainerSequenceGroup {
                label: "playlist_tracks".to_string(),
                weight: 1.1,
                sequences: playlist_sequences,
            },
            TrainerSequenceGroup {
                label: "album_tracks".to_string(),
                weight: 0.7,
                sequences: album_sequences,
            },
            TrainerSequenceGroup {
                label: "artist_tracks".to_string(),
                weight: 0.35,
                sequences: artist_sequences,
            },
            TrainerSequenceGroup {
                label: "genre_tracks".to_string(),
                weight: 0.3,
                sequences: genre_sequences,
            },
            TrainerSequenceGroup {
                label: "favorites".to_string(),
                weight: 1.2,
                sequences: favorite_sequences,
            },
        ],
        heldout_pairs,
        cached_audio_features,
    })
}

fn centroid_for_ids(active: &ActiveLearningModel, anchor_ids: &[i64]) -> Vec<f64> {
    let dim = active.dimension;
    let mut centroid = vec![0.0; dim];
    let mut count = 0.0;
    for track_id in anchor_ids {
        if let Some(vector) = active.vectors.get(track_id) {
            // Defensive: skip vectors that don't match the model's stated dim.
            // Shouldn't happen post-fix, but a pre-fix DB may have stale rows
            // from a prior intensity tier and a mismatched accumulate would
            // index out-of-range or silently leave the tail at zero.
            if vector.len() != dim {
                continue;
            }
            for (index, value) in vector.iter().enumerate() {
                centroid[index] += value;
            }
            count += 1.0;
        }
    }
    if count > 0.0 {
        for value in &mut centroid {
            *value /= count;
        }
    }
    normalize_vector(&centroid)
}

fn rank_preview_candidates(
    active: &ActiveLearningModel,
    candidates: &[crate::db::models::Track],
    anchor_ids: &[i64],
    centroid: &[f64],
    limit: usize,
) -> Vec<DiscoveryPreviewResult> {
    let anchor_set = anchor_ids.iter().copied().collect::<HashSet<_>>();
    let mut results = candidates
        .iter()
        .filter(|track| !anchor_set.contains(&track.id))
        .filter_map(|track| {
            active.vectors.get(&track.id).map(|vector| {
                let score = (cosine_similarity(centroid, vector).max(0.0) * 100.0).round() as i32;
                DiscoveryPreviewResult {
                    track_id: track.id,
                    title: track.title.clone(),
                    artist_name: track.artist_name.clone(),
                    album_title: track.album_title.clone(),
                    artwork_url: track.artwork_url.clone(),
                    duration_ms: track.duration_ms,
                    service: track.source.clone(),
                    service_track_id: track
                        .tidal_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| track.id.to_string()),
                    score,
                    tags: vec!["embedding centroid".to_string()],
                }
            })
        })
        .collect::<Vec<_>>();
    results.sort_by(|left, right| right.score.cmp(&left.score));
    results.truncate(limit.max(1));
    results
}

fn resolve_prompt_anchor_ids(
    prompt: &str,
    candidates: &[crate::db::models::Track],
    track_map: &HashMap<i64, EmbeddingTrackRow>,
) -> Vec<i64> {
    let tokens = tokenize(prompt);
    let mut scored = candidates
        .iter()
        .filter_map(|track| {
            let mut score = 0_i32;
            let haystack = format!(
                "{} {} {}",
                track.title,
                track.artist_name.as_deref().unwrap_or_default(),
                track.album_title.as_deref().unwrap_or_default()
            )
            .to_ascii_lowercase();
            for token in &tokens {
                if haystack.contains(token) {
                    score += 4;
                }
            }
            if let Some(row) = track_map.get(&track.id) {
                for genre in &row.genre_paths {
                    let genre_lower = genre.to_ascii_lowercase();
                    for token in &tokens {
                        if genre_lower.contains(token) {
                            score += 3;
                        }
                    }
                }
            }
            if score > 0 {
                Some((track.id, score))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| right.1.cmp(&left.1));
    scored
        .into_iter()
        .take(3)
        .map(|(track_id, _)| track_id)
        .collect()
}

fn external_candidate_proxy_vector(candidate: &DiscoveryCandidateTrack, dim: usize) -> Vec<f64> {
    let mut tokens = tokenize(&candidate.title);
    if let Some(artist) = candidate.artist_name.as_deref() {
        tokens.extend(tokenize(artist));
    }
    if let Some(album) = candidate.album_title.as_deref() {
        tokens.extend(tokenize(album));
    }
    for tag in candidate
        .raw_genre_hints
        .iter()
        .chain(candidate.lastfm_tags.iter())
        .chain(candidate.discogs_styles.iter())
        .chain(candidate.discogs_genres.iter())
    {
        tokens.extend(tokenize(tag));
    }
    hashed_token_vector(&tokens, dim)
}

pub fn pack_vector_f64(vector: &[f64]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        bytes.extend_from_slice(&(*value as f32).to_le_bytes());
    }
    bytes
}

pub fn unpack_vector_blob(blob: &[u8]) -> Vec<f64> {
    blob.chunks_exact(4)
        .map(|chunk| {
            let bytes: [u8; 4] = [chunk[0], chunk[1], chunk[2], chunk[3]];
            f32::from_le_bytes(bytes) as f64
        })
        .collect()
}

fn l2_norm(vector: &[f64]) -> f64 {
    vector.iter().map(|value| value * value).sum::<f64>().sqrt()
}

fn normalize_vector(vector: &[f64]) -> Vec<f64> {
    let norm = l2_norm(vector);
    if norm <= 1e-12 {
        return vec![0.0; vector.len()];
    }
    vector.iter().map(|value| value / norm).collect()
}

fn cosine_similarity(left: &[f64], right: &[f64]) -> f64 {
    left.iter()
        .zip(right.iter())
        .map(|(a, b)| a * b)
        .sum::<f64>()
}

fn tokenize(value: &str) -> Vec<String> {
    value
        .to_ascii_lowercase()
        .split(|char: char| !char.is_ascii_alphanumeric())
        .filter(|part| !part.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn hashed_token_vector(tokens: &[String], dim: usize) -> Vec<f64> {
    let mut vector = vec![0.0; dim];
    for token in tokens {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        token.hash(&mut hasher);
        let hash = hasher.finish();
        for offset in 0..4 {
            let bucket = ((hash >> (offset * 8)) as usize) % dim;
            let sign = if ((hash >> (offset * 8 + 1)) & 1) == 0 {
                1.0
            } else {
                -1.0
            };
            vector[bucket] += sign * 0.5;
        }
    }
    normalize_vector(&vector)
}

fn parse_reason_tags(reason_json: Option<&str>) -> Vec<String> {
    serde_json::from_str::<Vec<DiscoveryNeighborReason>>(reason_json.unwrap_or("[]"))
        .unwrap_or_default()
        .into_iter()
        .map(|reason| reason.label)
        .collect()
}

fn reason_label(key: &str) -> &'static str {
    match key {
        "behavioral" => "same pocket",
        "audio_texture" => "audio texture",
        "album_context" => "album-adjacent",
        "artist_affinity" => "session neighbor",
        "genre_branch" => "genre branch",
        _ => "learned signal",
    }
}
