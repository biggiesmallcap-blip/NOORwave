use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use chrono::Utc;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64};

use crate::SharedState;
use crate::db::{
    models::{AudioDjProfileCorrectionRow, AudioDjProfileKey, AudioDjProfileRow},
    queries,
};
use crate::playback::decode::decode_and_buffer_job;
use crate::playback::dj_lookahead::DjMediaRef;
use crate::playback::gapless::GaplessPlan;
use crate::playback::player::{self, PlaybackSourceKind, PlaybackSourceRequest};
use crate::playback::runtime::PlaybackRuntimeConfig;
use crate::playback::runtime::commands::PlaybackRuntimeCommand;
use crate::playback::runtime::shared::PlaybackSharedState;
use crate::services::audio_analysis::dj_profile::{
    DJ_PROFILE_VERSION, decode_f32_blob, decode_u32_blob,
};
use crate::services::tidal::stream as tidal_stream;

const DEFAULT_DJ_LOOKAHEAD_DEADLINE_SAMPLES: u64 = 48_000 * 30;
const DJ_PROFILE_CONFIDENCE_FLOOR: f64 = 0.65;
const SAFE_SUGGESTION_BAD_COUNT: i64 = 3;
const DJ_PROFILE_AUTO_REBUILD_RETRY_SECS: u64 = 300;
const DJ_TIMING_HISTORY_LIMIT: i64 = 5;

pub fn routes() -> Router<SharedState> {
    Router::new()
        .route("/api/dj/enabled", get(get_enabled).put(set_enabled))
        .route("/api/dj/status", get(get_status))
        .route("/api/dj/profile/{track_id}", get(get_profile))
        .route("/api/dj/profile-rebuild", post(rebuild_profile))
        .route("/api/dj/profile-correction", post(set_profile_correction))
        .route(
            "/api/dj/profile-correction/{kind}/{id}",
            get(get_profile_correction),
        )
        .route("/api/dj/policy", get(get_policy).put(set_policy))
        .route(
            "/api/dj/mix-intent",
            get(get_mix_intent).put(set_mix_intent),
        )
        .route("/api/dj/feedback", post(record_feedback))
}

