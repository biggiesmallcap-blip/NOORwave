//! Radio endpoints: song, album, artist and seed starts, the TIDAL mix fallback, and the radio similarity index.

use super::*;

#[derive(Debug, Deserialize)]
pub(super) struct RadioRequest {
    pub(super) seed_track_id: Option<i64>,
    pub(super) seed_tidal_id: Option<i64>, // resolve to local library track when seed_track_id <= 0
    pub(super) creativity: Option<f64>,    // 0.0 (tight) to 1.0 (adventurous), default 0.3
    pub(super) context_window: Option<i64>, // number of recent tracks to influence, default 5
    pub(super) limit: Option<i64>,         // results to return, default 20
    pub(super) exclude_ids: Option<Vec<i64>>, // already-played track IDs
}

/// Get similar tracks for the "Similar Radio" feature.
/// Combines pre-computed similarity scores with creativity/context adjustments.
pub(super) async fn get_radio_tracks(
    State(state): State<SharedState>,
    Json(payload): Json<RadioRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let creativity = payload.creativity.unwrap_or(0.3).clamp(0.0, 1.0);
    let context_window = payload.context_window.unwrap_or(5).max(0) as usize;
    let limit = payload.limit.unwrap_or(20).clamp(1, 50);
    let exclude_ids = payload.exclude_ids.unwrap_or_default();

    let state = state.read().await;

    let seed_track_id: i64 = if let Some(id) = payload.seed_track_id.filter(|&id| id > 0) {
        id
    } else if let Some(tidal_id) = payload.seed_tidal_id {
        state
            .db
            .with_conn(|conn| crate::db::catalogue::track_id(conn, tidal_id))
            .map_err(|e| {
                tracing::error!("DB error resolving tidal_id {}: {}", tidal_id, e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": "Database error"})),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error": "No local track matches that Tidal ID"})),
                )
            })?
    } else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "seed_track_id or seed_tidal_id required"})),
        ));
    };

    if let Some(mut rows) = discovery_learning::radio_from_neighbors(
        &state.db,
        seed_track_id,
        &exclude_ids,
        limit,
        creativity,
    )
    .map_err(|e| {
        tracing::error!("Failed to load embedding neighbors: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Failed to query learned neighbors"})),
        )
    })? {
        // DSP harmonic post-scoring - apply the shared harmonic multiplier to
        // every row that has audio features on both sides. Rows without
        // features are left untouched (never penalised for being unanalyzed).
        let seed_features = state
            .db
            .with_conn(|conn| queries::get_audio_dsp_features(conn, seed_track_id))
            .ok()
            .flatten();

        if let Some(seed) = seed_features.as_ref() {
            // Batch the candidate harmonic-key lookups into one query instead of a
            // serialized per-candidate round trip under the DB mutex.
            let cand_ids: Vec<i64> = rows.iter().map(|r| r.track_id).collect();
            let cand_keys = state
                .db
                .with_conn(|conn| queries::get_dsp_harmonic_keys_batch(conn, &cand_ids))
                .unwrap_or_default();
            for row in rows.iter_mut() {
                if let Some((cand_camelot, cand_bpm)) = cand_keys.get(&row.track_id) {
                    let mult = crate::services::audio_analysis::compute_harmonic_multiplier(
                        seed.camelot_key.as_deref(),
                        cand_camelot.as_deref(),
                        seed.bpm,
                        *cand_bpm,
                    );
                    row.adjusted_score *= mult;
                    if mult > 1.5 && !row.reason_tags.iter().any(|t| t == "harmonic match") {
                        row.reason_tags.push("harmonic match".to_string());
                    }
                }
            }
            // Re-sort by adjusted_score descending after the multiplier pass.
            rows.sort_by(|a, b| {
                b.adjusted_score
                    .partial_cmp(&a.adjusted_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        let model = state
            .db
            .with_conn(queries::get_selected_discovery_embedding_model)
            .ok()
            .flatten();
        return Ok(Json(json!({
            "tracks": rows,
            "seed_track_id": seed_track_id,
            "creativity": creativity,
            "context_window": context_window,
            "computed_at": model.as_ref().and_then(|m| m.trained_at.clone()),
            "model_family": model.as_ref().map(|m| m.family.clone()),
            "model_key": model.as_ref().map(|m| m.model_key.clone()),
            "reasons": ["learned neighbors", "session feedback", "taste graph", "harmonic post-scoring"],
        })));
    }

    // Get similar tracks from pre-computed similarity table
    let similar = state
        .db
        .with_conn(|conn| queries::get_similar_tracks(conn, seed_track_id, limit * 3, &exclude_ids))
        .map_err(|e| {
            tracing::error!("Failed to get similar tracks: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to query similar tracks"})),
            )
        })?;

    if similar.is_empty() {
        // Fallback: return random tracks from the same artist/genre
        return Ok(Json(json!({
            "tracks": [],
            "message": "No similar tracks found. Try syncing your library or running similarity computation.",
            "computed_at": null,
        })));
    }

    // Apply creativity filter: higher creativity = pick from further down the list
    // We use a temperature-based sampling: sort by adjusted score with noise
    let temperature = creativity * 0.5; // 0.0 = deterministic, 0.5 = max noise

    use rand::Rng;
    let mut rng = rand::rng();

    let mut scored: Vec<_> = similar
        .into_iter()
        .map(|track| {
            // Add noise proportional to creativity
            let noise = rng.random_range(0.0..=temperature);
            let adjusted_score = track.similarity_score * (1.0 - temperature) + noise;
            (track, adjusted_score)
        })
        .collect();

    // Sort by adjusted score
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Take top `limit` results
    let results: Vec<_> = scored
        .into_iter()
        .take(limit as usize)
        .map(|(track, adjusted_score)| {
            json!({
                "track_id": track.track_id,
                "title": track.title,
                "artist_name": track.artist_name,
                "album_title": track.album_title,
                "artwork_url": track.artwork_url,
                "duration_ms": track.duration_ms,
                "best_quality": track.best_quality,
                "similarity_score": track.similarity_score,
                "adjusted_score": adjusted_score,
                "co_listen_score": track.co_listen_score,
                "co_album_score": track.co_album_score,
                "co_artist_score": track.co_artist_score,
                "genre_proximity": track.genre_proximity,
                "reason_tags": Vec::<String>::new(),
                "model_key": Value::Null,
                "source_mode": "legacy",
            })
        })
        .collect();

    // Get computation timestamp
    let computed_at = state
        .db
        .with_conn(queries::get_similarity_computed_at)
        .ok()
        .flatten();

    Ok(Json(json!({
        "tracks": results,
        "seed_track_id": seed_track_id,
        "creativity": creativity,
        "context_window": context_window,
        "computed_at": computed_at,
        "model_family": Value::Null,
        "model_key": Value::Null,
        "reasons": ["legacy similarity fallback"],
    })))
}

