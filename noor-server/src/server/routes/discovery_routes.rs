use crate::SharedState;
use crate::db::queries;
use crate::metadata::lastfm::LastFmClient;
use crate::services::discovery::{DiscoveryCandidateSeed, DiscoveryProvider};
use crate::services::learning as discovery_learning;
use crate::services::tidal::client::TidalClient;
use crate::smart::discovery as discovery_engine;
use crate::smart::external_discovery as external_discovery_engine;
use axum::{extract::State, http::StatusCode, response::Json};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
pub(super) struct DiscoveryPreviewRequest {
    prompt: String,
    mode: Option<String>,
    services: Option<Vec<String>>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DiscoveryPresetRequest {
    name: String,
    prompt: String,
    mode: Option<String>,
    services: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DiscoveryExternalRequest {
    prompt: String,
    mode: Option<String>,
    services: Option<Vec<String>>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DiscoveryConnectionsRequest {
    prompt: String,
    mode: Option<String>,
    services: Option<Vec<String>>,
    limit: Option<i64>,
    seed: super::DiscoveryExternalResultRequest,
}

#[derive(Debug, Deserialize)]
pub(super) struct DiscoveryTrainRequest {
    mode: Option<String>,
    rebuild_audio: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DiscoveryFeedbackRequest {
    seed_track_id: i64,
    candidate_track_id: i64,
    action: String,
    surface: String,
    context: Option<Value>,
    #[serde(default)]
    session_id: Option<String>,
}

fn parse_discovery_training_mode(mode: Option<&str>) -> Result<(&'static str, bool), StatusCode> {
    let Some(mode) = mode else {
        return Ok(("incremental", false));
    };
    match mode.trim().to_ascii_lowercase().as_str() {
        "incremental" => Ok(("incremental", false)),
        "full" => Ok(("full", true)),
        "" => Err(StatusCode::BAD_REQUEST),
        _ => Err(StatusCode::BAD_REQUEST),
    }
}

fn parse_discovery_intensity_request(
    intensity: &str,
) -> Result<discovery_learning::DiscoveryIntensity, StatusCode> {
    match intensity.trim().to_ascii_lowercase().as_str() {
        "max" => Ok(discovery_learning::DiscoveryIntensity::Max),
        "medium" => Ok(discovery_learning::DiscoveryIntensity::Medium),
        "low" => Ok(discovery_learning::DiscoveryIntensity::Low),
        _ => Err(StatusCode::BAD_REQUEST),
    }
}

fn parse_discovery_engine_request(
    engine: &str,
) -> Result<discovery_learning::DiscoveryEngine, StatusCode> {
    match engine.trim().to_ascii_lowercase().as_str() {
        "v2" => Ok(discovery_learning::DiscoveryEngine::V2),
        "v1" | "legacy" => Ok(discovery_learning::DiscoveryEngine::V1),
        _ => Err(StatusCode::BAD_REQUEST),
    }
}

fn parse_discovery_safety_profile_request(
    profile: &str,
) -> Result<discovery_learning::DiscoveryTrainingSafetyProfile, StatusCode> {
    match profile.trim().to_ascii_lowercase().as_str() {
        "laptop_safe" | "laptop-safe" | "safe" => {
            Ok(discovery_learning::DiscoveryTrainingSafetyProfile::LaptopSafe)
        }
        "balanced" => Ok(discovery_learning::DiscoveryTrainingSafetyProfile::Balanced),
        "performance" | "fast" => {
            Ok(discovery_learning::DiscoveryTrainingSafetyProfile::Performance)
        }
        _ => Err(StatusCode::BAD_REQUEST),
    }
}

pub(super) async fn preview_discovery(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryPreviewRequest>,
) -> Result<Json<Value>, StatusCode> {
    let prompt = payload.prompt.trim();
    if prompt.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let services = payload
        .services
        .clone()
        .unwrap_or_else(|| vec!["tidal".to_string()]);
    let mode = super::normalize_discovery_mode(payload.mode.as_deref());
    let candidate_limit = payload.limit.unwrap_or(18).clamp(1, 40);
    let result_limit = payload.limit.unwrap_or(8).clamp(1, 20) as usize;

    let state = state.read().await;
    let recent_similar = state
        .db
        .with_conn(|conn| queries::get_similar_tracks(conn, 1, 5, &[]))
        .unwrap_or_default();
    if let Ok(Some(preview)) = discovery_learning::build_prompt_preview(
        &state.db,
        prompt,
        &mode,
        &services,
        result_limit,
        &recent_similar,
    ) {
        return Ok(Json(json!({ "preview": preview })));
    }

    let preview = state
        .db
        .with_conn(|conn| {
            let request = discovery_engine::DiscoveryPreviewRequest {
                prompt: prompt.to_string(),
                mode,
                services,
                limit: result_limit,
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
            let candidates = queries::get_discovery_candidate_tracks(conn, candidate_limit)?;
            let preview = discovery_engine::build_preview(&request, &context, &candidates);
            queries::cache_discovery_results(conn, None, &preview.results)?;
            Ok(preview)
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({ "preview": preview })))
}

pub(super) async fn discover_new_music(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryExternalRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let prompt = payload.prompt.trim();
    if prompt.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "prompt_required",
                "message": "Add a few words first so NOOR can search outward.",
            })),
        ));
    }

    let mode = super::normalize_discovery_mode(payload.mode.as_deref());
    let services = super::normalize_discovery_services(payload.services);
    if !services.iter().any(|service| service == "tidal") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "tidal_required",
                "message": "TIDAL must stay selected for real new-music discovery right now.",
            })),
        ));
    }

    let request = external_discovery_engine::ExternalDiscoveryRequest {
        prompt: prompt.to_string(),
        mode,
        services,
        limit: payload.limit.unwrap_or(10).clamp(1, 20) as usize,
    };
    let context = super::load_external_discovery_context(&state)
        .await
        .map_err(super::internal_discovery_error)?;
    let queries = external_discovery_engine::build_search_queries(&request, &context);
    let queries =
        super::augment_search_queries_with_lastfm(&state, &request, &context, queries).await;
    let provider = super::tidal_discovery_provider(&state).await?;
    let candidates = provider
        .search_tracks(&queries, 10)
        .await
        .map_err(super::discovery_upstream_error)?;
    let candidates = super::enrich_candidates_with_metadata(&state, candidates).await;
    let embedding_scores = discovery_learning::compute_external_embedding_scores(
        &{
            let guard = state.read().await;
            guard.db.clone()
        },
        prompt,
        &candidates,
    )
    .unwrap_or_default();
    let library_tidal_ids = super::existing_candidate_tidal_ids(&state, &candidates)
        .await
        .map_err(super::internal_discovery_error)?;
    let mut feed = external_discovery_engine::build_external_feed(
        &request,
        &context,
        &candidates,
        &library_tidal_ids,
        super::discovery_provider_capabilities(),
        None,
    );
    for result in &mut feed.results {
        result.embedding_score = embedding_scores.get(&result.provider_track_id).copied();
        if let Some(score) = result.embedding_score {
            result.score = (((result.score as f64) * 0.8) + (score.max(0.0) * 20.0)).round() as i32;
            if score > 0.2 {
                result.tags.push("embedding boost".to_string());
            }
        }
    }
    feed.results
        .sort_by(|left, right| right.score.cmp(&left.score));

    Ok(Json(json!({ "feed": feed })))
}

