use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

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
    DJ_WAVEFORM_PEAK_COUNT, decode_f32_blob, decode_u32_blob, dj_profile_row_is_current,
    encode_f32_blob,
};
use crate::services::tidal::stream as tidal_stream;

mod transition_history;
use transition_history::*;

mod deck_status;
pub(crate) use deck_status::*;

mod profile_rebuild;
pub(crate) use profile_rebuild::*;

mod ready_pair;
pub(in crate::server::routes) use ready_pair::*;

const DEFAULT_DJ_LOOKAHEAD_DEADLINE_SAMPLES: u64 = 48_000 * 30;
const DJ_PROFILE_CONFIDENCE_FLOOR: f64 = 0.65;
const SAFE_SUGGESTION_BAD_COUNT: i64 = 3;
const DJ_PROFILE_AUTO_REBUILD_RETRY_SECS: u64 = 300;
const DJ_PROFILE_TRANSIENT_RETRY_SECS: u64 = 25;
// After this many consecutive transient decode failures, a profile rebuild
// stops retrying and rests as decode_failed instead of hammering the source
// forever (e.g. a TIDAL stream that only ever resolves to ad segments). The
// DJ engine then falls back - bass swap on an unknown key - rather than
// waiting on a profile that will never arrive. The failure entry's TTL still
// lets a chronically-failing track try again fresh much later.
const DJ_PROFILE_MAX_TRANSIENT_ATTEMPTS: u32 = 4;
const DJ_PROFILE_MAX_RETRY_BACKOFF_SECS: u64 = 15 * 60;
const DJ_TIMING_HISTORY_LIMIT: i64 = 5;
const DJ_READY_PAIR_TRANSITION_WINDOW_MS: i64 = 30_000;
const DJ_TIMING_SANITY_MAX_DELTA_MS: i64 = 30_000;
const DROP_PREVIEW_MIN_POSITION_MS: i64 = 60_000;
const DROP_PREVIEW_FINAL_WINDOW_GUARD_MS: i64 = 45_000;
#[cfg(test)]
const DJ_READY_PAIR_PLANNING_RETRY_SECS: u64 = 15;
const DJ_PROFILE_REBUILD_FAILURE_TTL_SECS: u64 = 300;
// An exhausted source must not start a fresh burst every five minutes during
// the same session. Explicit Rebuild still clears this cooldown immediately.
const DJ_PROFILE_EXHAUSTED_FAILURE_TTL_SECS: u64 = 24 * 60 * 60;
const DJ_PROFILE_AUTO_REBUILD_MAX_ACTIVE: usize = 2;
const DJ_PROFILE_ANALYSIS_TIDAL_QUALITIES: [&str; 2] = ["LOW", "LOSSLESS"];
const MAX_MANUAL_DROP_MARKERS: usize = 16;
const MAX_MANUAL_DROP_MARKER_MS: i64 = 30 * 60 * 1_000;

#[cfg(test)]
type ReadyPairPlanningKey = (i64, u64);

#[cfg(test)]
#[allow(dead_code)]
static READY_PAIR_PLANNING_ATTEMPTS: OnceLock<Mutex<HashMap<ReadyPairPlanningKey, Instant>>> =
    OnceLock::new();
static DJ_PROFILE_REBUILD_FAILURES: OnceLock<Mutex<HashMap<String, DjProfileRebuildFailure>>> =
    OnceLock::new();
static DJ_PROFILE_AUTO_REBUILD_ACTIVE: AtomicUsize = AtomicUsize::new(0);

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
    manual_drop_markers_ms: Option<Vec<i64>>,
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
    manual_drop_markers_ms: Vec<i64>,
    notes: Option<String>,
    applies: String,
}

#[derive(Debug, Deserialize)]
struct SetDjPolicyRequest {
    mix_intent: Option<String>,
    transition_speed_bias: Option<String>,
    preferred_strategy: Option<String>,
}

#[derive(Debug, Serialize)]
struct DjPolicyResponse {
    mix_intent: String,
    transition_speed_bias: String,
    preferred_strategy: String,
}