#[derive(Debug, Serialize)]
struct EnabledResponse {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
struct SetEnabledRequest {
    enabled: bool,
}

#[derive(Debug, Serialize)]
struct ProfileResponse {
    track_id: i64,
    profile_version: String,
    beat_count: usize,
    downbeat_count: usize,
    phrase_count: usize,
}

#[derive(Debug, Deserialize)]
struct SetMixIntentRequest {
    intent: String,
}

#[derive(Debug, Serialize)]
struct MixIntentResponse {
    intent: String,
}

#[derive(Debug, Deserialize)]
struct DjFeedbackRequest {
    transition_event_id: Option<i64>,
    rating: String,
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DjProfileCorrectionRequest {
    media_ref_kind: String,
    media_ref_id: String,
    bpm_multiplier: Option<f64>,
    downbeat_offset_beats: Option<i64>,
    phrase_offset_bars: Option<i64>,
    safe_crossfade_only: Option<bool>,
    transition_speed_bias: Option<String>,
    notes: Option<String>,
}

#[derive(Debug, Serialize)]
struct DjProfileCorrectionResponse {
    media_ref_kind: String,
    media_ref_id: String,
    bpm_multiplier: Option<f64>,
    downbeat_offset_beats: Option<i64>,
    phrase_offset_bars: Option<i64>,
    safe_crossfade_only: bool,
    transition_speed_bias: Option<String>,
    notes: Option<String>,
    applies: String,
}

#[derive(Debug, Deserialize)]
struct SetDjPolicyRequest {
    mix_intent: Option<String>,
    transition_speed_bias: Option<String>,
}

#[derive(Debug, Serialize)]
struct DjPolicyResponse {
    mix_intent: String,
    transition_speed_bias: String,
}

#[derive(Debug, Serialize)]
struct DjStatusResponse {
    enabled: bool,
    current: Option<DjDeckStatus>,
    next: Option<DjDeckStatus>,
    selected_program: Option<String>,
    planned_template: Option<String>,
    renderer_template: Option<String>,
    renderer_mode: Option<String>,
    downgrade_reason: Option<String>,
    planning_reason: Option<String>,
    sync_target: Option<String>,
    planned_start_ms: Option<i64>,
    actual_start_ms: Option<i64>,
    timing_delta_ms: Option<i64>,
    timing_source: Option<String>,
    timing_status: Option<String>,
    timing_quality: String,
    timing_direction: String,
    fallback_reason: Option<String>,
    profile_confidence_floor: f64,
    last_transition_event_id: Option<i64>,
    recent_timing_events: Vec<DjTimingHistoryEvent>,
    timing_history_summary: DjTimingHistorySummary,
    safe_crossfade_suggestion: Option<DjSafeSuggestion>,
}

#[derive(Debug, Serialize)]
struct DjDeckStatus {
    media_ref_kind: String,
    media_ref_id: String,
    title: String,
    artist: Option<String>,
    profile_ready: bool,
    profile_confidence: Option<f64>,
    beat_count: Option<usize>,
    downbeat_count: Option<usize>,
    phrase_count: Option<usize>,
    safe_crossfade_only: bool,
}

#[derive(Debug, Serialize)]
struct DjSafeSuggestion {
    media_ref_kind: String,
    media_ref_id: String,
    bad_feedback_count: i64,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct DjTimingHistoryEvent {
    event_id: i64,
    from_title: Option<String>,
    from_artist: Option<String>,
    to_title: Option<String>,
    to_artist: Option<String>,
    planned_template: String,
    renderer_template: Option<String>,
    planning_reason: Option<String>,
    planned_start_ms: Option<i64>,
    actual_start_ms: Option<i64>,
    timing_delta_ms: Option<i64>,
    timing_source: Option<String>,
    timing_status: Option<String>,
    timing_quality: String,
    timing_direction: String,
    started_at: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct DjTimingHistorySummary {
    event_count: usize,
    average_delta_ms: Option<i64>,
    average_abs_delta_ms: Option<i64>,
    tight_count: usize,
    usable_count: usize,
    loose_count: usize,
    bad_count: usize,
    late_count: usize,
    missed_count: usize,
}

struct OpenTransition {
    id: i64,
    template: String,
    renderer_template: Option<String>,
    fallback_reason: Option<String>,
    planned_start_ms: Option<i64>,
    actual_start_ms: Option<i64>,
    timing_delta_ms: Option<i64>,
    timing_source: Option<String>,
    timing_status: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct RendererStatus {
    planned_template: Option<String>,
    renderer_template: Option<String>,
    renderer_mode: Option<String>,
    downgrade_reason: Option<String>,
    planning_reason: Option<String>,
    sync_target: Option<String>,
    planned_start_ms: Option<i64>,
    actual_start_ms: Option<i64>,
    timing_delta_ms: Option<i64>,
    timing_source: Option<String>,
    timing_status: Option<String>,
    timing_quality: String,
    timing_direction: String,
}

#[derive(Debug, Deserialize)]
struct RebuildDjProfileRequest {
    media_ref_kind: String,
    media_ref_id: String,
}

#[derive(Debug, Serialize)]
struct RebuildDjProfileResponse {
    accepted: bool,
    status: String,
}

enum RebuildProfileCandidate {
    Ready(
        DjMediaRef,
        tokio::sync::mpsc::UnboundedSender<
            crate::services::audio_analysis::dj_profile::DjAnalysisJob,
        >,
    ),
    Response(RebuildDjProfileResponse),
}

#[derive(Debug, PartialEq, Eq)]
enum ProfileRebuildInflightDecision {
    Start,
    AlreadyRunning,
}

#[derive(Debug, Serialize)]
struct FeedbackResponse {
    accepted: bool,
}

async fn get_enabled(
    State(state): State<SharedState>,
) -> Result<Json<EnabledResponse>, StatusCode> {
    let enabled = {
        let state = state.read().await;
        state
            .db
            .with_conn(queries::is_dj_engine_enabled)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    Ok(Json(EnabledResponse { enabled }))
}

async fn set_enabled(
    State(state): State<SharedState>,
    Json(payload): Json<SetEnabledRequest>,
) -> Result<Json<EnabledResponse>, StatusCode> {
    let (runtime, lookahead) = {
        let state_guard = state.write().await;
        let ephemeral_lookahead = if payload.enabled {
            super::active_ephemeral_tidal_mix_dj_pair(&state_guard).and_then(|pair| {
                player::dj_lookahead_start_from_pair(pair, DEFAULT_DJ_LOOKAHEAD_DEADLINE_SAMPLES)
            })
        } else {
            None
        };
        let lookahead = state_guard
            .db
            .with_conn(|conn| {
                queries::set_dj_engine_enabled(conn, payload.enabled)?;
                if payload.enabled {
                    player::build_dj_lookahead_start(conn, DEFAULT_DJ_LOOKAHEAD_DEADLINE_SAMPLES)
                } else {
                    Ok(None)
                }
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let lookahead = ephemeral_lookahead.or(lookahead);
        (
            state_guard
                .playback_runtime
                .as_ref()
                .map(|runtime| runtime.handle.clone()),
            lookahead,
        )
    };

    if let Some(runtime) = runtime {
        let _ = runtime.set_dj_engine_enabled(payload.enabled);
        if payload.enabled {
            if let Some(lookahead) = lookahead {
                let _ = lookahead.dispatch(&runtime);
            }
        } else {
            let _ = runtime.start_dj_lookahead(None, None, None, None, u64::MAX, 0);
        }
    }

    Ok(Json(EnabledResponse {
        enabled: payload.enabled,
    }))
}

async fn get_status(
    State(state): State<SharedState>,
) -> Result<Json<DjStatusResponse>, StatusCode> {
    let (response, missing_profile_refs, ready_active_pair) = {
        let state = state.read().await;
        let ephemeral_pair = super::active_ephemeral_tidal_mix_dj_pair(&state);
        let ephemeral_labels = super::active_ephemeral_tidal_mix_dj_labels(&state);
        let active_track_id = state
            .playback_runtime_info
            .as_ref()
            .and_then(|info| info.active_track_id);
        let playback_generation = state
            .playback_generation
            .load(std::sync::atomic::Ordering::Relaxed);
        state
            .db
            .with_conn(|conn| {
                let enabled = queries::is_dj_engine_enabled(conn)?;
                let pair = match ephemeral_pair.clone() {
                    Some(pair) => pair,
                    None => crate::playback::dj_lookahead::load_dj_lookahead_pair(conn)?,
                };
                let current_ref = pair.current.clone();
                let next_ref = pair.next.clone();
                let current = match pair.current {
                    Some(media_ref) => {
                        let key = media_ref.profile_key();
                        let label = ephemeral_labels
                            .iter()
                            .find(|(candidate, _)| candidate == &key)
                            .map(|(_, label)| label);
                        Some(deck_status(conn, &media_ref, label)?)
                    }
                    None => None,
                };
                let next = match pair.next {
                    Some(media_ref) => {
                        let key = media_ref.profile_key();
                        let label = ephemeral_labels
                            .iter()
                            .find(|(candidate, _)| candidate == &key)
                            .map(|(_, label)| label);
                        Some(deck_status(conn, &media_ref, label)?)
                    }
                    None => None,
                };
                let safe_crossfade_suggestion =
                    safe_crossfade_suggestion(conn, current.as_ref(), next.as_ref())?;
                let fallback_reason = if !enabled {
                    Some("disabled".to_string())
                } else if current.is_none() || next.is_none() {
                    Some("pair_missing".to_string())
                } else if current.as_ref().is_some_and(|deck| !deck.profile_ready) {
                    Some("missing_current_profile".to_string())
                } else if next.as_ref().is_some_and(|deck| !deck.profile_ready) {
                    Some("missing_next_profile".to_string())
                } else {
                    None
                };
                let latest_transition =
                    latest_open_transition_for_pair(conn, current_ref.as_ref(), next_ref.as_ref())?;
                let recent_timing_events =
                    latest_dj_transition_timing_history(conn, DJ_TIMING_HISTORY_LIMIT)?;
                let timing_history_summary = summarize_timing_history(&recent_timing_events);
                let mut missing_profile_refs = Vec::new();
                if enabled {
                    if current.as_ref().is_some_and(|deck| !deck.profile_ready)
                        && let Some(media_ref) = current_ref.clone()
                    {
                        missing_profile_refs.push(media_ref);
                    }
                    if next.as_ref().is_some_and(|deck| !deck.profile_ready)
                        && let Some(media_ref) = next_ref.clone()
                    {
                        missing_profile_refs.push(media_ref);
                    }
                }
                let ready_active_pair = enabled
                    .then_some(())
                    .filter(|_| current_ref.is_some() && next_ref.is_some())
                    .and(active_track_id)
                    .filter(|_| fallback_reason.is_none() && latest_transition.is_none())
                    .map(|track_id| (track_id, playback_generation));
                let renderer_status = renderer_status_for_transition(latest_transition.as_ref());
                Ok((
                    DjStatusResponse {
                        enabled,
                        current,
                        next,
                        selected_program: latest_transition
                            .as_ref()
                            .map(|transition| transition.template.clone()),
                        planned_template: renderer_status.planned_template,
                        renderer_template: renderer_status.renderer_template,
                        renderer_mode: renderer_status.renderer_mode,
                        downgrade_reason: renderer_status.downgrade_reason,
                        planning_reason: renderer_status.planning_reason,
                        sync_target: renderer_status.sync_target,
                        planned_start_ms: renderer_status.planned_start_ms,
                        actual_start_ms: renderer_status.actual_start_ms,
                        timing_delta_ms: renderer_status.timing_delta_ms,
                        timing_source: renderer_status.timing_source,
                        timing_status: renderer_status.timing_status,
                        timing_quality: renderer_status.timing_quality,
                        timing_direction: renderer_status.timing_direction,
                        fallback_reason,
                        profile_confidence_floor: DJ_PROFILE_CONFIDENCE_FLOOR,
                        last_transition_event_id: latest_transition
                            .as_ref()
                            .map(|transition| transition.id),
                        recent_timing_events,
                        timing_history_summary,
                        safe_crossfade_suggestion,
                    },
                    missing_profile_refs,
                    ready_active_pair,
                ))
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    for media_ref in missing_profile_refs {
        queue_tidal_profile_rebuild_if_idle(state.clone(), media_ref).await?;
    }
    if let Some((current_track_id, generation)) = ready_active_pair {
        tracing::info!(
            current_track_id,
            generation,
            "DJ profiles ready, requesting transition planning"
        );
        if let Err(error) =
            super::handle_near_end(state.clone(), current_track_id, generation).await
        {
            tracing::warn!(error = %error, "DJ ready pair planning failed");
        }
    }
    Ok(Json(response))
}

async fn get_profile(
    State(state): State<SharedState>,
    Path(track_id): Path<i64>,
) -> Result<Json<ProfileResponse>, StatusCode> {
    let profile = {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| queries::get_audio_dj_profile_for_track(conn, track_id))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    }
    .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(profile_response(track_id, &profile)))
}

async fn rebuild_profile(
    State(state): State<SharedState>,
    Json(payload): Json<RebuildDjProfileRequest>,
) -> Result<Json<RebuildDjProfileResponse>, StatusCode> {
    let candidate = {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| {
                if !queries::is_dj_engine_enabled(conn)? {
                    return Ok(RebuildProfileCandidate::Response(
                        RebuildDjProfileResponse {
                            accepted: false,
                            status: "dj_disabled".to_string(),
                        },
                    ));
                }
                let key = AudioDjProfileKey {
                    media_ref_kind: payload.media_ref_kind.clone(),
                    media_ref_id: payload.media_ref_id.clone(),
                };
                let pair = match super::active_ephemeral_tidal_mix_dj_pair(&state) {
                    Some(pair) => pair,
                    None => crate::playback::dj_lookahead::load_dj_lookahead_pair(conn)?,
                };
                let media_ref = pair
                    .current
                    .as_ref()
                    .filter(|media_ref| media_ref.profile_key() == key)
                    .or_else(|| {
                        pair.next
                            .as_ref()
                            .filter(|media_ref| media_ref.profile_key() == key)
                    })
                    .cloned();
                let Some(media_ref) = media_ref else {
                    return Ok(RebuildProfileCandidate::Response(
                        RebuildDjProfileResponse {
                            accepted: false,
                            status: "not_current_pair".to_string(),
                        },
                    ));
                };
                if dj_profile_is_current_version(conn, &key)? {
                    return Ok(RebuildProfileCandidate::Response(
                        RebuildDjProfileResponse {
                            accepted: false,
                            status: "already_current".to_string(),
                        },
                    ));
                }
                let Some(dj_analysis_tx) = state.dj_analysis_tx.clone() else {
                    return Ok(RebuildProfileCandidate::Response(
                        RebuildDjProfileResponse {
                            accepted: false,
                            status: "source_unavailable".to_string(),
                        },
                    ));
                };
                Ok(RebuildProfileCandidate::Ready(media_ref, dj_analysis_tx))
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    let (media_ref, dj_analysis_tx) = match candidate {
        RebuildProfileCandidate::Ready(media_ref, dj_analysis_tx) => (media_ref, dj_analysis_tx),
        RebuildProfileCandidate::Response(response) => return Ok(Json(response)),
    };

    let queued = queue_tidal_profile_rebuild(state, media_ref, dj_analysis_tx, true).await?;
    Ok(Json(queued))
}

async fn queue_tidal_profile_rebuild(
    state: SharedState,
    media_ref: DjMediaRef,
    dj_analysis_tx: tokio::sync::mpsc::UnboundedSender<
        crate::services::audio_analysis::dj_profile::DjAnalysisJob,
    >,
    force: bool,
) -> Result<RebuildDjProfileResponse, StatusCode> {
    let DjMediaRef::TidalTrack { tidal_id, .. } = media_ref.clone() else {
        return Ok(RebuildDjProfileResponse {
            accepted: false,
            status: "source_unavailable".to_string(),
        });
    };
    let media_key = media_ref.profile_key();
    if !force {
        let already_current = {
            let state_guard = state.read().await;
            state_guard
                .db
                .with_conn(|conn| dj_profile_is_current_version(conn, &media_key))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        };
        if already_current {
            tracing::debug!(
                media_ref_kind = %media_key.media_ref_kind,
                media_ref_id = %media_key.media_ref_id,
                "Skipping current DJ profile rebuild"
            );
            return Ok(RebuildDjProfileResponse {
                accepted: false,
                status: "already_current".to_string(),
            });
        }
    }
    let inflight_key = dj_profile_inflight_key(&media_key);
    let inflight = {
        let state_guard = state.read().await;
        state_guard.dj_profile_rebuild_inflight.clone()
    };
    match mark_dj_profile_rebuild_inflight(
        &inflight,
        &inflight_key,
        std::time::Duration::from_secs(DJ_PROFILE_AUTO_REBUILD_RETRY_SECS),
    )? {
        ProfileRebuildInflightDecision::Start => {
            tracing::info!(
                media_ref_kind = %media_ref.profile_key().media_ref_kind,
                media_ref_id = %media_ref.profile_key().media_ref_id,
                force,
                "DJ profile rebuild accepted"
            );
        }
        ProfileRebuildInflightDecision::AlreadyRunning => {
            tracing::debug!(
                media_ref_kind = %media_ref.profile_key().media_ref_kind,
                media_ref_id = %media_ref.profile_key().media_ref_id,
                force,
                "DJ profile rebuild already running"
            );
            return Ok(RebuildDjProfileResponse {
                accepted: true,
                status: "already_running".to_string(),
            });
        }
    }

    let tokens = {
        let state_guard = state.read().await;
        state_guard.tidal_tokens.clone()
    };
    let tokens = match tokens {
        Some(tokens) => Some(tokens),
        None => super::load_persisted_tidal_tokens(&state)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    };
    let Some(tokens) = tokens else {
        clear_dj_profile_inflight(&inflight, &inflight_key);
        tracing::warn!(
            media_ref_kind = %media_ref.profile_key().media_ref_kind,
            media_ref_id = %media_ref.profile_key().media_ref_id,
            "DJ profile rebuild source unavailable"
        );
        return Ok(RebuildDjProfileResponse {
            accepted: false,
            status: "source_unavailable".to_string(),
        });
    };

    let user_quality = super::current_user_audio_quality(&state).await;
    let request = tidal_stream::StreamRequest::new(
        tidal_id,
        super::requested_tidal_quality(user_quality, None),
    );
    let track = rebuild_track_for_tidal_ref(&state, tidal_id).await;
    let (http_client, generation) = {
        let state_guard = state.read().await;
        (
            state_guard.http_client.clone(),
            state_guard
                .playback_generation
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    };
    let config = PlaybackRuntimeConfig::new(http_client, tokens.access_token, None)
        .with_dj_analysis(true, Some(dj_analysis_tx))
        .for_dj_analysis_only();
    let job = player::PreparedPlaybackJob::new(
        track.clone(),
        PlaybackSourceRequest::TidalStream(request),
        GaplessPlan::disabled(),
    )
    .with_generation(generation)
    .with_dj_media_ref(media_ref);
    let (command_tx, _command_rx) = std::sync::mpsc::channel::<PlaybackRuntimeCommand>();
    let shared = Arc::new(PlaybackSharedState::new(
        track.id,
        generation,
        PlaybackSourceKind::TidalStream,
        GaplessPlan::disabled(),
        48_000,
        2,
        None,
        command_tx,
        Arc::new(AtomicU32::new(1.0_f32.to_bits())),
        Arc::new(AtomicU64::new(0)),
        Arc::new(AtomicU64::new(0)),
    ));

    tokio::task::spawn_blocking(move || {
        if let Err(error) = decode_and_buffer_job(config, job, shared, 48_000, 2) {
            tracing::warn!(tidal_id, error = %error, "DJ profile rebuild decode failed");
        } else {
            tracing::info!(tidal_id, "DJ profile rebuild decode queued analysis");
        }
    });

    Ok(RebuildDjProfileResponse {
        accepted: true,
        status: "accepted".to_string(),
    })
}

async fn queue_tidal_profile_rebuild_if_idle(
    state: SharedState,
    media_ref: DjMediaRef,
) -> Result<(), StatusCode> {
    let dj_analysis_tx = {
        let state_guard = state.read().await;
        state_guard.dj_analysis_tx.clone()
    };
    let Some(dj_analysis_tx) = dj_analysis_tx else {
        return Ok(());
    };
    let _ = queue_tidal_profile_rebuild(state, media_ref, dj_analysis_tx, false).await?;
    Ok(())
}

fn mark_dj_profile_rebuild_inflight(
    inflight: &Arc<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>>,
    key: &str,
    retry_after: std::time::Duration,
) -> Result<ProfileRebuildInflightDecision, StatusCode> {
    let mut guard = inflight
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if let Some(started_at) = guard.get(key)
        && started_at.elapsed() < retry_after
    {
        return Ok(ProfileRebuildInflightDecision::AlreadyRunning);
    }
    guard.insert(key.to_string(), std::time::Instant::now());
    Ok(ProfileRebuildInflightDecision::Start)
}

fn dj_profile_inflight_key(key: &AudioDjProfileKey) -> String {
    format!("{}:{}", key.media_ref_kind, key.media_ref_id)
}

fn clear_dj_profile_inflight(
    inflight: &Arc<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>>,
    key: &str,
) {
    if let Ok(mut guard) = inflight.lock() {
        guard.remove(key);
    }
}

async fn rebuild_track_for_tidal_ref(
    state: &SharedState,
    tidal_id: i64,
) -> crate::db::models::Track {
    let state_guard = state.read().await;
    if let Some(track) = state_guard
        .ephemeral_tidal_track
        .as_ref()
        .filter(|track| track.tidal_id == Some(tidal_id))
    {
        return track.clone();
    }
    if let Some(prepared) = state_guard
        .prepared_ephemeral_tidal_next
        .as_ref()
        .filter(|prepared| prepared.tidal_track_id == tidal_id)
    {
        return prepared.synthetic_track.clone();
    }
    if let Some(pending) = state_guard
        .pending_tidal_mix_queue
        .lock()
        .unwrap()
        .iter()
        .find(|track| track.tidal_track_id == tidal_id)
        .cloned()
    {
        return synthetic_rebuild_track(&pending);
    }
    state_guard
        .db
        .with_conn(|conn| library_track_for_tidal_id(conn, tidal_id))
        .ok()
        .flatten()
        .unwrap_or_else(|| {
            synthetic_rebuild_track(&crate::PendingEphemeralTidalTrack {
                tidal_track_id: tidal_id,
                title: format!("TIDAL {tidal_id}"),
                artist_name: None,
                album_title: None,
                artwork_url: None,
                duration_ms: None,
            })
        })
}

fn library_track_for_tidal_id(
    conn: &rusqlite::Connection,
    tidal_id: i64,
) -> anyhow::Result<Option<crate::db::models::Track>> {
    conn.query_row(
        "SELECT t.id, t.title, t.artist_id, ar.name, t.album_id, al.title,
                t.disc_number, t.track_number, t.duration_ms, t.isrc, t.tidal_id,
                t.ytmusic_id, t.soundcloud_id, t.best_quality, t.best_source,
                t.fidelity_score, t.is_favorite, t.play_count, t.last_played_at,
                t.date_added, t.source, al.artwork_url
         FROM tracks t
         LEFT JOIN artists ar ON t.artist_id = ar.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE t.tidal_id = ?1
         LIMIT 1",
        params![tidal_id],
        |row| {
            Ok(crate::db::models::Track {
                id: row.get(0)?,
                title: row.get(1)?,
                artist_id: row.get(2)?,
                artist_name: row.get(3)?,
                album_id: row.get(4)?,
                album_title: row.get(5)?,
                disc_number: row.get(6)?,
                track_number: row.get(7)?,
                duration_ms: row.get(8)?,
                isrc: row.get(9)?,
                tidal_id: row.get(10)?,
                ytmusic_id: row.get(11)?,
                soundcloud_id: row.get(12)?,
                best_quality: row.get(13)?,
                best_source: row.get(14)?,
                fidelity_score: row.get(15)?,
                is_favorite: row.get(16)?,
                play_count: row.get(17)?,
                last_played_at: row.get(18)?,
                date_added: row.get(19)?,
                source: row.get(20)?,
                artwork_url: row.get(21)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn synthetic_rebuild_track(track: &crate::PendingEphemeralTidalTrack) -> crate::db::models::Track {
    crate::db::models::Track {
        id: -track.tidal_track_id,
        title: track.title.clone(),
        artist_id: 0,
        artist_name: track.artist_name.clone(),
        album_id: None,
        album_title: track.album_title.clone(),
        disc_number: None,
        track_number: None,
        duration_ms: track.duration_ms,
        isrc: None,
        tidal_id: Some(track.tidal_track_id),
        ytmusic_id: None,
        soundcloud_id: None,
        best_quality: None,
        best_source: Some("tidal".to_string()),
        fidelity_score: 0,
        is_favorite: false,
        play_count: 0,
        last_played_at: None,
        date_added: None,
        source: "tidal_ephemeral".to_string(),
        artwork_url: track.artwork_url.clone(),
    }
}

async fn get_mix_intent(
    State(state): State<SharedState>,
) -> Result<Json<MixIntentResponse>, StatusCode> {
    let intent = {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| Ok(queries::get_dj_global_policy(conn)?.0))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    Ok(Json(MixIntentResponse { intent }))
}

async fn set_mix_intent(
    State(state): State<SharedState>,
    Json(payload): Json<SetMixIntentRequest>,
) -> Result<Json<MixIntentResponse>, StatusCode> {
    if !matches!(payload.intent.as_str(), "safe" | "balanced" | "bold") {
        return Err(StatusCode::BAD_REQUEST);
    }
    let intent = {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| {
                let (_, speed) = queries::get_dj_global_policy(conn)?;
                queries::set_dj_global_policy(conn, &payload.intent, &speed)?;
                Ok(payload.intent.clone())
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    Ok(Json(MixIntentResponse { intent }))
}

async fn get_policy(
    State(state): State<SharedState>,
) -> Result<Json<DjPolicyResponse>, StatusCode> {
    let (mix_intent, transition_speed_bias) = {
        let state = state.read().await;
        state
            .db
            .with_conn(queries::get_dj_global_policy)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    Ok(Json(DjPolicyResponse {
        mix_intent,
        transition_speed_bias,
    }))
}

async fn set_policy(
    State(state): State<SharedState>,
    Json(payload): Json<SetDjPolicyRequest>,
) -> Result<Json<DjPolicyResponse>, StatusCode> {
    let (mix_intent, transition_speed_bias) = {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| {
                let (current_intent, current_speed) = queries::get_dj_global_policy(conn)?;
                let mix_intent = payload.mix_intent.clone().unwrap_or(current_intent);
                let transition_speed_bias = payload
                    .transition_speed_bias
                    .clone()
                    .unwrap_or(current_speed);
                queries::set_dj_global_policy(conn, &mix_intent, &transition_speed_bias)?;
                Ok((mix_intent, transition_speed_bias))
            })
            .map_err(|_| StatusCode::BAD_REQUEST)?
    };
    Ok(Json(DjPolicyResponse {
        mix_intent,
        transition_speed_bias,
    }))
}

async fn record_feedback(
    State(state): State<SharedState>,
    Json(payload): Json<DjFeedbackRequest>,
) -> Result<Json<FeedbackResponse>, StatusCode> {
    let rating = feedback_rating(&payload.rating).ok_or(StatusCode::BAD_REQUEST)?;
    if let Some(id) = payload.transition_event_id {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE dj_transition_events
                     SET user_rating = ?1,
                         outcome = COALESCE(outcome, ?2),
                         outcome_at = COALESCE(outcome_at, datetime('now')),
                         rejected_alternatives_json = COALESCE(rejected_alternatives_json, ?3)
                     WHERE id = ?4",
                    params![
                        rating,
                        if rating < 0 {
                            "bad_feedback"
                        } else {
                            "good_feedback"
                        },
                        payload.reason.as_deref(),
                        id
                    ],
                )?;
                Ok(())
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    Ok(Json(FeedbackResponse { accepted: true }))
}

async fn get_profile_correction(
    State(state): State<SharedState>,
    Path((kind, id)): Path<(String, String)>,
) -> Result<Json<DjProfileCorrectionResponse>, StatusCode> {
    let key = AudioDjProfileKey {
        media_ref_kind: kind,
        media_ref_id: id,
    };
    let correction = {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| queries::get_audio_dj_profile_correction(conn, &key))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    }
    .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(correction_response(correction)))
}

async fn set_profile_correction(
    State(state): State<SharedState>,
    Json(payload): Json<DjProfileCorrectionRequest>,
) -> Result<Json<DjProfileCorrectionResponse>, StatusCode> {
    if let Some(speed) = payload.transition_speed_bias.as_deref()
        && !matches!(speed, "slower" | "neutral" | "faster")
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let now = Utc::now().to_rfc3339();
    let row = AudioDjProfileCorrectionRow {
        media_ref_kind: payload.media_ref_kind,
        media_ref_id: payload.media_ref_id,
        bpm_multiplier: payload.bpm_multiplier,
        downbeat_offset_beats: payload.downbeat_offset_beats,
        phrase_offset_bars: payload.phrase_offset_bars,
        safe_crossfade_only: payload.safe_crossfade_only.unwrap_or(false),
        transition_speed_bias: payload.transition_speed_bias,
        notes: payload.notes,
        created_at: now.clone(),
        updated_at: now,
    };
    {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| queries::upsert_audio_dj_profile_correction(conn, &row))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    Ok(Json(correction_response(row)))
}

fn profile_response(track_id: i64, profile: &AudioDjProfileRow) -> ProfileResponse {
    ProfileResponse {
        track_id,
        profile_version: profile.profile_version.clone(),
        beat_count: decode_f32_blob(&profile.beat_grid_blob)
            .map(|values| values.len())
            .unwrap_or(0),
        downbeat_count: decode_f32_blob(&profile.downbeats_blob)
            .map(|values| values.len())
            .unwrap_or(0),
        phrase_count: decode_u32_blob(&profile.phrase_boundaries_blob)
            .map(|values| values.len())
            .unwrap_or(0),
    }
}

fn dj_profile_is_current_version(
    conn: &rusqlite::Connection,
    key: &AudioDjProfileKey,
) -> anyhow::Result<bool> {
    Ok(queries::get_audio_dj_profile(conn, key)?
        .is_some_and(|row| row.profile_version == DJ_PROFILE_VERSION))
}

fn correction_response(row: AudioDjProfileCorrectionRow) -> DjProfileCorrectionResponse {
    DjProfileCorrectionResponse {
        media_ref_kind: row.media_ref_kind,
        media_ref_id: row.media_ref_id,
        bpm_multiplier: row.bpm_multiplier,
        downbeat_offset_beats: row.downbeat_offset_beats,
        phrase_offset_bars: row.phrase_offset_bars,
        safe_crossfade_only: row.safe_crossfade_only,
        transition_speed_bias: row.transition_speed_bias,
        notes: row.notes,
        applies: "next_transition".to_string(),
    }
}

fn deck_status(
    conn: &rusqlite::Connection,
    media_ref: &DjMediaRef,
    label_override: Option<&(String, Option<String>)>,
) -> anyhow::Result<DjDeckStatus> {
    let key = media_ref.profile_key();
    let profile = queries::get_audio_dj_profile(conn, &key)?;
    let correction = queries::get_audio_dj_profile_correction(conn, &key)?;
    let (title, artist) = match label_override {
        Some((title, artist)) => (title.clone(), artist.clone()),
        None => media_ref_label(conn, media_ref)?,
    };
    let (beat_count, downbeat_count, phrase_count, profile_confidence) =
        if let Some(profile) = profile.as_ref() {
            (
                decode_f32_blob(&profile.beat_grid_blob).map(|values| values.len()),
                decode_f32_blob(&profile.downbeats_blob).map(|values| values.len()),
                decode_u32_blob(&profile.phrase_boundaries_blob).map(|values| values.len()),
                Some(profile.profile_confidence),
            )
        } else {
            (None, None, None, None)
        };
    Ok(DjDeckStatus {
        media_ref_kind: key.media_ref_kind,
        media_ref_id: key.media_ref_id,
        title,
        artist,
        profile_ready: profile.is_some(),
        profile_confidence,
        beat_count,
        downbeat_count,
        phrase_count,
        safe_crossfade_only: correction.is_some_and(|row| row.safe_crossfade_only),
    })
}

fn latest_open_transition_for_pair(
    conn: &rusqlite::Connection,
    current: Option<&DjMediaRef>,
    next: Option<&DjMediaRef>,
) -> anyhow::Result<Option<OpenTransition>> {
    let (Some(current), Some(next)) = (current, next) else {
        return Ok(None);
    };
    let current_key = current.profile_key();
    let next_key = next.profile_key();
    conn.query_row(
        "SELECT id, template, program_json, fallback_reason,
                planned_start_ms, actual_start_ms, timing_delta_ms,
                timing_source, timing_status
         FROM dj_transition_events
         WHERE from_media_ref_kind = ?1
           AND from_media_ref_id = ?2
           AND to_media_ref_kind = ?3
           AND to_media_ref_id = ?4
           AND outcome IS NULL
         ORDER BY started_at DESC, id DESC
         LIMIT 1",
        params![
            current_key.media_ref_kind,
            current_key.media_ref_id,
            next_key.media_ref_kind,
            next_key.media_ref_id,
        ],
        |row| {
            let program_json: String = row.get(2)?;
            Ok(OpenTransition {
                id: row.get(0)?,
                template: row.get(1)?,
                renderer_template: renderer_template_from_program_json(&program_json),
                fallback_reason: row.get(3)?,
                planned_start_ms: row.get(4)?,
                actual_start_ms: row.get(5)?,
                timing_delta_ms: row.get(6)?,
                timing_source: row.get(7)?,
                timing_status: row.get(8)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn latest_dj_transition_timing_history(
    conn: &rusqlite::Connection,
    limit: i64,
) -> anyhow::Result<Vec<DjTimingHistoryEvent>> {
    let mut stmt = conn.prepare(
        "SELECT e.id,
                from_track.title, from_artist.name,
                to_track.title, to_artist.name,
                e.template, e.program_json, e.fallback_reason,
                planned_start_ms, actual_start_ms, timing_delta_ms,
                timing_source, timing_status, e.started_at
         FROM dj_transition_events e
         LEFT JOIN tracks from_track ON from_track.id = e.from_track_id
         LEFT JOIN artists from_artist ON from_artist.id = from_track.artist_id
         LEFT JOIN tracks to_track ON to_track.id = e.to_track_id
         LEFT JOIN artists to_artist ON to_artist.id = to_track.artist_id
         WHERE e.timing_status IN ('fired', 'late', 'missed')
           AND NOT (
             e.timing_status = 'missed'
             AND EXISTS (
               SELECT 1
               FROM dj_transition_events fired
               WHERE fired.from_media_ref_kind IS e.from_media_ref_kind
                  AND fired.from_media_ref_id IS e.from_media_ref_id
                  AND fired.to_media_ref_kind IS e.to_media_ref_kind
                  AND fired.to_media_ref_id IS e.to_media_ref_id
                  AND fired.id > e.id
                  AND fired.timing_status = 'fired'
              )
            )
         ORDER BY e.started_at DESC, e.id DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit.max(0)], |row| {
        let program_json: String = row.get(6)?;
        let timing_delta_ms: Option<i64> = row.get(10)?;
        let timing_status: Option<String> = row.get(12)?;
        Ok(DjTimingHistoryEvent {
            event_id: row.get(0)?,
            from_title: row.get(1)?,
            from_artist: row.get(2)?,
            to_title: row.get(3)?,
            to_artist: row.get(4)?,
            planned_template: row.get(5)?,
            renderer_template: renderer_template_from_program_json(&program_json),
            planning_reason: row.get(7)?,
            planned_start_ms: row.get(8)?,
            actual_start_ms: row.get(9)?,
            timing_delta_ms,
            timing_source: row.get(11)?,
            timing_status: timing_status.clone(),
            timing_quality: timing_quality(timing_status.as_deref(), timing_delta_ms).to_string(),
            timing_direction: timing_direction(timing_status.as_deref(), timing_delta_ms)
                .to_string(),
            started_at: row.get(13)?,
        })
    })?;
    let mut events = Vec::new();
    for row in rows {
        events.push(row?);
    }
    Ok(events)
}

fn timing_quality(timing_status: Option<&str>, timing_delta_ms: Option<i64>) -> &'static str {
    if timing_status == Some("missed") {
        return "bad";
    }
    let Some(delta_ms) = timing_delta_ms else {
        return "bad";
    };
    match delta_ms.abs() {
        0..=150 => "tight",
        151..=500 => "usable",
        501..=1000 => "loose",
        _ => "bad",
    }
}

fn timing_direction(timing_status: Option<&str>, timing_delta_ms: Option<i64>) -> &'static str {
    match timing_status {
        Some("missed") => "missed",
        Some("armed") => "pending",
        Some("late") => "late",
        Some("fired") => match timing_delta_ms {
            Some(delta_ms) if delta_ms < -250 => "early",
            Some(delta_ms) if delta_ms > 250 => "late",
            Some(_) => "on_time",
            None => "unknown",
        },
        _ => "unknown",
    }
}

fn summarize_timing_history(events: &[DjTimingHistoryEvent]) -> DjTimingHistorySummary {
    let mut delta_sum = 0i64;
    let mut abs_delta_sum = 0i64;
    let mut delta_count = 0i64;
    let mut tight_count = 0usize;
    let mut usable_count = 0usize;
    let mut loose_count = 0usize;
    let mut bad_count = 0usize;
    let mut late_count = 0usize;
    let mut missed_count = 0usize;

    for event in events {
        match event.timing_quality.as_str() {
            "tight" => tight_count += 1,
            "usable" => usable_count += 1,
            "loose" => loose_count += 1,
            _ => bad_count += 1,
        }
        if event.timing_status.as_deref() == Some("late") {
            late_count += 1;
        }
        if event.timing_status.as_deref() == Some("missed") {
            missed_count += 1;
        }
        if let Some(delta_ms) = event.timing_delta_ms {
            delta_sum += delta_ms;
            abs_delta_sum += delta_ms.abs();
            delta_count += 1;
        }
    }

    DjTimingHistorySummary {
        event_count: events.len(),
        average_delta_ms: (delta_count > 0).then_some(delta_sum / delta_count),
        average_abs_delta_ms: (delta_count > 0).then_some(abs_delta_sum / delta_count),
        tight_count,
        usable_count,
        loose_count,
        bad_count,
        late_count,
        missed_count,
    }
}

#[cfg(test)]
fn open_transition_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OpenTransition> {
    let program_json: String = row.get(2)?;
    Ok(OpenTransition {
        id: row.get(0)?,
        template: row.get(1)?,
        renderer_template: renderer_template_from_program_json(&program_json),
        fallback_reason: row.get(3)?,
        planned_start_ms: row.get(4)?,
        actual_start_ms: row.get(5)?,
        timing_delta_ms: row.get(6)?,
        timing_source: row.get(7)?,
        timing_status: row.get(8)?,
    })
}

#[cfg(test)]
fn latest_completed_timing_transition(
    conn: &rusqlite::Connection,
) -> anyhow::Result<Option<OpenTransition>> {
    conn.query_row(
        "SELECT id, template, program_json, fallback_reason,
                planned_start_ms, actual_start_ms, timing_delta_ms,
                timing_source, timing_status
         FROM dj_transition_events
         WHERE timing_status IN ('fired', 'late', 'missed')
         ORDER BY started_at DESC, id DESC
         LIMIT 1",
        [],
        open_transition_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn renderer_status_for_transition(transition: Option<&OpenTransition>) -> RendererStatus {
    let Some(transition) = transition else {
        return RendererStatus {
            planned_template: None,
            renderer_template: None,
            renderer_mode: None,
            downgrade_reason: None,
            planning_reason: None,
            sync_target: None,
            planned_start_ms: None,
            actual_start_ms: None,
            timing_delta_ms: None,
            timing_source: None,
            timing_status: None,
            timing_quality: "unknown".to_string(),
            timing_direction: "unknown".to_string(),
        };
    };
    let quality = timing_quality(
        transition.timing_status.as_deref(),
        transition.timing_delta_ms,
    )
    .to_string();
    let direction = timing_direction(
        transition.timing_status.as_deref(),
        transition.timing_delta_ms,
    )
    .to_string();
    if transition.renderer_template.as_deref() == Some("SafeCrossfade") {
        return RendererStatus {
            planned_template: Some(transition.template.clone()),
            renderer_template: transition.renderer_template.clone(),
            renderer_mode: Some("dj_gain_program".to_string()),
            downgrade_reason: (transition.template != "SafeCrossfade")
                .then(|| "template_not_renderable".to_string()),
            planning_reason: transition.fallback_reason.clone(),
            sync_target: transition.timing_source.clone(),
            planned_start_ms: transition.planned_start_ms,
            actual_start_ms: transition.actual_start_ms,
            timing_delta_ms: transition.timing_delta_ms,
            timing_source: transition.timing_source.clone(),
            timing_status: transition.timing_status.clone(),
            timing_quality: quality,
            timing_direction: direction,
        };
    }
    RendererStatus {
        planned_template: Some(transition.template.clone()),
        renderer_template: None,
        renderer_mode: Some("legacy_overlap".to_string()),
        planning_reason: transition.fallback_reason.clone(),
        downgrade_reason: Some(
            if transition.template == "SafeCrossfade" {
                "dj_program_renderer_pending"
            } else {
                "template_not_renderable"
            }
            .to_string(),
        ),
        sync_target: transition.timing_source.clone(),
        planned_start_ms: transition.planned_start_ms,
        actual_start_ms: transition.actual_start_ms,
        timing_delta_ms: transition.timing_delta_ms,
        timing_source: transition.timing_source.clone(),
        timing_status: transition.timing_status.clone(),
        timing_quality: quality,
        timing_direction: direction,
    }
}

fn renderer_template_from_program_json(program_json: &str) -> Option<String> {
    let program: noor_mix::TransitionProgram = serde_json::from_str(program_json).ok()?;
    (program.template == "SafeCrossfade").then_some(program.template)
}

fn media_ref_label(
    conn: &rusqlite::Connection,
    media_ref: &DjMediaRef,
) -> anyhow::Result<(String, Option<String>)> {
    if let Some(track_id) = media_ref.track_id()
        && let Some(row) = conn
            .query_row(
                "SELECT t.title, ar.name
                 FROM tracks t
                 LEFT JOIN artists ar ON ar.id = t.artist_id
                 WHERE t.id = ?1",
                params![track_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()?
    {
        return Ok(row);
    }
    match media_ref {
        DjMediaRef::PendingQueueItem {
            pending_artist,
            pending_title,
            ..
        } => Ok((pending_title.clone(), Some(pending_artist.clone()))),
        _ => Ok((media_ref.profile_key().media_ref_id, None)),
    }
}

fn safe_crossfade_suggestion(
    conn: &rusqlite::Connection,
    current: Option<&DjDeckStatus>,
    next: Option<&DjDeckStatus>,
) -> anyhow::Result<Option<DjSafeSuggestion>> {
    for deck in [current, next].into_iter().flatten() {
        let key = AudioDjProfileKey {
            media_ref_kind: deck.media_ref_kind.clone(),
            media_ref_id: deck.media_ref_id.clone(),
        };
        let bad_feedback_count =
            queries::count_recent_bad_dj_feedback_for_ref(conn, &key, SAFE_SUGGESTION_BAD_COUNT)?;
        if bad_feedback_count >= SAFE_SUGGESTION_BAD_COUNT {
            return Ok(Some(DjSafeSuggestion {
                media_ref_kind: key.media_ref_kind,
                media_ref_id: key.media_ref_id,
                bad_feedback_count,
            }));
        }
    }
    Ok(None)
}

fn feedback_rating(value: &str) -> Option<i64> {
    match value {
        "good" => Some(1),
        "bad" | "too_safe" | "too_bold" => Some(-1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_profile_row(key: &AudioDjProfileKey, version: &str) -> AudioDjProfileRow {
        AudioDjProfileRow {
            media_ref_kind: key.media_ref_kind.clone(),
            media_ref_id: key.media_ref_id.clone(),
            track_id: None,
            queue_item_id: None,
            tidal_id: None,
            profile_version: version.to_string(),
            beat_grid_blob: vec![1, 2, 3],
            downbeats_blob: vec![4, 5],
            phrase_boundaries_blob: vec![6],
            mix_in_blob: vec![7],
            mix_out_blob: vec![8],
            intro_end_seconds: Some(16.0),
            outro_start_seconds: Some(180.0),
            breakdown_blob: vec![9],
            drop_blob: vec![10],
            safe_transition_windows_blob: vec![11],
            energy_contour_blob: vec![12],
            vocal_presence_blob: vec![13],
            vocal_density_blob: vec![14],
            lufs_loud_body: Some(-12.0),
            true_peak_dbtp: Some(-1.0),
            beat_confidence: Some(0.9),
            profile_confidence: 0.85,
            analysis_scope_ms: 90_000,
            is_temporary: false,
            source: "test".to_string(),
            computed_at: "2026-05-21T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn dj_profile_current_check_requires_current_version() {
        let conn = rusqlite::Connection::open_in_memory().expect("db");
        crate::db::schema::run_migrations(&conn).expect("migrations");
        let key = AudioDjProfileKey {
            media_ref_kind: "tidal_track".to_string(),
            media_ref_id: "123".to_string(),
        };

        assert!(!dj_profile_is_current_version(&conn, &key).expect("missing"));
        queries::upsert_audio_dj_profile(&conn, &test_profile_row(&key, "old_profile_v0"))
            .expect("old profile");
        assert!(!dj_profile_is_current_version(&conn, &key).expect("old"));
        queries::upsert_audio_dj_profile(&conn, &test_profile_row(&key, DJ_PROFILE_VERSION))
            .expect("current profile");

        assert!(dj_profile_is_current_version(&conn, &key).expect("current"));
    }

    #[test]
    fn renderer_status_keeps_non_renderable_template_out_of_main_renderer() {
        let status = renderer_status_for_transition(Some(&OpenTransition {
            id: 17,
            template: "FilterSweep".to_string(),
            renderer_template: None,
            fallback_reason: None,
            planned_start_ms: None,
            actual_start_ms: None,
            timing_delta_ms: None,
            timing_source: None,
            timing_status: None,
        }));

        assert_eq!(status.planned_template.as_deref(), Some("FilterSweep"));
        assert_eq!(status.renderer_template, None);
        assert_eq!(status.renderer_mode.as_deref(), Some("legacy_overlap"));
        assert_eq!(
            status.downgrade_reason.as_deref(),
            Some("template_not_renderable")
        );
    }

    #[test]
    fn renderer_status_marks_safe_crossfade_renderer_as_pending() {
        let status = renderer_status_for_transition(Some(&OpenTransition {
            id: 18,
            template: "SafeCrossfade".to_string(),
            renderer_template: None,
            fallback_reason: None,
            planned_start_ms: None,
            actual_start_ms: None,
            timing_delta_ms: None,
            timing_source: None,
            timing_status: None,
        }));

        assert_eq!(status.planned_template.as_deref(), Some("SafeCrossfade"));
        assert_eq!(status.renderer_template, None);
        assert_eq!(status.renderer_mode.as_deref(), Some("legacy_overlap"));
        assert_eq!(
            status.downgrade_reason.as_deref(),
            Some("dj_program_renderer_pending")
        );
    }

    #[test]
    fn renderer_status_exposes_safe_crossfade_runtime_program() {
        let status = renderer_status_for_transition(Some(&OpenTransition {
            id: 19,
            template: "FilterSweep".to_string(),
            renderer_template: Some("SafeCrossfade".to_string()),
            fallback_reason: Some("template_not_renderable".to_string()),
            planned_start_ms: Some(112_000),
            actual_start_ms: Some(112_144),
            timing_delta_ms: Some(144),
            timing_source: Some("downbeat_sync".to_string()),
            timing_status: Some("fired".to_string()),
        }));

        assert_eq!(status.planned_template.as_deref(), Some("FilterSweep"));
        assert_eq!(status.renderer_template.as_deref(), Some("SafeCrossfade"));
        assert_eq!(status.renderer_mode.as_deref(), Some("dj_gain_program"));
        assert_eq!(
            status.downgrade_reason.as_deref(),
            Some("template_not_renderable")
        );
        assert_eq!(status.planned_start_ms, Some(112_000));
        assert_eq!(status.actual_start_ms, Some(112_144));
        assert_eq!(status.timing_delta_ms, Some(144));
        assert_eq!(status.timing_source.as_deref(), Some("downbeat_sync"));
        assert_eq!(status.timing_status.as_deref(), Some("fired"));
    }

    #[test]
    fn renderer_status_keeps_safe_crossfade_planning_reason_out_of_downgrade() {
        let status = renderer_status_for_transition(Some(&OpenTransition {
            id: 20,
            template: "SafeCrossfade".to_string(),
            renderer_template: Some("SafeCrossfade".to_string()),
            fallback_reason: Some("next_profile_missing".to_string()),
            planned_start_ms: Some(112_000),
            actual_start_ms: Some(112_144),
            timing_delta_ms: Some(144),
            timing_source: Some("downbeat_sync".to_string()),
            timing_status: Some("fired".to_string()),
        }));

        assert_eq!(status.planned_template.as_deref(), Some("SafeCrossfade"));
        assert_eq!(status.renderer_template.as_deref(), Some("SafeCrossfade"));
        assert_eq!(status.renderer_mode.as_deref(), Some("dj_gain_program"));
        assert_eq!(status.downgrade_reason, None);
        assert_eq!(
            status.planning_reason.as_deref(),
            Some("next_profile_missing")
        );
    }

    #[test]
    fn latest_completed_timing_transition_returns_last_fired_row() {
        let conn = rusqlite::Connection::open_in_memory().expect("db");
        crate::db::schema::run_migrations(&conn).expect("migrations");
        conn.execute(
            "INSERT INTO dj_transition_events (
                from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                template, program_json, planner_version, planned_start_ms,
                actual_start_ms, timing_delta_ms, timing_source, timing_status
             ) VALUES (
                'tidal_track', '1', 'tidal_track', '2',
                'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
                222000, 222040, 40, 'downbeat_sync', 'fired'
             )",
            [],
        )
        .expect("insert");

        let transition = latest_completed_timing_transition(&conn)
            .expect("query")
            .expect("transition");

        assert_eq!(transition.planned_start_ms, Some(222_000));
        assert_eq!(transition.actual_start_ms, Some(222_040));
        assert_eq!(transition.timing_delta_ms, Some(40));
        assert_eq!(transition.timing_status.as_deref(), Some("fired"));
    }

    #[test]
    fn timing_history_keeps_completed_rows_when_newer_pair_is_armed() {
        let conn = rusqlite::Connection::open_in_memory().expect("db");
        crate::db::schema::run_migrations(&conn).expect("migrations");
        conn.execute(
            "INSERT INTO dj_transition_events (
                from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                template, program_json, planner_version, planned_start_ms,
                actual_start_ms, timing_delta_ms, timing_source, timing_status
             ) VALUES (
                'tidal_track', '1', 'tidal_track', '2',
                'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
                222000, 222040, 40, 'downbeat_sync', 'fired'
             )",
            [],
        )
        .expect("insert fired");
        conn.execute(
            "INSERT INTO dj_transition_events (
                from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                template, program_json, planner_version, planned_start_ms,
                timing_source, timing_status
             ) VALUES (
                'tidal_track', '2', 'tidal_track', '3',
                'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
                300000, 'beat_sync', 'armed'
             )",
            [],
        )
        .expect("insert armed");

        let current = DjMediaRef::TidalTrack {
            tidal_id: 2,
            track_id: None,
        };
        let next = DjMediaRef::TidalTrack {
            tidal_id: 3,
            track_id: None,
        };
        let open = latest_open_transition_for_pair(&conn, Some(&current), Some(&next))
            .expect("open")
            .expect("armed row");
        let history = latest_dj_transition_timing_history(&conn, 5).expect("history");

        assert_eq!(open.timing_status.as_deref(), Some("armed"));
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].event_id, 1);
        assert_eq!(history[0].actual_start_ms, Some(222_040));
        assert_eq!(history[0].timing_status.as_deref(), Some("fired"));
        assert_eq!(history[0].timing_quality, "tight");
        assert_eq!(history[0].timing_direction, "on_time");
    }

    #[test]
    fn timing_history_filters_duplicate_missed_row_when_pair_already_fired() {
        let conn = rusqlite::Connection::open_in_memory().expect("db");
        crate::db::schema::run_migrations(&conn).expect("migrations");
        conn.execute(
            "INSERT INTO dj_transition_events (
                from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                template, program_json, planner_version, planned_start_ms,
                timing_source, timing_status
             ) VALUES (
                'tidal_track', '1', 'tidal_track', '2',
                'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
                222000, 'downbeat_sync', 'missed'
             )",
            [],
        )
        .expect("insert duplicate missed");
        conn.execute(
            "INSERT INTO dj_transition_events (
                from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                template, program_json, planner_version, planned_start_ms,
                actual_start_ms, timing_delta_ms, timing_source, timing_status
             ) VALUES (
                'tidal_track', '1', 'tidal_track', '2',
                'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
                222000, 222040, 40, 'downbeat_sync', 'fired'
             )",
            [],
        )
        .expect("insert fired");

        let history = latest_dj_transition_timing_history(&conn, 5).expect("history");

        assert_eq!(history.len(), 1);
        assert_eq!(history[0].timing_status.as_deref(), Some("fired"));
        assert_eq!(history[0].timing_delta_ms, Some(40));
    }

    #[test]
    fn timing_history_keeps_newer_missed_attempt_after_older_fired_pair() {
        let conn = rusqlite::Connection::open_in_memory().expect("db");
        crate::db::schema::run_migrations(&conn).expect("migrations");
        conn.execute(
            "INSERT INTO dj_transition_events (
                from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                template, program_json, planner_version, planned_start_ms,
                actual_start_ms, timing_delta_ms, timing_source, timing_status
             ) VALUES (
                'tidal_track', '1', 'tidal_track', '2',
                'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
                222000, 222040, 40, 'downbeat_sync', 'fired'
             )",
            [],
        )
        .expect("insert fired");
        conn.execute(
            "INSERT INTO dj_transition_events (
                from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                template, program_json, planner_version, planned_start_ms,
                timing_source, timing_status
             ) VALUES (
                'tidal_track', '1', 'tidal_track', '2',
                'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
                222000, 'downbeat_sync', 'missed'
             )",
            [],
        )
        .expect("insert newer missed");

        let history = latest_dj_transition_timing_history(&conn, 5).expect("history");

        assert_eq!(history.len(), 2);
        assert_eq!(history[0].timing_status.as_deref(), Some("missed"));
        assert_eq!(history[1].timing_status.as_deref(), Some("fired"));
    }

    #[test]
    fn timing_history_includes_track_pair_labels() {
        let conn = rusqlite::Connection::open_in_memory().expect("db");
        crate::db::schema::run_migrations(&conn).expect("migrations");
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (10, 'Outgoing Artist'), (11, 'Incoming Artist')",
            [],
        )
        .expect("insert artists");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, source)
             VALUES (100, 'Outgoing Track', 10, 'tidal'), (101, 'Incoming Track', 11, 'tidal')",
            [],
        )
        .expect("insert tracks");
        conn.execute(
            "INSERT INTO dj_transition_events (
                from_track_id, to_track_id,
                from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                template, program_json, planner_version, fallback_reason, planned_start_ms,
                actual_start_ms, timing_delta_ms, timing_source, timing_status
             ) VALUES (
                100, 101,
                'tidal_track', '1', 'tidal_track', '2',
                'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
                'next_profile_missing',
                10000, 10320, 320, 'downbeat_sync', 'fired'
             )",
            [],
        )
        .expect("insert event");

        let history = latest_dj_transition_timing_history(&conn, 5).expect("history");

        assert_eq!(history[0].from_title.as_deref(), Some("Outgoing Track"));
        assert_eq!(history[0].from_artist.as_deref(), Some("Outgoing Artist"));
        assert_eq!(history[0].to_title.as_deref(), Some("Incoming Track"));
        assert_eq!(history[0].to_artist.as_deref(), Some("Incoming Artist"));
        assert_eq!(
            history[0].planning_reason.as_deref(),
            Some("next_profile_missing")
        );
        assert_eq!(history[0].timing_direction, "late");
    }

    #[test]
    fn timing_quality_labels_delta_bands_and_missed() {
        assert_eq!(timing_quality(Some("fired"), Some(150)), "tight");
        assert_eq!(timing_quality(Some("fired"), Some(-500)), "usable");
        assert_eq!(timing_quality(Some("late"), Some(1000)), "loose");
        assert_eq!(timing_quality(Some("late"), Some(1001)), "bad");
        assert_eq!(timing_quality(Some("missed"), None), "bad");
        assert_eq!(timing_quality(Some("fired"), None), "bad");
    }

    #[test]
    fn timing_direction_labels_delta_direction_and_status() {
        assert_eq!(timing_direction(Some("fired"), Some(150)), "on_time");
        assert_eq!(timing_direction(Some("fired"), Some(-251)), "early");
        assert_eq!(timing_direction(Some("fired"), Some(251)), "late");
        assert_eq!(timing_direction(Some("late"), Some(0)), "late");
        assert_eq!(timing_direction(Some("missed"), None), "missed");
        assert_eq!(timing_direction(Some("armed"), None), "pending");
        assert_eq!(timing_direction(Some("fired"), None), "unknown");
    }

    #[test]
    fn timing_history_summary_counts_quality_and_status() {
        let events = vec![
            DjTimingHistoryEvent {
                event_id: 1,
                from_title: Some("A".to_string()),
                from_artist: Some("Artist A".to_string()),
                to_title: Some("B".to_string()),
                to_artist: Some("Artist B".to_string()),
                planned_template: "SafeCrossfade".to_string(),
                renderer_template: Some("SafeCrossfade".to_string()),
                planning_reason: None,
                planned_start_ms: Some(10_000),
                actual_start_ms: Some(10_100),
                timing_delta_ms: Some(100),
                timing_source: Some("downbeat_sync".to_string()),
                timing_status: Some("fired".to_string()),
                timing_quality: "tight".to_string(),
                timing_direction: "on_time".to_string(),
                started_at: "now".to_string(),
            },
            DjTimingHistoryEvent {
                event_id: 2,
                from_title: Some("B".to_string()),
                from_artist: Some("Artist B".to_string()),
                to_title: Some("C".to_string()),
                to_artist: Some("Artist C".to_string()),
                planned_template: "SafeCrossfade".to_string(),
                renderer_template: Some("SafeCrossfade".to_string()),
                planning_reason: Some("next_profile_missing".to_string()),
                planned_start_ms: Some(20_000),
                actual_start_ms: Some(20_800),
                timing_delta_ms: Some(800),
                timing_source: Some("beat_sync".to_string()),
                timing_status: Some("late".to_string()),
                timing_quality: "loose".to_string(),
                timing_direction: "late".to_string(),
                started_at: "now".to_string(),
            },
            DjTimingHistoryEvent {
                event_id: 3,
                from_title: Some("C".to_string()),
                from_artist: Some("Artist C".to_string()),
                to_title: Some("D".to_string()),
                to_artist: Some("Artist D".to_string()),
                planned_template: "SafeCrossfade".to_string(),
                renderer_template: Some("SafeCrossfade".to_string()),
                planning_reason: Some("analysis_late".to_string()),
                planned_start_ms: Some(30_000),
                actual_start_ms: None,
                timing_delta_ms: None,
                timing_source: Some("fallback_overlap".to_string()),
                timing_status: Some("missed".to_string()),
                timing_quality: "bad".to_string(),
                timing_direction: "missed".to_string(),
                started_at: "now".to_string(),
            },
        ];

        let summary = summarize_timing_history(&events);

        assert_eq!(summary.event_count, 3);
        assert_eq!(summary.average_delta_ms, Some(450));
        assert_eq!(summary.average_abs_delta_ms, Some(450));
        assert_eq!(summary.tight_count, 1);
        assert_eq!(summary.loose_count, 1);
        assert_eq!(summary.bad_count, 1);
        assert_eq!(summary.late_count, 1);
        assert_eq!(summary.missed_count, 1);
    }

    #[test]
    fn profile_rebuild_inflight_reports_running_for_recent_duplicate() {
        let inflight = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let first = mark_dj_profile_rebuild_inflight(
            &inflight,
            "tidal_track:250295727",
            std::time::Duration::from_secs(60),
        )
        .expect("first mark");
        let second = mark_dj_profile_rebuild_inflight(
            &inflight,
            "tidal_track:250295727",
            std::time::Duration::from_secs(60),
        )
        .expect("second mark");

        assert_eq!(first, ProfileRebuildInflightDecision::Start);
        assert_eq!(second, ProfileRebuildInflightDecision::AlreadyRunning);
    }
}