pub(super) async fn save_discovery_track(
    State(state): State<SharedState>,
    Json(payload): Json<super::DiscoveryExternalResultRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let provider = super::normalize_external_provider(&payload.provider).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "unsupported_provider",
                "message": "That discovery provider is not supported yet.",
            })),
        )
    })?;

    if provider != "tidal" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "unsupported_provider",
                "message": "Only TIDAL discovery saves are wired up right now.",
            })),
        ));
    }

    let provider = super::tidal_discovery_provider(&state).await?;
    provider
        .save_track(&payload.provider_track_id)
        .await
        .map_err(super::discovery_upstream_error)?;

    Ok(Json(json!({
        "saved": true,
        "provider": "tidal",
        "provider_track_id": payload.provider_track_id,
        "message": format!("Saved \"{}\" to TIDAL favorites. Run sync to pull it fully into NOOR.", payload.title),
    })))
}

pub(super) async fn discover_connected_music(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryConnectionsRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let prompt = payload.prompt.trim();
    if prompt.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "prompt_required",
                "message": "Keep a prompt in play so NOOR can connect the next songs.",
            })),
        ));
    }

    let provider_name =
        super::normalize_external_provider(&payload.seed.provider).ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "unsupported_provider",
                    "message": "That discovery provider is not supported yet.",
                })),
            )
        })?;
    if provider_name != "tidal" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "unsupported_provider",
                "message": "Only TIDAL connection trails are wired up right now.",
            })),
        ));
    }

    let request = external_discovery_engine::ExternalDiscoveryRequest {
        prompt: prompt.to_string(),
        mode: super::normalize_discovery_mode(payload.mode.as_deref()),
        services: super::normalize_discovery_services(payload.services),
        limit: payload.limit.unwrap_or(10).clamp(1, 20) as usize,
    };
    let context = super::load_external_discovery_context(&state)
        .await
        .map_err(super::internal_discovery_error)?;
    let seed = DiscoveryCandidateSeed {
        provider_track_id: payload.seed.provider_track_id.clone(),
        title: payload.seed.title.clone(),
        artist_name: payload.seed.artist_name.clone(),
        album_title: payload.seed.album_title.clone(),
        normalized_genres: payload.seed.normalized_genres.clone().unwrap_or_default(),
    };
    let queries = external_discovery_engine::build_connection_queries(&request, &context, &seed);
    let queries = super::augment_connection_queries_with_lastfm(&state, &seed, queries).await;
    let provider = super::tidal_discovery_provider(&state).await?;
    let candidates = provider
        .connected_tracks(&seed, &queries, 8)
        .await
        .map_err(super::discovery_upstream_error)?
        .into_iter()
        .filter(|candidate| candidate.provider_track_id != seed.provider_track_id)
        .collect::<Vec<_>>();
    let candidates = super::enrich_candidates_with_metadata(&state, candidates).await;
    let embedding_scores = discovery_learning::compute_external_embedding_scores(
        &{
            let guard = state.read().await;
            guard.db.clone()
        },
        prompt,
        &candidates,
    )
    .unwrap_or_default();
    let library_tidal_ids = super::existing_candidate_tidal_ids(&state, &candidates)
        .await
        .map_err(super::internal_discovery_error)?;
    let trail_item = Some(super::discovery_request_to_trail_item(&payload.seed));
    let mut feed = external_discovery_engine::build_external_feed(
        &request,
        &context,
        &candidates,
        &library_tidal_ids,
        super::discovery_provider_capabilities(),
        trail_item,
    );
    for result in &mut feed.results {
        result.embedding_score = embedding_scores.get(&result.provider_track_id).copied();
        if let Some(score) = result.embedding_score {
            result.score = (((result.score as f64) * 0.8) + (score.max(0.0) * 20.0)).round() as i32;
            if score > 0.2 {
                result.tags.push("embedding boost".to_string());
            }
        }
    }
    feed.results
        .sort_by(|left, right| right.score.cmp(&left.score));

    Ok(Json(json!({ "feed": feed })))
}