/// Trigger background similarity computation.
pub(super) async fn compute_radio_similarity(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let (db, event_tx, running, busy) = {
        let s = state.read().await;
        let busy = crate::services::radio_similarity::busy_reason(&s, &s.db);
        (
            s.db.clone(),
            s.event_tx.clone(),
            s.radio_similarity_running.clone(),
            busy,
        )
    };

    // A rebuild owns SQLite's single writer slot for minutes. The manual route
    // gates on the same idle check as the auto path â€” clicking the button does
    // not justify failing an in-flight sync or listen-history write.
    if let Some(reason) = busy {
        return Ok(Json(json!({
            "status": "busy",
            "message": format!("Can't rebuild while {reason} is active. Try again once it's finished.")
        })));
    }

    // Shared single-flight + isolated-connection rebuild path: a manual click
    // and an auto-rebuild can never run the multi-minute job twice, and the
    // job never holds the shared connection mutex.
    if crate::services::radio_similarity::try_spawn_rebuild(db, event_tx, running) {
        Ok(Json(json!({
            "status": "computation_started",
            "message": "Similarity computation running in background. This may take a few minutes for large libraries."
        })))
    } else {
        Ok(Json(json!({
            "status": "already_running",
            "message": "Similarity computation is already in progress."
        })))
    }
}