#[derive(Debug, Serialize)]
struct DjStatusResponse {
    enabled: bool,
    transition_plan: Option<noor_mix::TransitionProgram>,
    playback_position_ms: Option<i64>,
    feedback_transition_event_id: Option<i64>,
    active_transition: Option<DjActiveTransition>,
    current: Option<DjDeckStatus>,
    next: Option<DjDeckStatus>,
    planning_status: String,
    selected_program: Option<String>,
    planned_template: Option<String>,
    renderer_template: Option<String>,
    renderer_mode: Option<String>,
    downgrade_reason: Option<String>,
    planning_reason: Option<String>,
    sync_target: Option<String>,
    planned_start_ms: Option<i64>,
    runtime_planned_start_ms: Option<i64>,
    actual_start_ms: Option<i64>,
    timing_delta_ms: Option<i64>,
    timing_source: Option<String>,
    timing_status: Option<String>,
    timing_quality: String,
    timing_direction: String,
    runtime_rendered_dj_mixer: Option<bool>,
    runtime_renderer_status: Option<String>,
    runtime_renderer_reason: Option<String>,
    overlay_details: Option<DjOverlayDetails>,
    fallback_reason: Option<String>,
    rejected_alternatives: Vec<DjRejectedAlternative>,
    profile_confidence_floor: f64,
    last_transition_event_id: Option<i64>,
    recent_timing_events: Vec<DjTimingHistoryEvent>,
    timing_history_summary: DjTimingHistorySummary,
    safe_crossfade_suggestion: Option<DjSafeSuggestion>,
    drop_preview: DjDropPreviewStatus,
}

#[derive(Debug, Serialize)]
struct DjDeckStatus {
    media_ref_kind: String,
    media_ref_id: String,
    title: String,
    artist: Option<String>,
    profile_ready: bool,
    profile_status: String,
    profile_error: Option<String>,
    profile_retry_after_ms: Option<i64>,
    profile_retry_reason: Option<String>,
    profile_confidence: Option<f64>,
    beat_confidence: Option<f64>,
    grid_is_synthetic: bool,
    analysis_scope_ms: Option<i64>,
    energy: Option<f64>,
    beat_count: Option<usize>,
    downbeat_count: Option<usize>,
    phrase_count: Option<usize>,
    waveform_status: String,
    waveform_peaks: Vec<f32>,
    beat_markers_ms: Vec<i64>,
    downbeat_markers_ms: Vec<i64>,
    phrase_markers_ms: Vec<i64>,
    drop_markers_ms: Vec<i64>,
    manual_drop_markers_ms: Vec<i64>,
    mix_in_markers_ms: Vec<i64>,
    mix_out_markers_ms: Vec<i64>,
    passive_analysis_status: Option<String>,
    passive_analysis_reason: Option<String>,
    safe_crossfade_only: bool,
}