pub(super) async fn get_discovery_presets(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    let presets = state
        .db
        .with_conn(queries::list_discovery_presets)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({ "presets": presets })))
}

pub(super) async fn create_discovery_preset(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryPresetRequest>,
) -> Result<Json<Value>, StatusCode> {
    let name = payload.name.trim();
    let prompt = payload.prompt.trim();
    if name.is_empty() || prompt.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mode = super::normalize_discovery_mode(payload.mode.as_deref());

    let services_json = serde_json::to_string(
        &payload
            .services
            .unwrap_or_else(|| vec!["tidal".to_string()]),
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let state = state.read().await;
    let preset = state
        .db
        .with_conn(|conn| {
            queries::create_discovery_preset(conn, name, prompt, &mode, &services_json)
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({ "preset": preset })))
}

pub(super) async fn get_discovery_status(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    let status = state
        .db
        .with_conn(queries::get_discovery_status)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "status": status })))
}

pub(super) async fn get_discovery_training_status(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    let run = state
        .db
        .with_conn(queries::get_latest_training_run)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Build a synthetic per-stage breakdown so the frontend can render a
    // pipeline view without needing multi-row stage history in the schema.
    const STAGE_ORDER: &[&str] = &[
        "corpus",
        "behavioral",
        "audio",
        "fusion",
        "neighbors",
        "evaluate",
    ];
    const STAGE_THRESHOLDS: &[f64] = &[0.05, 0.2, 0.55, 0.72, 0.88, 0.96];

    let stages: Vec<Value> = if let Some(ref r) = run {
        let current_stage_idx = STAGE_ORDER.iter().position(|&s| s == r.stage).unwrap_or(0);
        STAGE_ORDER
            .iter()
            .enumerate()
            .map(|(i, &name)| {
                let stage_status = if r.status == "failed" && i == current_stage_idx {
                    "failed"
                } else if i < current_stage_idx {
                    "done"
                } else if i == current_stage_idx {
                    r.status.as_str()
                } else {
                    "pending"
                };
                let progress = if i < current_stage_idx {
                    1.0_f64
                } else if i == current_stage_idx {
                    let lo = if i == 0 { 0.0 } else { STAGE_THRESHOLDS[i - 1] };
                    let hi = STAGE_THRESHOLDS[i];
                    ((r.progress - lo) / (hi - lo)).clamp(0.0, 1.0)
                } else {
                    0.0_f64
                };
                json!({ "stage": name, "status": stage_status, "progress": progress })
            })
            .collect()
    } else {
        Vec::new()
    };

    Ok(Json(json!({ "run": run, "stages": stages })))
}