/// Status of the radio similarity index: row count + last-built timestamp.
/// Powers the Settings "Build radio similarity index" panel â€” the frontend
/// polls this after triggering a compute to detect completion. `built_at`
/// comes from `server_config`, not the table's rows, so a legitimate zero-row
/// rebuild still reads as built.
pub(super) async fn radio_similarity_status(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let db = {
        let s = state.read().await;
        s.db.clone()
    };
    let row_count = db.with_conn(queries::count_track_similarity).unwrap_or(0);
    let built_at = db
        .with_conn(queries::get_radio_similarity_built_at)
        .ok()
        .flatten();
    Ok(Json(json!({
        "row_count": row_count,
        "built_at": built_at,
    })))
}

pub(in crate::server) fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": e.to_string() })),
    )
}

#[derive(Debug, Deserialize)]
pub(super) struct RadioSongRequest {
    pub(super) seed_track_id: i64,
    #[serde(default)]
    pub(super) blend: Option<crate::services::radio::RadioBlend>,
    #[serde(default)]
    pub(super) limit: Option<usize>,
    #[serde(default)]
    pub(super) exclude_track_ids: Option<Vec<i64>>,
}

pub(super) async fn radio_song(
    State(state): State<SharedState>,
    Json(payload): Json<RadioSongRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    // Reject ephemeral Tidal ids (negative) and zero up front. The
    // orchestrator can only resolve positive library ids; previous
    // behaviour was a 500 with no body and a WARN log, which made
    // mis-routed callers (e.g. menu dispatch bugs) look like server
    // failures rather than bad inputs. Hand back a 400 with a hint
    // pointing at the right endpoint for Tidal-only seeds.
    if payload.seed_track_id <= 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "seed_track_id must be a positive library id",
                "hint": "for Tidal-only seeds, POST /api/discovery/radio with seed_tidal_id"
            })),
        ));
    }

    let blend = payload.blend.unwrap_or_default();
    let limit = payload.limit.unwrap_or(60).clamp(8, 200);
    let exclude = payload.exclude_track_ids.unwrap_or_default();

    let (db, lastfm, lastfm_similar_cache) = {
        let g = state.read().await;
        g.user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let lastfm = crate::metadata::lastfm::LastFmClient::load(g.http_client.clone(), &g.db);
        (g.db.clone(), lastfm, g.lastfm_similar_cache.clone())
    };

    let mut queue = crate::services::radio::orchestrate_song(
        &db,
        lastfm.as_ref(),
        Some(&lastfm_similar_cache),
        payload.seed_track_id,
        blend,
        limit,
        &exclude,
    )
    .await
    .map_err(|e| {
        tracing::warn!("radio_song failed: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "radio orchestration failed" })),
        )
    })?;
    add_tidal_mix_fallback(
        &state,
        &db,
        TidalMixSeed::Track(payload.seed_track_id),
        &mut queue.tracks,
        limit,
    )
    .await;

    let (first_playable, pending_count) = build_radio_queue_and_spawn_resolvers(
        &state,
        &db,
        Some(payload.seed_track_id),
        queue.tracks.clone(),
        "radio_song",
    )
    .await?;
    remember_radio_seed(
        &db,
        crate::server::radio_continuation::RadioSeedKind::Track,
        payload.seed_track_id,
        blend,
    );
    let snapshot = start_first_radio_queue_item(&state).await?;
    let mut body = serde_json::to_value(queue).unwrap_or(json!({}));
    body["first_playable"] = first_playable;
    body["pending_count"] = json!(pending_count);
    body["state"] = json!(snapshot.state);
    body["queue"] = json!(snapshot.queue);
    Ok(Json(body))
}