#[derive(Debug, Serialize)]
struct DjActiveTransition {
    event_id: i64,
    outgoing: DjDeckStatus,
    incoming: DjDeckStatus,
    program: noor_mix::TransitionProgram,
    start_ms: i64,
    actual_start_ms: i64,
    elapsed_ms: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct DjDropPreviewStatus {
    status: String,
    planned_fire_ms: Option<i64>,
    actual_fire_ms: Option<i64>,
    incoming_drop_ms: Option<i64>,
    source: Option<String>,
    reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DropPreviewPlan {
    pub(crate) planned_fire_ms: i64,
    pub(crate) incoming_drop_ms: i64,
    pub(crate) source: String,
}

#[derive(Debug, Serialize)]
struct DjSafeSuggestion {
    media_ref_kind: String,
    media_ref_id: String,
    bad_feedback_count: i64,
}

#[derive(Debug, Serialize, PartialEq)]
struct DjTimingHistoryEvent {
    event_id: i64,
    from_title: Option<String>,
    from_artist: Option<String>,
    to_title: Option<String>,
    to_artist: Option<String>,
    planned_template: String,
    renderer_template: Option<String>,
    planning_reason: Option<String>,
    rejected_alternatives: Vec<DjRejectedAlternative>,
    planned_start_ms: Option<i64>,
    runtime_planned_start_ms: Option<i64>,
    actual_start_ms: Option<i64>,
    timing_delta_ms: Option<i64>,
    timing_source: Option<String>,
    timing_status: Option<String>,
    timing_quality: String,
    timing_direction: String,
    runtime_rendered_dj_mixer: Option<bool>,
    runtime_renderer_status: Option<String>,
    runtime_renderer_reason: Option<String>,
    started_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct DjOverlayDetails {
    overlay_status: String,
    overlay_start_ms: Option<i64>,
    overlay_end_ms: Option<i64>,
    tempo_ratio: Option<f64>,
    deck_b_start_frame: u64,
    drop_marker_ms: Option<i64>,
    drop_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct DjRejectedAlternative {
    template: String,
    score: f64,
    reason: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct DjTimingHistorySummary {
    event_count: usize,
    average_delta_ms: Option<i64>,
    average_abs_delta_ms: Option<i64>,
    median_abs_delta_ms: Option<i64>,
    worst_abs_delta_ms: Option<i64>,
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
    overlay_details: Option<DjOverlayDetails>,
    runtime_rendered_dj_mixer: Option<bool>,
    runtime_renderer_status: Option<String>,
    runtime_renderer_reason: Option<String>,
    rejected_alternatives: Vec<DjRejectedAlternative>,
}

#[derive(Debug, PartialEq)]
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
    runtime_rendered_dj_mixer: Option<bool>,
    runtime_renderer_status: Option<String>,
    runtime_renderer_reason: Option<String>,
    overlay_details: Option<DjOverlayDetails>,
    rejected_alternatives: Vec<DjRejectedAlternative>,
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct DjProfileRebuildFailure {
    status: String,
    message: String,
    retry_reason: Option<String>,
    next_retry_at: Option<Instant>,
    recorded_at: Instant,
    /// Consecutive transient-failure count for this media ref, carried across
    /// automatic retries so the backoff grows and the loop eventually gives up.
    attempts: u32,
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
        let lookahead = state_guard
            .db
            .with_conn(|conn| {
                queries::set_dj_engine_enabled(conn, payload.enabled)?;
                if payload.enabled {
                    let pair = super::active_dj_pair_for_state_and_conn(&state_guard, conn)?;
                    Ok(player::dj_lookahead_start_from_pair(
                        pair,
                        DEFAULT_DJ_LOOKAHEAD_DEADLINE_SAMPLES,
                    ))
                } else {
                    Ok(None)
                }
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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
                let generation = super::current_playback_generation(&*state.read().await);
                super::spawn_dj_pair_preparation(
                    state.clone(),
                    runtime.clone(),
                    lookahead,
                    generation,
                );
            }
        } else {
            let _ = runtime.start_dj_lookahead(None, None, None, None, u64::MAX, 0);
        }
    }
    if payload.enabled {
        queue_missing_dj_profiles_for_current_pair(state.clone()).await?;
    }

    Ok(Json(EnabledResponse {
        enabled: payload.enabled,
    }))
}

async fn get_status(
    State(state): State<SharedState>,
) -> Result<Json<DjStatusResponse>, StatusCode> {
    let response = {
        let state = state.read().await;
        let ephemeral_labels: Vec<(AudioDjProfileKey, (String, Option<String>))> = Vec::new();
        let active_track_id = state
            .playback_runtime_info
            .as_ref()
            .and_then(|info| info.active_track_id);
        let active_generation = super::current_playback_generation(&state);
        state
            .db
            .with_conn(|conn| {
                let enabled = queries::is_dj_engine_enabled(conn)?;
                let pair = super::active_dj_pair_for_state_and_conn(&state, conn)?;
                let current_ref = pair.current.clone();
                let next_ref = pair.next.clone();
                let preview_outcome = state.last_drop_preview.filter(|preview| {
                    Some(preview.track_id) == active_track_id
                        && preview.generation == active_generation
                        && preview.queue_generation == pair.queue_generation
                });
                let drop_preview_actual_fire_ms = preview_outcome.and_then(|preview| preview.actual_fire_ms);
                let current = match pair.current {
                    Some(media_ref) => {
                        let key = media_ref.profile_key();
                        let label = ephemeral_labels
                            .iter()
                            .find(|(candidate, _)| candidate == &key)
                            .map(|(_, label)| label);
                        let inflight_key = dj_profile_inflight_key(&key);
                        let rebuild_inflight = dj_profile_rebuild_is_inflight(
                            &state.dj_profile_rebuild_inflight,
                            &inflight_key,
                        );
                        Some(deck_status(conn, &media_ref, label, rebuild_inflight)?)
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
                        let inflight_key = dj_profile_inflight_key(&key);
                        let rebuild_inflight = dj_profile_rebuild_is_inflight(
                            &state.dj_profile_rebuild_inflight,
                            &inflight_key,
                        );
                        Some(deck_status(conn, &media_ref, label, rebuild_inflight)?)
                    }
                    None => None,
                };
                let safe_crossfade_suggestion =
                    safe_crossfade_suggestion(conn, current.as_ref(), next.as_ref())?;
                let fallback_reason = if !enabled {
                    Some("disabled".to_string())
                } else if current.is_none() || next.is_none() {
                    Some("pair_missing".to_string())
                } else if current
                    .as_ref()
                    .is_some_and(|deck| deck.profile_status == "decode_failed")
                {
                    Some("current_profile_decode_failed".to_string())
                } else if next
                    .as_ref()
                    .is_some_and(|deck| deck.profile_status == "decode_failed")
                {
                    Some("next_profile_decode_failed".to_string())
                } else if current.as_ref().is_some_and(|deck| !deck.profile_ready) {
                    Some("missing_current_profile".to_string())
                } else if next.as_ref().is_some_and(|deck| !deck.profile_ready) {
                    Some("missing_next_profile".to_string())
                } else {
                    None
                };
                let latest_transition =
                    latest_open_transition_for_pair(conn, current_ref.as_ref(), next_ref.as_ref())?;
                let transition_plan = latest_transition.as_ref().and_then(|event| {
                    conn.query_row(
                        "SELECT program_json FROM dj_transition_events WHERE id = ?1",
                        [event.id],
                        |row| row.get::<_, String>(0),
                    ).ok().and_then(|json| serde_json::from_str(&json).ok())
                });
                let playback_position_ms = state.playback_runtime.as_ref().zip(
                    state.playback_runtime_info.as_ref(),
                ).map(|(runtime, info)| runtime.handle.get_position_ms(info.sample_rate, info.channels));
                let active_transition = if enabled {
                    let elapsed_ms = state.playback_runtime.as_ref().zip(
                        state.playback_runtime_info.as_ref(),
                    ).and_then(|(runtime, info)| runtime.handle.get_dj_handoff_elapsed_ms(info.sample_rate, info.channels));
                    active_transition_for_runtime(conn,
                        state.active_listen_session.as_ref()
                            .filter(|session| Some(session.track_id) == active_track_id),
                        current_ref.as_ref(), elapsed_ms)?
                } else { None };
                let feedback_transition_event_id = conn.query_row(
                    "SELECT id FROM dj_transition_events
                     WHERE actual_start_ms IS NOT NULL
                       AND timing_status IN ('fired', 'late')
                       AND runtime_renderer_status IN ('rendered_handoff', 'rendered_overlay', 'legacy_overlap')
                     ORDER BY id DESC LIMIT 1",
                    [], |row| row.get::<_, i64>(0),
                ).optional()?;
                let ready_pair_due = active_track_id
                    .and_then(|track_id| {
                        let duration_ms = current_track_duration_ms(conn, track_id).ok().flatten();
                        let runtime = state.playback_runtime.as_ref()?;
                        let info = state.playback_runtime_info.as_ref()?;
                        if info.active_track_id != Some(track_id) {
                            return None;
                        }
                        Some(ready_pair_transition_due(
                            runtime
                                .handle
                                .get_position_ms(info.sample_rate, info.channels),
                            duration_ms,
                        ))
                    })
                    .unwrap_or(false);
                let planning_status = pair_planning_status(
                    enabled,
                    current.as_ref(),
                    next.as_ref(),
                    latest_transition.as_ref(),
                    ready_pair_due,
                )
                .to_string();
                let recent_timing_events =
                    latest_dj_transition_timing_history(conn, DJ_TIMING_HISTORY_LIMIT)?;
                let tuning_deltas = latest_fired_dj_timing_deltas(conn, 20)?;
                let timing_history_summary =
                    summarize_timing_history(&recent_timing_events, &tuning_deltas);
                let mut renderer_status =
                    renderer_status_for_transition(latest_transition.as_ref());
                renderer_status.overlay_details =
                    annotate_overlay_drop_source(renderer_status.overlay_details, next.as_ref());
                let mut drop_preview = drop_preview_status(
                    conn,
                    enabled,
                    current_ref.as_ref(),
                    next_ref.as_ref(),
                    current.as_ref(),
                    next.as_ref(),
                    active_track_id.and_then(|track_id| {
                        current_track_duration_ms(conn, track_id).ok().flatten()
                    }),
                    drop_preview_actual_fire_ms,
                )?;
                if let Some(reason) = preview_outcome.and_then(|preview| preview.skipped_reason) {
                    drop_preview.status = "skipped".into();
                    drop_preview.actual_fire_ms = None;
                    drop_preview.reason = Some(reason.into());
                }
                Ok(DjStatusResponse {
                    enabled,
                    transition_plan,
                    playback_position_ms,
                    feedback_transition_event_id,
                    active_transition,
                    current,
                    next,
                    planning_status,
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
                    runtime_planned_start_ms: latest_transition.as_ref().and_then(|event| {
                        conn.query_row("SELECT runtime_planned_start_ms FROM dj_transition_events WHERE id = ?1",
                            [event.id], |row| row.get::<_, Option<i64>>(0)).ok().flatten()
                    }),
                    actual_start_ms: renderer_status.actual_start_ms,
                    timing_delta_ms: renderer_status.timing_delta_ms,
                    timing_source: renderer_status.timing_source,
                    timing_status: renderer_status.timing_status,
                    timing_quality: renderer_status.timing_quality,
                    timing_direction: renderer_status.timing_direction,
                    runtime_rendered_dj_mixer: renderer_status.runtime_rendered_dj_mixer,
                    runtime_renderer_status: renderer_status.runtime_renderer_status,
                    runtime_renderer_reason: renderer_status.runtime_renderer_reason,
                    overlay_details: renderer_status.overlay_details,
                    fallback_reason,
                    rejected_alternatives: renderer_status.rejected_alternatives,
                    profile_confidence_floor: DJ_PROFILE_CONFIDENCE_FLOOR,
                    last_transition_event_id: latest_transition
                        .as_ref()
                        .map(|transition| transition.id),
                    recent_timing_events,
                    timing_history_summary,
                    safe_crossfade_suggestion,
                    drop_preview,
                })
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    Ok(Json(response))
}

async fn get_profile(
    State(state): State<SharedState>,
    Path(track_id): Path<i64>,
) -> Result<Json<ProfileResponse>, StatusCode> {
    if track_id <= 0 {
        return Err(StatusCode::BAD_REQUEST);
    }

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
    let (mix_intent, transition_speed_bias, preferred_strategy) = {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| {
                let (intent, speed) = queries::get_dj_global_policy(conn)?;
                Ok((intent, speed, queries::get_dj_preferred_strategy(conn)?))
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    Ok(Json(DjPolicyResponse {
        mix_intent,
        transition_speed_bias,
        preferred_strategy,
    }))
}

async fn set_policy(
    State(state): State<SharedState>,
    Json(payload): Json<SetDjPolicyRequest>,
) -> Result<Json<DjPolicyResponse>, StatusCode> {
    let (mix_intent, transition_speed_bias, preferred_strategy) = {
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
                let preferred_strategy = payload
                    .preferred_strategy
                    .clone()
                    .unwrap_or(queries::get_dj_preferred_strategy(conn)?);
                // All policy fields are committed together; an invalid
                // strategy must not partially save the other controls.
                let transaction = conn.unchecked_transaction()?;
                queries::set_dj_preferred_strategy(&transaction, &preferred_strategy)?;
                queries::set_dj_global_policy(&transaction, &mix_intent, &transition_speed_bias)?;
                transaction.commit()?;
                Ok((mix_intent, transition_speed_bias, preferred_strategy))
            })
            .map_err(|_| StatusCode::BAD_REQUEST)?
    };
    let runtime = state
        .read()
        .await
        .playback_runtime
        .as_ref()
        .map(|r| r.handle.clone());
    if let Some(runtime) = runtime
        && let Err(error) = super::refresh_prepared_dj_transition(&state, &runtime).await
    {
        tracing::warn!("DJ policy saved; prepared transition refresh skipped: {error:?}");
    }
    Ok(Json(DjPolicyResponse {
        mix_intent,
        transition_speed_bias,
        preferred_strategy,
    }))
}

async fn record_feedback(
    State(state): State<SharedState>,
    Json(payload): Json<DjFeedbackRequest>,
) -> Result<Json<FeedbackResponse>, StatusCode> {
    feedback_rating(&payload.rating).ok_or(StatusCode::BAD_REQUEST)?;
    let id = payload.transition_event_id.ok_or(StatusCode::BAD_REQUEST)?;
    let accepted = state
        .read()
        .await
        .db
        .with_conn(|conn| {
            queries::record_dj_feedback(conn, id, &payload.rating, payload.reason.as_deref())
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if !accepted {
        return Err(StatusCode::CONFLICT);
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
    let key = AudioDjProfileKey {
        media_ref_kind: payload.media_ref_kind.clone(),
        media_ref_id: payload.media_ref_id.clone(),
    };
    let manual_drop_blob = if let Some(markers) = payload.manual_drop_markers_ms {
        let manual_drop_markers_ms = normalize_manual_drop_markers_ms(Some(markers))?;
        encode_marker_ms_blob(&manual_drop_markers_ms)
    } else {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| existing_manual_drop_blob(conn, &key))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    let now = Utc::now().to_rfc3339();
    let row = AudioDjProfileCorrectionRow {
        media_ref_kind: payload.media_ref_kind,
        media_ref_id: payload.media_ref_id,
        bpm_multiplier: payload.bpm_multiplier,
        downbeat_offset_beats: payload.downbeat_offset_beats,
        phrase_offset_bars: payload.phrase_offset_bars,
        safe_crossfade_only: payload.safe_crossfade_only.unwrap_or(false),
        transition_speed_bias: payload.transition_speed_bias,
        manual_drop_blob,
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

fn existing_manual_drop_blob(
    conn: &rusqlite::Connection,
    key: &AudioDjProfileKey,
) -> anyhow::Result<Vec<u8>> {
    queries::get_audio_dj_profile_correction(conn, key)
        .map(|row| row.map(|row| row.manual_drop_blob).unwrap_or_default())
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
    Ok(
        queries::get_audio_dj_profile(conn, key)?
            .is_some_and(|row| dj_profile_row_is_current(&row)),
    )
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
        manual_drop_markers_ms: decode_marker_blob_ms(&row.manual_drop_blob),
        notes: row.notes,
        applies: "next_transition".to_string(),
    }
}

fn normalize_manual_drop_markers_ms(markers: Option<Vec<i64>>) -> Result<Vec<i64>, StatusCode> {
    let mut markers = markers.unwrap_or_default();
    if markers.len() > MAX_MANUAL_DROP_MARKERS
        || markers
            .iter()
            .any(|marker| *marker < 0 || *marker > MAX_MANUAL_DROP_MARKER_MS)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    markers.sort_unstable();
    markers.dedup();
    Ok(markers)
}

fn encode_marker_ms_blob(markers_ms: &[i64]) -> Vec<u8> {
    let seconds: Vec<f32> = markers_ms
        .iter()
        .map(|marker| *marker as f32 / 1000.0)
        .collect();
    encode_f32_blob(&seconds)
}

fn decode_marker_blob_ms(blob: &[u8]) -> Vec<i64> {
    decode_f32_blob(blob)
        .map(|values| seconds_markers_ms(&values))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