pub(super) async fn start_discovery_training(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryTrainRequest>,
) -> Result<Json<Value>, StatusCode> {
    use std::sync::atomic::Ordering;

    let (mode, full_mode) = parse_discovery_training_mode(payload.mode.as_deref())?;
    let rebuild_audio = payload.rebuild_audio.unwrap_or(false);
    let (db, cancel) = {
        let guard = state.read().await;
        (guard.db.clone(), guard.discovery_train_cancel.clone())
    };

    // Guard: reject if a run is already in progress
    let already_running = db
        .with_conn(queries::get_latest_training_run)
        .ok()
        .flatten()
        .map(|run| run.status == "running")
        .unwrap_or(false);

    if already_running {
        return Ok(Json(json!({
            "status": "already_running",
            "mode": mode
        })));
    }

    let engine = discovery_learning::load_discovery_engine(&db);
    if !engine.supports_training() {
        return Ok(Json(json!({
            "status": "legacy_trainer_unavailable",
            "mode": mode,
            "engine": engine.as_str(),
            "message": "V1 legacy can read existing models. Switch to V2 to train a new model."
        })));
    }

    // Reset cancel flag synchronously before spawning so that a Stop request
    // arriving immediately after this call reaches the spawned task.
    cancel.store(false, Ordering::SeqCst);

    tokio::spawn(async move {
        let (event_tx, http_client, tidal_http_client, tidal_tokens) = {
            let guard = state.read().await;
            (
                guard.event_tx.clone(),
                guard.http_client.clone(),
                guard.tidal_http_client.clone(),
                guard.tidal_tokens.clone(),
            )
        };
        let lastfm = LastFmClient::load(http_client, &db);
        let tokens = match tidal_tokens {
            Some(tokens) => Some(tokens),
            None => super::load_persisted_tidal_tokens(&state)
                .await
                .ok()
                .flatten(),
        };
        let tidal = tokens.map(|tokens| {
            TidalClient::with_http(tidal_http_client, tokens.access_token, tokens.country_code)
        });
        let external_refresh_clients =
            discovery_learning::ExternalProviderRefreshClients { lastfm, tidal };
        if let Err(error) = discovery_learning::start_training(
            db,
            event_tx,
            full_mode,
            rebuild_audio,
            cancel,
            external_refresh_clients,
        )
        .await
        {
            tracing::error!(
                target: "noor.discovery.training",
                error = %error,
                "discovery learning pipeline failed"
            );
        }
    });
    Ok(Json(json!({
        "status": "training_started",
        "mode": if full_mode { "full" } else { "incremental" }
    })))
}