#[derive(Debug, Deserialize)]
pub(super) struct RadioStartRequest {
    pub(super) seed_track_id: i64,
    #[serde(default)]
    pub(super) blend: Option<crate::services::radio::RadioBlend>,
    #[serde(default)]
    pub(super) limit: Option<usize>,
}

/// Seed for the TIDAL mix fallback, by local id.
#[derive(Clone, Copy)]
pub(crate) enum TidalMixSeed {
    Track(i64),
    Artist(i64),
}

/// A radio with fewer than `TIDAL_MIX_FALLBACK_MIN_PICKS` picks has a seed with
/// too little evidence of its own (never played, no genres or audio analysis,
/// no Last.fm match). Fill it from TIDAL's track or artist mix for the seed.
/// Best effort: no TIDAL session or no mix leaves the radio as it was.
pub(crate) async fn add_tidal_mix_fallback(
    state: &SharedState,
    db: &crate::db::Database,
    seed: TidalMixSeed,
    tracks: &mut Vec<crate::services::radio::RadioCandidate>,
    limit: usize,
) {
    use crate::services::radio::{
        TIDAL_MIX_FALLBACK_MIN_PICKS, tidal_mix_candidates, tidal_mix_id,
    };
    if tracks.len() >= TIDAL_MIX_FALLBACK_MIN_PICKS {
        return;
    }
    let (sql, local_id) = match seed {
        TidalMixSeed::Track(id) => ("SELECT tidal_id FROM tracks WHERE id = ?1", id),
        TidalMixSeed::Artist(id) => ("SELECT tidal_id FROM artists WHERE id = ?1", id),
    };
    let tidal_id = db
        .with_conn(|conn| {
            Ok(conn
                .query_row(sql, [local_id], |row| row.get::<_, Option<i64>>(0))
                .optional()?
                .flatten())
        })
        .ok()
        .flatten()
        .filter(|id| *id > 0);
    let Some(tidal_id) = tidal_id else { return };
    let Some(persisted) = load_persisted_tidal_tokens(state).await.ok().flatten() else {
        return;
    };
    let (tokens, tidal_session) = {
        let s = state.read().await;
        (s.tidal.tokens().unwrap_or(persisted), s.tidal.clone())
    };
    let client = TidalClient::for_session(tidal_session.clone(), &tokens.country_code)
        .with_metadata_store(db.clone());
    let mix_id = match seed {
        TidalMixSeed::Track(_) => client
            .get_track(tidal_id)
            .await
            .ok()
            .and_then(|track| tidal_mix_id(&track.extra, "TRACK_MIX")),
        TidalMixSeed::Artist(_) => client
            .get_artist(tidal_id)
            .await
            .ok()
            .and_then(|artist| tidal_mix_id(&artist.extra, "ARTIST_MIX")),
    };
    let Some(mix_id) = mix_id else { return };
    let mix = match client.get_mix_tracks(&mix_id).await {
        Ok(mix) => mix,
        Err(error) => {
            tracing::warn!(%error, mix_id, "radio: TIDAL mix fallback failed");
            return;
        }
    };
    let seed_tidal_id = matches!(seed, TidalMixSeed::Track(_)).then_some(tidal_id);
    let wanted = limit.saturating_sub(tracks.len());
    let existing = tracks.clone();
    let added = db
        .with_conn(move |conn| {
            Ok(tidal_mix_candidates(
                conn,
                mix,
                seed_tidal_id,
                &existing,
                wanted,
            ))
        })
        .unwrap_or_default();
    tracing::info!(
        before = tracks.len(),
        added = added.len(),
        "radio: seed had too little evidence; filled from TIDAL mix"
    );
    tracks.extend(added);
}

/// Remember which radio built the queue so topping it up continues that radio
/// (server::radio_continuation). Best effort: a failure only means automix
/// carries the queue on instead.
pub(super) fn remember_radio_seed(
    db: &crate::db::Database,
    kind: crate::server::radio_continuation::RadioSeedKind,
    id: i64,
    blend: crate::services::radio::RadioBlend,
) {
    let seed = crate::server::radio_continuation::RadioSeed { kind, id, blend };
    if let Err(error) = db.with_conn(|conn| {
        Ok(crate::server::radio_continuation::remember_seed(
            conn, seed,
        )?)
    }) {
        tracing::warn!(%error, "radio: could not remember the radio seed");
    }
}