pub(super) async fn stop_discovery_training(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use std::sync::atomic::Ordering;
    let s = state.read().await;
    s.discovery_train_cancel.store(true, Ordering::Relaxed);
    Ok(Json(json!({ "status": "stopping" })))
}

pub(super) async fn get_discovery_intensity(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::learning::{DiscoveryIntensity, load_discovery_intensity};
    let s = state.read().await;
    let intensity = load_discovery_intensity(&s.db);
    let params = intensity.params();
    Ok(Json(json!({
        "intensity": intensity.as_str(),
        "dimension": params.dimension,
        "top_k": params.top_k,
        "window_size": params.window_size,
        "include_audio_proxy": params.include_audio_proxy,
        "available": [
            DiscoveryIntensity::Max.as_str(),
            DiscoveryIntensity::Medium.as_str(),
            DiscoveryIntensity::Low.as_str(),
        ],
    })))
}

#[derive(Debug, Deserialize)]
pub(super) struct IntensityRequest {
    intensity: String,
}

pub(super) async fn set_discovery_intensity(
    State(state): State<SharedState>,
    Json(payload): Json<IntensityRequest>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::learning::set_discovery_intensity as save_intensity;
    let s = state.read().await;
    let parsed = parse_discovery_intensity_request(&payload.intensity)?;
    save_intensity(&s.db, parsed).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "intensity": parsed.as_str() })))
}

pub(super) async fn get_discovery_engine(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::learning::DiscoveryEngine;
    let s = state.read().await;
    let engine = discovery_learning::load_discovery_engine(&s.db);
    Ok(Json(json!({
        "engine": engine.as_str(),
        "label": engine.label(),
        "family": engine.family(),
        "trainable": engine.supports_training(),
        "available": [
            DiscoveryEngine::V2.as_str(),
            DiscoveryEngine::V1.as_str(),
        ],
    })))
}

#[derive(Debug, Deserialize)]
pub(super) struct EngineRequest {
    engine: String,
}

pub(super) async fn set_discovery_engine(
    State(state): State<SharedState>,
    Json(payload): Json<EngineRequest>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    let parsed = parse_discovery_engine_request(&payload.engine)?;
    discovery_learning::set_discovery_engine(&s.db, parsed)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({
        "engine": parsed.as_str(),
        "label": parsed.label(),
        "family": parsed.family(),
        "trainable": parsed.supports_training(),
    })))
}

pub(super) async fn get_discovery_safety_profile(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::learning::{
        DiscoveryTrainingSafetyProfile, discovery_training_worker_threads,
        load_discovery_training_safety_profile,
    };
    let s = state.read().await;
    let profile = load_discovery_training_safety_profile(&s.db);
    Ok(Json(json!({
        "profile": profile.as_str(),
        "label": profile.label(),
        "worker_threads": discovery_training_worker_threads(profile),
        "available": [
            DiscoveryTrainingSafetyProfile::LaptopSafe.as_str(),
            DiscoveryTrainingSafetyProfile::Balanced.as_str(),
            DiscoveryTrainingSafetyProfile::Performance.as_str(),
        ],
    })))
}

#[derive(Debug, Deserialize)]
pub(super) struct SafetyProfileRequest {
    profile: String,
}

pub(super) async fn set_discovery_safety_profile(
    State(state): State<SharedState>,
    Json(payload): Json<SafetyProfileRequest>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::learning::{
        discovery_training_worker_threads, set_discovery_training_safety_profile,
    };
    let s = state.read().await;
    let parsed = parse_discovery_safety_profile_request(&payload.profile)?;
    set_discovery_training_safety_profile(&s.db, parsed)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({
        "profile": parsed.as_str(),
        "label": parsed.label(),
        "worker_threads": discovery_training_worker_threads(parsed),
    })))
}