pub(in crate::server) async fn build_radio_queue_and_spawn_resolvers(
    state: &SharedState,
    db: &crate::db::Database,
    seed_track_id: Option<i64>,
    tracks: Vec<crate::services::radio::RadioCandidate>,
    context: &'static str,
) -> Result<(Value, usize), (StatusCode, Json<Value>)> {
    let build = db
        .with_conn(move |conn| {
            Ok(
                crate::server::radio_pipeline::build_radio_queue_from_candidates_with_seed(
                    conn,
                    seed_track_id,
                    tracks,
                )?,
            )
        })
        .map_err(|e| {
            tracing::error!("{context}: queue build failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to build queue" })),
            )
        })?;
    let first_item = build.first_item;
    let pending_item_ids = build.pending_item_ids;
    let pending_count = pending_item_ids.len();

    let first_playable = match first_item {
        Some((queue_item_id, Some(track_id))) => json!({
            "type": "library",
            "queue_item_id": queue_item_id,
            "track_id": track_id
        }),
        Some((queue_item_id, None)) => json!({
            "type": "pending",
            "queue_item_id": queue_item_id,
            "track_id": null
        }),
        None => {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "error": "queue is empty after insert" })),
            ));
        }
    };

    spawn_pending_resolvers_for_queue_items(state, db, pending_item_ids, context).await;

    {
        let s = state.read().await;
        let _ = s.event_tx.send(AppEvent::QueueUpdated);
    }

    Ok((first_playable, pending_count))
}

pub(in crate::server) async fn spawn_pending_resolvers_for_queue_items(
    state: &SharedState,
    db: &crate::db::Database,
    pending_item_ids: Vec<i64>,
    context: &'static str,
) {
    if pending_item_ids.is_empty() {
        return;
    }

    let tokens_opt: Option<crate::services::tidal::auth::TidalTokens> = {
        let s = state.read().await;
        if let Some(t) = s.tidal.tokens() {
            Some(t)
        } else {
            drop(s);
            load_persisted_tidal_tokens(state).await.ok().flatten()
        }
    };

    if let Some(tokens) = tokens_opt {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(RESOLVER_POOL_SIZE));
        let event_tx = state.read().await.event_tx.clone();
        for item_id in pending_item_ids {
            let sem = semaphore.clone();
            let db_bg = db.clone();
            let tok = tokens.clone();
            let tx = event_tx.clone();
            let state_bg = state.clone();
            tokio::spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                if resolve_pending_row(state_bg.clone(), db_bg, tok, item_id, tx).await {
                    refresh_dj_after_queue_change(state_bg, context).await;
                }
            });
        }
    } else {
        tracing::warn!(
            "{context}: Tidal tokens unavailable - pending rows will rely on lazy resolution"
        );
    }
}

pub(in crate::server) async fn start_first_radio_queue_item(
    state: &SharedState,
) -> Result<player::PlaybackSnapshot, (StatusCode, Json<Value>)> {
    transport_command::start_queue_from_beginning(state)
        .await
        .map_err(|error| command_error_response(state, error))
}

pub(super) async fn radio_start(
    State(state): State<SharedState>,
    Json(payload): Json<RadioStartRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if payload.seed_track_id <= 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "seed_track_id must be a positive library id" })),
        ));
    }

    let blend = payload.blend.unwrap_or_default();
    let limit = payload.limit.unwrap_or(60).clamp(8, 200);

    let (db, lastfm, lastfm_similar_cache) = {
        let g = state.read().await;
        // User-driven radio start; reset post-clear suppression so the
        // freshly-built queue gets normal automix gating downstream.
        g.user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let lastfm = crate::metadata::lastfm::LastFmClient::load(g.http_client.clone(), &g.db);
        (g.db.clone(), lastfm, g.lastfm_similar_cache.clone())
    };

    let mut radio_queue = crate::services::radio::orchestrate_song(
        &db,
        lastfm.as_ref(),
        Some(&lastfm_similar_cache),
        payload.seed_track_id,
        blend,
        limit,
        &[],
    )
    .await
    .map_err(|e| {
        tracing::warn!("radio_start: orchestrate_song failed: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "radio orchestration failed" })),
        )
    })?;
    add_tidal_mix_fallback(
        &state,
        &db,
        TidalMixSeed::Track(payload.seed_track_id),
        &mut radio_queue.tracks,
        limit,
    )
    .await;

    // Build queue atomically and collect pending row IDs for background tasks.
    let seed_track_id = payload.seed_track_id;
    let build = db
        .with_conn(move |conn| {
            Ok(
                crate::server::radio_pipeline::build_radio_queue_from_candidates(
                    conn,
                    seed_track_id,
                    radio_queue.tracks,
                )?,
            )
        })
        .map_err(|e| {
            tracing::error!("radio_start: queue build failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to build queue" })),
            )
        })?;
    remember_radio_seed(
        &db,
        crate::server::radio_continuation::RadioSeedKind::Track,
        seed_track_id,
        blend,
    );
    let first_item = build.first_item;
    let pending_item_ids = build.pending_item_ids;
    let pending_count = pending_item_ids.len();

    let first_playable = match first_item {
        Some((queue_item_id, Some(track_id))) => json!({
            "type": "library",
            "queue_item_id": queue_item_id,
            "track_id": track_id
        }),
        Some((queue_item_id, None)) => json!({
            "type": "pending",
            "queue_item_id": queue_item_id,
            "track_id": null
        }),
        None => {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "error": "queue is empty after insert" })),
            ));
        }
    };

    // Spawn bounded background resolvers for all pending rows.
    if !pending_item_ids.is_empty() {
        let tokens_opt: Option<crate::services::tidal::auth::TidalTokens> = {
            let s = state.read().await;
            if let Some(t) = s.tidal.tokens() {
                Some(t)
            } else {
                drop(s);
                load_persisted_tidal_tokens(&state).await.ok().flatten()
            }
        };

        if let Some(tokens) = tokens_opt {
            let semaphore = Arc::new(tokio::sync::Semaphore::new(RESOLVER_POOL_SIZE));
            let event_tx = state.read().await.event_tx.clone();
            for item_id in pending_item_ids {
                let sem = semaphore.clone();
                let db_bg = db.clone();
                let tok = tokens.clone();
                let tx = event_tx.clone();
                let state_bg = state.clone();
                tokio::spawn(async move {
                    let _permit = sem.acquire_owned().await.ok();
                    if resolve_pending_row(state_bg.clone(), db_bg, tok, item_id, tx).await {
                        refresh_dj_after_queue_change(state_bg, "radio_start_pending_resolved")
                            .await;
                    }
                });
            }
        } else {
            tracing::warn!(
                "radio_start: Tidal tokens unavailable - pending rows will rely on lazy resolution"
            );
        }
    }

    {
        let s = state.read().await;
        let _ = s.event_tx.send(AppEvent::QueueUpdated);
    }

    let snapshot = start_first_radio_queue_item(&state).await?;

    Ok(Json(json!({
        "first_playable": first_playable,
        "pending_count": pending_count,
        "state": snapshot.state,
        "queue": snapshot.queue,
        "queue_revision": snapshot.queue_revision
    })))
}

#[derive(Debug, Deserialize)]
pub(super) struct RadioAlbumRequest {
    pub(super) seed_album_id: i64,
    #[serde(default)]
    pub(super) blend: Option<crate::services::radio::RadioBlend>,
    #[serde(default)]
    pub(super) limit: Option<usize>,
    #[serde(default)]
    pub(super) exclude_track_ids: Option<Vec<i64>>,
}