// Safety estimate: tells the UI how long training is expected to take and
// how much memory it'll claim, derived from the current track count, the
// active intensity tier, and the duration of the most recent successful run
// (if any). Frontend uses this to gate the user with a "this'll take ~X min"
// preview before they hit Start.
pub(super) async fn get_discovery_safety(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::learning::{
        discovery_training_safety_timeout, discovery_training_worker_threads,
        load_discovery_intensity, load_discovery_training_safety_profile,
    };
    let s = state.read().await;

    let (track_count, last_run_seconds): (i64, Option<f64>) =
        s.db.with_conn(|conn| {
            let tracks: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM tracks WHERE source IS NOT NULL",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            // Most recent finished run, in seconds. SQLite's strftime epoch
            // gives integer seconds; subtraction is the wall-clock duration.
            let last: Option<f64> = conn
                .query_row(
                    "SELECT (julianday(finished_at) - julianday(started_at)) * 86400.0
                     FROM training_runs
                     WHERE finished_at IS NOT NULL AND status = 'completed'
                     ORDER BY id DESC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .ok();
            Ok((tracks, last))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let intensity = load_discovery_intensity(&s.db);
    let params = intensity.params();
    let safety_profile = load_discovery_training_safety_profile(&s.db);
    let safety_timeout_seconds = discovery_training_safety_timeout(intensity).as_secs();
    let worker_threads = discovery_training_worker_threads(safety_profile);

    // Cost model: similarity_neighbors is O(n^2) on track count. Constant
    // factor scales roughly with `dim * top_k`. Calibrated against the
    // observed Max-tier baseline of ~12 minutes for 30,000 tracks (~0.8 microseconds
    // per pair on a typical laptop). Final fudge factor includes I/O,
    // co-occurrence build, and audio-proxy overhead.
    let n = track_count as f64;
    let pair_cost_ns = 800.0 * (params.dimension as f64 / 96.0) * (params.top_k as f64 / 64.0);
    let neighbors_seconds = (n * n * pair_cost_ns) / 1.0e9;
    let audio_seconds = if params.include_audio_proxy {
        n * 0.0008
    } else {
        0.0
    };
    let behavioral_seconds = n * 0.001;
    let estimated_seconds_model = neighbors_seconds + audio_seconds + behavioral_seconds;

    // Prefer the actual last-run duration if we have one - it captures the
    // user's real machine and library. Blend 70/30 with the model so we
    // don't anchor too hard on a single noisy datapoint.
    let estimated_seconds = match last_run_seconds {
        Some(observed) if observed > 5.0 => 0.3 * estimated_seconds_model + 0.7 * observed,
        _ => estimated_seconds_model,
    };

    // Peak RAM rough estimate: dim * N * 8 bytes for behavioral vectors,
    // doubled for audio + fusion, plus the neighbor graph (top_k * N * 32).
    let ram_mb = ((params.dimension as f64 * n * 8.0 * 3.0) + (params.top_k as f64 * n * 32.0))
        / (1024.0 * 1024.0);

    // Safety classification: Green when the run is short or matches a known
    // baseline. Yellow when we expect 5-20 min on a non-trivial library.
    // Red when we predict over 20 min or RAM crosses 1.5 GB - these are the
    // cases where the user should consider dropping intensity.
    let recommendation = if estimated_seconds > 1200.0 || ram_mb > 1500.0 {
        "high_cost"
    } else if estimated_seconds > 300.0 {
        "moderate"
    } else {
        "safe"
    };

    Ok(Json(json!({
        "track_count": track_count,
        "intensity": intensity.as_str(),
        "estimated_seconds": estimated_seconds.round() as i64,
        "estimated_minutes": (estimated_seconds / 60.0 * 10.0).round() / 10.0,
        "estimated_ram_mb": ram_mb.round() as i64,
        "last_run_seconds": last_run_seconds.map(|s| s.round() as i64),
        "recommendation": recommendation,
        "safety_profile": safety_profile.as_str(),
        "safety_timeout_seconds": safety_timeout_seconds,
        "worker_threads": worker_threads,
        "params": {
            "dimension": params.dimension,
            "top_k": params.top_k,
            "window_size": params.window_size,
            "include_audio_proxy": params.include_audio_proxy,
        },
    })))
}

pub(super) async fn record_discovery_feedback(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryFeedbackRequest>,
) -> Result<Json<Value>, StatusCode> {
    let context_json = payload.context.as_ref().map(Value::to_string);
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            queries::record_discovery_feedback(
                conn,
                payload.seed_track_id,
                payload.candidate_track_id,
                &payload.action,
                &payload.surface,
                context_json.as_deref(),
                payload.session_id.as_deref(),
            )
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({ "recorded": true })))
}