pub(super) async fn radio_album(
    State(state): State<SharedState>,
    Json(payload): Json<RadioAlbumRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let blend = payload.blend.unwrap_or_default();
    let limit = payload.limit.unwrap_or(60).clamp(8, 200);
    let exclude = payload.exclude_track_ids.unwrap_or_default();

    let (db, lastfm, lastfm_similar_cache) = {
        let g = state.read().await;
        g.user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let lastfm = crate::metadata::lastfm::LastFmClient::load(g.http_client.clone(), &g.db);
        (g.db.clone(), lastfm, g.lastfm_similar_cache.clone())
    };

    let queue = crate::services::radio::orchestrate_album(
        &db,
        lastfm.as_ref(),
        Some(&lastfm_similar_cache),
        payload.seed_album_id,
        blend,
        limit,
        &exclude,
    )
    .await
    .map_err(|e| {
        tracing::warn!("radio_album failed: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "radio orchestration failed" })),
        )
    })?;

    let (first_playable, pending_count) = build_radio_queue_and_spawn_resolvers(
        &state,
        &db,
        None,
        queue.tracks.clone(),
        "radio_album",
    )
    .await?;
    remember_radio_seed(
        &db,
        crate::server::radio_continuation::RadioSeedKind::Album,
        payload.seed_album_id,
        blend,
    );
    let snapshot = start_first_radio_queue_item(&state).await?;
    let mut body = serde_json::to_value(queue).unwrap_or(json!({}));
    body["first_playable"] = first_playable;
    body["pending_count"] = json!(pending_count);
    body["state"] = json!(snapshot.state);
    body["queue"] = json!(snapshot.queue);
    Ok(Json(body))
}

#[derive(Debug, Deserialize)]
pub(super) struct RadioArtistRequest {
    pub(super) seed_artist_id: i64,
    #[serde(default)]
    pub(super) blend: Option<crate::services::radio::RadioBlend>,
    #[serde(default)]
    pub(super) limit: Option<usize>,
    #[serde(default)]
    pub(super) exclude_track_ids: Option<Vec<i64>>,
}

pub(super) async fn radio_artist(
    State(state): State<SharedState>,
    Json(payload): Json<RadioArtistRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let blend = payload.blend.unwrap_or_default();
    let limit = payload.limit.unwrap_or(60).clamp(8, 200);
    let exclude = payload.exclude_track_ids.unwrap_or_default();

    let (db, lastfm, lastfm_similar_cache) = {
        let g = state.read().await;
        g.user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let lastfm = crate::metadata::lastfm::LastFmClient::load(g.http_client.clone(), &g.db);
        (g.db.clone(), lastfm, g.lastfm_similar_cache.clone())
    };

    let mut queue = crate::services::radio::orchestrate_artist(
        &db,
        lastfm.as_ref(),
        Some(&lastfm_similar_cache),
        payload.seed_artist_id,
        blend,
        limit,
        &exclude,
    )
    .await
    .map_err(|e| {
        tracing::warn!("radio_artist failed: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "radio orchestration failed" })),
        )
    })?;
    add_tidal_mix_fallback(
        &state,
        &db,
        TidalMixSeed::Artist(payload.seed_artist_id),
        &mut queue.tracks,
        limit,
    )
    .await;

    let (first_playable, pending_count) = build_radio_queue_and_spawn_resolvers(
        &state,
        &db,
        None,
        queue.tracks.clone(),
        "radio_artist",
    )
    .await?;
    remember_radio_seed(
        &db,
        crate::server::radio_continuation::RadioSeedKind::Artist,
        payload.seed_artist_id,
        blend,
    );
    let snapshot = start_first_radio_queue_item(&state).await?;
    let mut body = serde_json::to_value(queue).unwrap_or(json!({}));
    body["first_playable"] = first_playable;
    body["pending_count"] = json!(pending_count);
    body["state"] = json!(snapshot.state);
    body["queue"] = json!(snapshot.queue);
    Ok(Json(body))
}
