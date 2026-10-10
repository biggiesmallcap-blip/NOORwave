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

pub(super) async fn queue_missing_dj_profiles_for_current_pair(
    state: SharedState,
) -> Result<usize, StatusCode> {
    let missing_profile_refs = {
        let state_guard = state.read().await;
        let ephemeral_labels: Vec<(AudioDjProfileKey, (String, Option<String>))> = Vec::new();
        state_guard
            .db
            .with_conn(|conn| {
                if !queries::is_dj_engine_enabled(conn)? {
                    return Ok(Vec::new());
                }
                let pair = super::active_dj_pair_for_state_and_conn(&state_guard, conn)?;
                missing_dj_profile_refs_for_pair(
                    conn,
                    pair,
                    &ephemeral_labels,
                    &state_guard.dj_profile_rebuild_inflight,
                )
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    let mut attempted = 0usize;
    for media_ref in missing_profile_refs {
        queue_profile_rebuild_if_idle(state.clone(), media_ref).await?;
        attempted += 1;
    }
    Ok(attempted)
}

fn missing_dj_profile_refs_for_pair(
    conn: &rusqlite::Connection,
    pair: crate::playback::dj_lookahead::DjLookaheadPair,
    ephemeral_labels: &[(AudioDjProfileKey, (String, Option<String>))],
    inflight: &Arc<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>>,
) -> anyhow::Result<Vec<DjMediaRef>> {
    let mut missing = Vec::new();
    for media_ref in [pair.current, pair.next].into_iter().flatten() {
        if unsupported_auto_profile_rebuild_status(&media_ref).is_some() {
            continue;
        }
        let key = media_ref.profile_key();
        let label = ephemeral_labels
            .iter()
            .find(|(candidate, _)| candidate == &key)
            .map(|(_, label)| label);
        let inflight_key = dj_profile_inflight_key(&key);
        let rebuild_inflight = dj_profile_rebuild_is_inflight(inflight, &inflight_key);
        let deck = deck_status(conn, &media_ref, label, rebuild_inflight)?;
        let outdated_analysis = queries::get_audio_dj_profile(conn, &key)?.is_some_and(|profile| {
            profile.source.starts_with("dj_playback")
                && profile.profile_version
                    != crate::services::audio_analysis::dj_profile::DJ_PROFILE_VERSION
        });
        if deck_needs_profile_rebuild(&deck)
            || (outdated_analysis
                && !rebuild_inflight
                && (deck.profile_status == "ready"
                    || (deck.profile_status == "retrying"
                        && deck.profile_retry_after_ms.unwrap_or(0) <= 0)))
        {
            missing.push(media_ref);
        }
    }
    Ok(missing)
}

#[cfg(test)]
#[allow(dead_code)]
async fn ready_pair_transition_is_due(
    state: &SharedState,
    current_track_id: i64,
    generation: u64,
) -> bool {
    let state_guard = state.read().await;
    let Some(info) = state_guard.playback_runtime_info.as_ref() else {
        return false;
    };
    if info.active_track_id != Some(current_track_id)
        || state_guard
            .playback_generation
            .load(std::sync::atomic::Ordering::Relaxed)
            != generation
    {
        return false;
    }
    let Some(runtime) = state_guard.playback_runtime.as_ref() else {
        return false;
    };
    let position_ms = runtime
        .handle
        .get_position_ms(info.sample_rate, info.channels);
    let duration_ms = state_guard
        .db
        .with_conn(|conn| current_track_duration_ms(conn, current_track_id))
        .ok()
        .flatten();
    ready_pair_transition_due(position_ms, duration_ms)
}

fn current_track_duration_ms(
    conn: &rusqlite::Connection,
    current_track_id: i64,
) -> anyhow::Result<Option<i64>> {
    conn.query_row(
        "SELECT duration_ms FROM tracks WHERE id = ?1",
        [current_track_id],
        |row| row.get::<_, Option<i64>>(0),
    )
    .optional()
    .map(|value| value.flatten())
    .map_err(anyhow::Error::from)
}

fn ready_pair_transition_due(position_ms: i64, duration_ms: Option<i64>) -> bool {
    let Some(duration_ms) = duration_ms else {
        return false;
    };
    duration_ms.saturating_sub(position_ms.max(0)) <= DJ_READY_PAIR_TRANSITION_WINDOW_MS
}

fn pair_planning_status(
    enabled: bool,
    current: Option<&DjDeckStatus>,
    next: Option<&DjDeckStatus>,
    latest_transition: Option<&OpenTransition>,
    ready_pair_due: bool,
) -> &'static str {
    if !enabled {
        return "disabled";
    }
    let (Some(current), Some(next)) = (current, next) else {
        return "pair_missing";
    };
    if current.profile_status == "decode_failed" || next.profile_status == "decode_failed" {
        return "profile_failed";
    }
    if !current.profile_ready || !next.profile_ready {
        return "waiting_for_profiles";
    }
    if let Some(transition) = latest_transition {
        return if transition.timing_status.as_deref() == Some("missed") {
            "missed"
        } else {
            "armed"
        };
    }
    if ready_pair_due {
        "ready_to_plan"
    } else {
        "waiting_for_window"
    }
}

#[cfg(test)]
#[allow(dead_code)]
fn claim_ready_pair_transition_planning(current_track_id: i64, generation: u64) -> bool {
    let attempts = READY_PAIR_PLANNING_ATTEMPTS.get_or_init(|| Mutex::new(HashMap::new()));
    let now = Instant::now();
    let mut attempts = attempts.lock().unwrap_or_else(|error| error.into_inner());
    claim_ready_pair_transition_planning_at(&mut attempts, current_track_id, generation, now)
}

#[cfg(test)]
fn claim_ready_pair_transition_planning_at(
    attempts: &mut HashMap<ReadyPairPlanningKey, Instant>,
    current_track_id: i64,
    generation: u64,
    now: Instant,
) -> bool {
    let retry_after = Duration::from_secs(DJ_READY_PAIR_PLANNING_RETRY_SECS);
    attempts.retain(|_, last_attempt| now.duration_since(*last_attempt) < retry_after);
    let key = (current_track_id, generation);
    if let Some(last_attempt) = attempts.get(&key)
        && now.duration_since(*last_attempt) < retry_after
    {
        return false;
    }
    attempts.insert(key, now);
    true
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
                let pair = crate::playback::dj_lookahead::load_dj_lookahead_pair(conn)?;

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
        if recent_dj_profile_rebuild_failure(&dj_profile_inflight_key(&media_key))
            .is_some_and(|failure| failure.status == "source_unavailable")
        {
            return Ok(RebuildDjProfileResponse {
                accepted: false,
                status: "source_unavailable".to_string(),
            });
        }
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
    let auto_slot = if force {
        None
    } else {
        match try_claim_auto_dj_profile_rebuild_slot() {
            Some(slot) => Some(slot),
            None => {
                tracing::debug!(
                    media_ref_kind = %media_key.media_ref_kind,
                    media_ref_id = %media_key.media_ref_id,
                    max_active = DJ_PROFILE_AUTO_REBUILD_MAX_ACTIVE,
                    "Skipping automatic DJ profile rebuild: active rebuild limit reached"
                );
                return Ok(RebuildDjProfileResponse {
                    accepted: false,
                    status: "busy".to_string(),
                });
            }
        }
    };
    let inflight_key = dj_profile_inflight_key(&media_key);
    let inflight = {
        let state_guard = state.read().await;
        state_guard.dj_profile_rebuild_inflight.clone()
    };
    let retry_after = if force {
        std::time::Duration::ZERO
    } else {
        std::time::Duration::from_secs(DJ_PROFILE_AUTO_REBUILD_RETRY_SECS)
    };
    match mark_dj_profile_rebuild_inflight(&inflight, &inflight_key, retry_after)? {
        ProfileRebuildInflightDecision::Start => {
            // Only a user-forced rebuild resets the failure history. An
            // automatic re-accept must preserve the attempt counter, or the
            // backoff/give-up logic can never converge and the rebuild loops
            // forever on a permanently-failing stream.
            if force {
                clear_dj_profile_rebuild_failure(&inflight_key);
            }
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
        state_guard.tidal.tokens()
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

    let requests = dj_profile_analysis_stream_requests(tidal_id);
    let track = rebuild_track_for_tidal_ref(&state, tidal_id).await;
    let (http_client, generation, runtime) = {
        let state_guard = state.read().await;
        (
            state_guard.http_client.clone(),
            state_guard
                .playback_generation
                .load(std::sync::atomic::Ordering::Relaxed),
            state_guard
                .playback_runtime
                .as_ref()
                .map(|runtime| runtime.handle.clone()),
        )
    };
    let config = PlaybackRuntimeConfig::new(http_client, tokens.access_token, None)
        .with_stream_resolver(super::runtime_stream_resolver(state.clone()))
        .with_dj_analysis(true, Some(dj_analysis_tx))
        .for_dj_analysis_only();

    let inflight_for_decode = inflight.clone();
    let inflight_key_for_decode = inflight_key.clone();
    let failure_key = inflight_key.clone();
    let retry_runtime = tokio::runtime::Handle::current();
    let retry_state = state.clone();
    let auto_slot_for_decode = auto_slot;
    tokio::task::spawn_blocking(move || {
        let _auto_slot = auto_slot_for_decode;
        // Prefer the fresh manifest that already produced this deck's audio.
        // The normal quality fallback remains available if it cannot be reused.
        let mut resolved = runtime.and_then(|runtime| runtime.resolved_analysis_stream(track.id));
        let mut last_error = None;
        let mut decoded = false;
        for (attempt_index, request) in requests.into_iter().enumerate() {
            let quality = request.audio_quality.clone();
            let mut job = player::PreparedPlaybackJob::new(
                track.clone(),
                PlaybackSourceRequest::TidalStream(request),
                GaplessPlan::disabled(),
            )
            .with_generation(generation)
            .with_dj_media_ref(media_ref.clone());
            if let Some(info) = resolved.take() {
                job.resolved_stream = Some(player::ResolvedStream {
                    info,
                    resolved_at: Instant::now(),
                });
            }
            let shared = dj_profile_rebuild_shared(track.id, generation);
            match decode_and_buffer_job(config.clone(), job, shared, 48_000, 2) {
                Ok(()) => {
                    decoded = true;
                    break;
                }
                Err(error) => {
                    if let Some(next_quality) =
                        next_dj_profile_analysis_quality(attempt_index, &error)
                    {
                        tracing::info!(
                            tidal_id,
                            quality,
                            next_quality,
                            error = %error,
                            "DJ profile rebuild retrying with fallback TIDAL quality"
                        );
                        last_error = Some(error);
                    } else {
                        last_error = Some(error);
                        break;
                    }
                }
            }
        }

        if decoded {
            clear_dj_profile_rebuild_failure(&failure_key);
            tracing::info!(tidal_id, "DJ profile rebuild decode queued analysis");
        } else if let Some(error) = last_error {
            let status = profile_rebuild_failure_status(&error);
            let message = profile_rebuild_error_message(&error, status);
            // The returned delay is the backoff for the next attempt, or None
            // once the attempt cap flips the failure to terminal decode_failed
            // - in which case we do NOT schedule another retry, ending the loop.
            let retry_delay = finish_dj_profile_rebuild_failure(
                &inflight_for_decode,
                &inflight_key_for_decode,
                status,
                message.clone(),
            );
            if let Some(delay) = retry_delay {
                schedule_dj_profile_retry(&retry_runtime, retry_state, delay);
            }
            tracing::warn!(tidal_id, error = %message, "DJ profile rebuild decode failed");
        }
    });

    Ok(RebuildDjProfileResponse {
        accepted: true,
        status: "accepted".to_string(),
    })
}

#[must_use]
struct AutoDjProfileRebuildSlot {
    counter: &'static AtomicUsize,
}

impl Drop for AutoDjProfileRebuildSlot {
    fn drop(&mut self) {
        release_auto_dj_profile_rebuild_slot(self.counter);
    }
}

fn try_claim_auto_dj_profile_rebuild_slot() -> Option<AutoDjProfileRebuildSlot> {
    try_claim_auto_dj_profile_rebuild_slot_from(
        &DJ_PROFILE_AUTO_REBUILD_ACTIVE,
        DJ_PROFILE_AUTO_REBUILD_MAX_ACTIVE,
    )
}

fn try_claim_auto_dj_profile_rebuild_slot_from(
    counter: &'static AtomicUsize,
    max_active: usize,
) -> Option<AutoDjProfileRebuildSlot> {
    let mut active = counter.load(Ordering::Acquire);
    loop {
        if active >= max_active {
            return None;
        }
        match counter.compare_exchange_weak(active, active + 1, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => return Some(AutoDjProfileRebuildSlot { counter }),
            Err(actual) => active = actual,
        }
    }
}

fn release_auto_dj_profile_rebuild_slot(counter: &AtomicUsize) {
    let _ = counter.try_update(Ordering::AcqRel, Ordering::Acquire, |active| {
        active.checked_sub(1)
    });
}

fn dj_profile_analysis_stream_requests(tidal_id: i64) -> Vec<tidal_stream::StreamRequest> {
    DJ_PROFILE_ANALYSIS_TIDAL_QUALITIES
        .iter()
        .map(|quality| tidal_stream::StreamRequest::new(tidal_id, *quality))
        .collect()
}

fn next_dj_profile_analysis_quality(
    attempt_index: usize,
    error: &anyhow::Error,
) -> Option<&'static str> {
    // Fall back to the next quality tier when a different tier might dodge the
    // failure: TIDAL asset-not-ready, or a DASH segment / prebuffer failure
    // such as the LOW/AAC tier routing to an unreachable ad CDN. Previously
    // only asset-not-ready fell back, so a LOW-tier CDN timeout gave up without
    // ever trying the LOSSLESS stream that resolves fine.
    if !profile_rebuild_error_is_asset_not_ready_chain(error)
        && !profile_rebuild_error_is_retryable_chain(error)
    {
        return None;
    }
    DJ_PROFILE_ANALYSIS_TIDAL_QUALITIES
        .get(attempt_index + 1)
        .copied()
}

fn dj_profile_rebuild_shared(track_id: i64, generation: u64) -> Arc<PlaybackSharedState> {
    let (command_tx, _command_rx) = std::sync::mpsc::channel::<PlaybackRuntimeCommand>();
    Arc::new(PlaybackSharedState::new(
        track_id,
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
    ))
}

async fn queue_profile_rebuild_if_idle(
    state: SharedState,
    media_ref: DjMediaRef,
) -> Result<(), StatusCode> {
    let key = media_ref.profile_key();
    if let Some(status) = unsupported_auto_profile_rebuild_status(&media_ref) {
        tracing::debug!(
            media_ref_kind = %key.media_ref_kind,
            media_ref_id = %key.media_ref_id,
            status,
            "Skipping automatic DJ profile rebuild for unsupported media ref"
        );
        return Ok(());
    }
    let dj_analysis_tx = {
        let state_guard = state.read().await;
        state_guard.dj_analysis_tx.clone()
    };
    let Some(dj_analysis_tx) = dj_analysis_tx else {
        tracing::debug!(
            media_ref_kind = %key.media_ref_kind,
            media_ref_id = %key.media_ref_id,
            "Skipping automatic DJ profile rebuild: analysis actor unavailable"
        );
        return Ok(());
    };
    let response = queue_tidal_profile_rebuild(state, media_ref, dj_analysis_tx, false).await?;
    if !response.accepted {
        tracing::debug!(
            media_ref_kind = %key.media_ref_kind,
            media_ref_id = %key.media_ref_id,
            status = %response.status,
            "Automatic DJ profile rebuild skipped"
        );
    }
    Ok(())
}

fn unsupported_auto_profile_rebuild_status(media_ref: &DjMediaRef) -> Option<&'static str> {
    match media_ref {
        DjMediaRef::TidalTrack { .. } => None,
        DjMediaRef::LibraryTrack { .. } | DjMediaRef::PendingQueueItem { .. } => {
            Some("source_unavailable")
        }
    }
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

fn deck_needs_profile_rebuild(deck: &DjDeckStatus) -> bool {
    (matches!(deck.profile_status.as_str(), "missing" | "ready")
        || (deck.profile_status == "retrying" && deck.profile_retry_after_ms.unwrap_or(0) <= 0))
        && (!deck.profile_ready || deck.waveform_status == "missing")
}

#[cfg(test)]
fn ready_pair_can_request_transition_planning(
    current: Option<&DjDeckStatus>,
    next: Option<&DjDeckStatus>,
) -> bool {
    let (Some(current), Some(next)) = (current, next) else {
        return false;
    };
    current.profile_status != "decode_failed" && next.profile_status != "decode_failed"
}

fn dj_profile_rebuild_is_inflight(
    inflight: &Arc<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>>,
    key: &str,
) -> bool {
    inflight
        .lock()
        .ok()
        .and_then(|guard| guard.get(key).copied())
        .is_some_and(|started_at| {
            started_at.elapsed() < Duration::from_secs(DJ_PROFILE_AUTO_REBUILD_RETRY_SECS)
        })
}

fn clear_dj_profile_inflight(
    inflight: &Arc<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>>,
    key: &str,
) {
    if let Ok(mut guard) = inflight.lock() {
        guard.remove(key);
    }
}

/// Records the failure and releases the inflight slot. Returns the backoff
/// delay to schedule the next retry, or `None` once the attempt cap is hit
/// (terminal decode_failed - do not schedule another retry).
fn finish_dj_profile_rebuild_failure(
    inflight: &Arc<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>>,
    key: &str,
    status: &str,
    message: String,
) -> Option<Duration> {
    let retry_delay = record_dj_profile_rebuild_failure(key, status, message);
    clear_dj_profile_inflight(inflight, key);
    retry_delay
}

/// Exponential backoff for transient profile-rebuild retries: 25s, 50s, 100s,
/// 200s, ... doubling per attempt, capped at DJ_PROFILE_MAX_RETRY_BACKOFF_SECS.
fn profile_rebuild_backoff(attempts: u32) -> Duration {
    let shift = attempts.saturating_sub(1).min(6);
    let secs = DJ_PROFILE_TRANSIENT_RETRY_SECS
        .saturating_mul(1u64 << shift)
        .min(DJ_PROFILE_MAX_RETRY_BACKOFF_SECS);
    Duration::from_secs(secs)
}

fn schedule_dj_profile_retry(
    runtime: &tokio::runtime::Handle,
    state: SharedState,
    delay: Duration,
) {
    runtime.spawn(async move {
        tokio::time::sleep(delay).await;
        if let Err(status) = queue_missing_dj_profiles_for_current_pair(state).await {
            tracing::warn!(
                ?status,
                "Scheduled DJ profile retry failed to queue current pair"
            );
        }
    });
}

fn profile_rebuild_failures() -> &'static Mutex<HashMap<String, DjProfileRebuildFailure>> {
    DJ_PROFILE_REBUILD_FAILURES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn record_dj_profile_rebuild_failure(key: &str, status: &str, message: String) -> Option<Duration> {
    let mut guard = profile_rebuild_failures().lock().ok()?;
    guard
        .retain(|_, failure| failure.recorded_at.elapsed() <= profile_rebuild_failure_ttl(failure));
    // This is short-lived suppression, not a permanent catalog blacklist.
    // Keep old failures bounded even when many unavailable assets are visited.
    if guard.len() >= 512
        && !guard.contains_key(key)
        && let Some(oldest_key) = guard
            .iter()
            .min_by_key(|(_, failure)| failure.recorded_at)
            .map(|(key, _)| key.clone())
    {
        guard.remove(&oldest_key);
    }
    // Carry the attempt count across automatic retries (the accept path no
    // longer clears it) so a chronically-failing stream backs off and finally
    // gives up instead of re-decoding every 25s forever.
    let attempts = guard
        .get(key)
        .map(|failure| failure.attempts)
        .unwrap_or(0)
        .saturating_add(1);
    let mut retry_reason = profile_rebuild_retry_reason(status, &message);
    let mut status = status.to_string();
    let mut message = message;
    if retry_reason.is_some() && attempts >= DJ_PROFILE_MAX_TRANSIENT_ATTEMPTS {
        // Give up: treat a chronically-failing rebuild as a hard decode
        // failure. deck_needs_profile_rebuild stops re-queuing decode_failed
        // decks, so the loop ends and the DJ engine can fall back.
        retry_reason = None;
        status = "decode_failed".to_string();
        message = format!(
            "{} Automatic analysis stopped after {attempts} attempts. Rebuild analysis to try again.",
            message.replace(" Retrying analysis.", "")
        );
    }
    let retry_delay = retry_reason
        .as_ref()
        .map(|_| profile_rebuild_backoff(attempts));
    let next_retry_at = retry_delay.map(|delay| Instant::now() + delay);
    guard.insert(
        key.to_string(),
        DjProfileRebuildFailure {
            status,
            message,
            retry_reason,
            next_retry_at,
            recorded_at: Instant::now(),
            attempts,
        },
    );
    retry_delay
}

fn clear_dj_profile_rebuild_failure(key: &str) {
    if let Ok(mut guard) = profile_rebuild_failures().lock() {
        guard.remove(key);
    }
}

pub(crate) fn record_unavailable_tidal_source(tidal_id: i64) {
    record_dj_profile_rebuild_failure(
        &format!("tidal_track:{tidal_id}"),
        "source_unavailable",
        "Track is unavailable on TIDAL. Automatic analysis stopped.".to_string(),
    );
}

pub(crate) fn clear_unavailable_tidal_source(tidal_id: i64) {
    let key = format!("tidal_track:{tidal_id}");
    if let Ok(mut guard) = profile_rebuild_failures().lock()
        && guard
            .get(&key)
            .is_some_and(|failure| failure.status == "source_unavailable")
    {
        // A newly resolved stream establishes that the source is available.
        // Preserve transient decode attempt counts until decoding succeeds.
        guard.remove(&key);
    }
}

fn profile_rebuild_failure_ttl(failure: &DjProfileRebuildFailure) -> Duration {
    Duration::from_secs(if failure.status == "decode_failed" {
        DJ_PROFILE_EXHAUSTED_FAILURE_TTL_SECS
    } else {
        DJ_PROFILE_REBUILD_FAILURE_TTL_SECS
    })
}

fn recent_dj_profile_rebuild_failure(key: &str) -> Option<DjProfileRebuildFailure> {
    let mut guard = profile_rebuild_failures().lock().ok()?;
    match guard.get(key) {
        Some(failure) if failure.recorded_at.elapsed() <= profile_rebuild_failure_ttl(failure) => {
            Some(failure.clone())
        }
        Some(_) => {
            guard.remove(key);
            None
        }
        None => None,
    }
}

fn profile_rebuild_failure_status(error: &anyhow::Error) -> &'static str {
    if error.chain().any(|cause| {
        cause
            .downcast_ref::<tidal_stream::StreamResolveError>()
            .is_some_and(|error| error.is_asset_not_ready() || error.is_track_specific_rejection())
    }) || profile_rebuild_error_is_asset_not_ready_chain(error)
    {
        // Called only after the bounded quality fallback has been exhausted.
        "source_unavailable"
    } else if profile_rebuild_error_is_retryable_chain(error) {
        "retrying"
    } else {
        "decode_failed"
    }
}

fn profile_rebuild_error_is_asset_not_ready_chain(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<tidal_stream::StreamResolveError>()
            .is_some_and(tidal_stream::StreamResolveError::is_asset_not_ready)
            || profile_rebuild_error_is_asset_not_ready(&cause.to_string())
    })
}

fn profile_rebuild_error_is_retryable_chain(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        if let Some(error) = cause.downcast_ref::<tidal_stream::StreamResolveError>() {
            match error {
                tidal_stream::StreamResolveError::RequestFailed { .. } => return true,
                tidal_stream::StreamResolveError::UpstreamHttp { status, .. } => {
                    return status.is_server_error()
                        || status.as_u16() == 408
                        || status.as_u16() == 429;
                }
                _ => {}
            }
        }
        profile_rebuild_error_is_retryable(&cause.to_string())
    })
}

fn profile_rebuild_error_is_retryable(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    message.contains("DASH stream prebuffer failed")
        || message.contains("DASH segment")
        || lower.contains("timed out")
        || lower.contains("request failed")
        || lower.contains("chunk error")
        || lower.contains("returned error status")
}

fn profile_rebuild_error_is_asset_not_ready(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("asset is not ready for playback") || lower.contains("\"substatus\":4005")
}

fn profile_rebuild_retry_reason(status: &str, message: &str) -> Option<String> {
    if status != "retrying" {
        return None;
    }
    let lower = message.to_ascii_lowercase();
    let reason = if lower.contains("asset") || lower.contains("substatus") {
        "asset_not_ready"
    } else if lower.contains("dash") || lower.contains("prebuffer") {
        "dash_prebuffer"
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "timeout"
    } else {
        "transient_decode"
    };
    Some(reason.to_string())
}

fn profile_rebuild_retry_after_ms(failure: &DjProfileRebuildFailure) -> Option<i64> {
    let next_retry_at = failure.next_retry_at?;
    let now = Instant::now();
    if next_retry_at <= now {
        return Some(0);
    }
    Some(
        next_retry_at
            .duration_since(now)
            .as_millis()
            .min(i64::MAX as u128) as i64,
    )
}

fn profile_rebuild_error_message(error: &anyhow::Error, status: &str) -> String {
    if status == "source_unavailable" {
        return "Track is unavailable on TIDAL. Automatic analysis stopped.".to_string();
    }
    if status == "retrying" {
        let message = error.to_string();
        if profile_rebuild_error_is_asset_not_ready(&message) {
            return "TIDAL asset is not ready. Retrying analysis.".to_string();
        }
        return "DASH stream prebuffer failed. Retrying analysis.".to_string();
    }
    let message = error.to_string();
    if message.trim().is_empty() {
        return "Profile decode failed".to_string();
    }
    message.chars().take(160).collect()
}

async fn rebuild_track_for_tidal_ref(
    state: &SharedState,
    tidal_id: i64,
) -> crate::db::models::Track {
    let db = {
        let state_guard = state.read().await;
        state_guard.db.clone()
    };
    db.with_conn(|conn| library_track_for_tidal_id(conn, tidal_id))
        .ok()
        .flatten()
        .unwrap_or_else(|| crate::db::models::Track {
            id: 0,
            title: format!("TIDAL {tidal_id}"),
            artist_id: 0,
            artist_name: None,
            album_id: None,
            album_title: None,
            disc_number: None,
            track_number: None,
            duration_ms: None,
            isrc: None,
            tidal_id: Some(tidal_id),
            artist_tidal_id: None,
            album_tidal_id: None,
            ytmusic_id: None,
            soundcloud_id: None,
            best_quality: None,
            best_source: Some("tidal".to_string()),
            fidelity_score: 0,
            is_favorite: false,
            play_count: 0,
            last_played_at: None,
            date_added: None,
            source: "tidal_stream".to_string(),
            artwork_url: None,
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
                artist_tidal_id: None,
                album_tidal_id: None,
                // Library row: links resolve via artist_id/album_id.
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

fn deck_status(
    conn: &rusqlite::Connection,
    media_ref: &DjMediaRef,
    label_override: Option<&(String, Option<String>)>,
    rebuild_inflight: bool,
) -> anyhow::Result<DjDeckStatus> {
    let key = media_ref.profile_key();
    let profile = queries::get_audio_dj_profile(conn, &key)?;
    let correction = queries::get_audio_dj_profile_correction(conn, &key)?;
    let rebuild_key = dj_profile_inflight_key(&key);
    let updating_analysis = profile.as_ref().is_some_and(|row| {
        // A cached profile still needs its waveform, regardless of provenance.
        // Do not let that cache clear a running rebuild or its retry budget.
        decode_f32_blob(&row.waveform_peaks_blob).is_none_or(|peaks| peaks.is_empty())
            || (row.source.starts_with("dj_playback") && !dj_profile_row_is_current(row))
    });
    let known_failure = recent_dj_profile_rebuild_failure(&rebuild_key);
    let source_unavailable = known_failure
        .as_ref()
        .is_some_and(|failure| failure.status == "source_unavailable");
    let rebuild_failure = if profile.is_some() && !updating_analysis && !source_unavailable {
        clear_dj_profile_rebuild_failure(&rebuild_key);
        None
    } else {
        known_failure
    };
    let profile_status = if source_unavailable {
        "source_unavailable".to_string()
    } else if profile.is_some() && !updating_analysis {
        "ready".to_string()
    } else if let Some(failure) = rebuild_failure.as_ref() {
        failure.status.clone()
    } else if rebuild_inflight {
        "analyzing".to_string()
    } else if profile.is_some() {
        "ready".to_string()
    } else {
        "missing".to_string()
    };
    let profile_retry_after_ms = rebuild_failure
        .as_ref()
        .and_then(profile_rebuild_retry_after_ms);
    let profile_retry_reason = rebuild_failure
        .as_ref()
        .and_then(|failure| failure.retry_reason.clone());
    let profile_error = rebuild_failure.map(|failure| failure.message);
    let (title, artist) = match label_override {
        Some((title, artist)) => (title.clone(), artist.clone()),
        None => media_ref_label(conn, media_ref)?,
    };
    let passive_analysis = media_ref
        .track_id()
        .and_then(crate::services::audio_analysis::queue_prescanner::prescan_status_for_track);
    let (
        beat_count,
        downbeat_count,
        phrase_count,
        profile_confidence,
        waveform_peaks,
        beat_markers_ms,
        downbeat_markers_ms,
        phrase_markers_ms,
        drop_markers_ms,
        mix_in_markers_ms,
        mix_out_markers_ms,
    ) = if let Some(profile) = profile.as_ref() {
        let beat_markers = decode_f32_blob(&profile.beat_grid_blob).unwrap_or_default();
        let downbeat_markers = decode_f32_blob(&profile.downbeats_blob).unwrap_or_default();
        let phrase_markers = phrase_markers_ms(
            &decode_u32_blob(&profile.phrase_boundaries_blob).unwrap_or_default(),
            &downbeat_markers,
        );
        (
            Some(beat_markers.len()),
            Some(downbeat_markers.len()),
            decode_u32_blob(&profile.phrase_boundaries_blob).map(|values| values.len()),
            Some(profile.profile_confidence),
            capped_waveform_peaks(&profile.waveform_peaks_blob),
            seconds_markers_ms(&beat_markers),
            seconds_markers_ms(&downbeat_markers),
            phrase_markers,
            seconds_markers_ms(&decode_f32_blob(&profile.drop_blob).unwrap_or_default()),
            seconds_markers_ms(&decode_f32_blob(&profile.mix_in_blob).unwrap_or_default()),
            seconds_markers_ms(&decode_f32_blob(&profile.mix_out_blob).unwrap_or_default()),
        )
    } else {
        (
            None,
            None,
            None,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
    };
    let manual_drop_markers_ms = correction
        .as_ref()
        .map(|row| decode_marker_blob_ms(&row.manual_drop_blob))
        .unwrap_or_default();
    let waveform_status = waveform_status(&profile_status, &waveform_peaks);
    let energy = media_ref
        .track_id()
        .or_else(|| profile.as_ref().and_then(|row| row.track_id))
        .map(|track_id| queries::get_audio_dsp_features(conn, track_id))
        .transpose()?
        .flatten()
        .and_then(|features| features.energy)
        .or_else(|| {
            profile
                .as_ref()
                .and_then(|row| row.lufs_loud_body)
                .map(crate::services::audio_analysis::features::energy_from_db)
        });
    Ok(DjDeckStatus {
        media_ref_kind: key.media_ref_kind,
        media_ref_id: key.media_ref_id,
        title,
        artist,
        profile_ready: profile.is_some(),
        profile_status,
        profile_error,
        profile_retry_after_ms,
        profile_retry_reason,
        profile_confidence,
        beat_confidence: profile.as_ref().and_then(|row| row.beat_confidence),
        grid_is_synthetic: profile.as_ref().is_some_and(|row| {
            crate::playback::dj_engine::dj_grid_is_synthetic(
                row,
                &decode_f32_blob(&row.beat_grid_blob).unwrap_or_default(),
            )
        }),
        analysis_scope_ms: profile.as_ref().map(|row| row.analysis_scope_ms),
        energy,
        beat_count,
        downbeat_count,
        phrase_count,
        waveform_status,
        waveform_peaks,
        beat_markers_ms,
        downbeat_markers_ms,
        phrase_markers_ms,
        drop_markers_ms,
        manual_drop_markers_ms,
        mix_in_markers_ms,
        mix_out_markers_ms,
        passive_analysis_status: passive_analysis
            .as_ref()
            .map(|snapshot| snapshot.status.to_string()),
        passive_analysis_reason: passive_analysis
            .as_ref()
            .map(|snapshot| snapshot.reason.to_string()),
        safe_crossfade_only: correction
            .as_ref()
            .is_some_and(|row| row.safe_crossfade_only),
    })
}

fn capped_waveform_peaks(blob: &[u8]) -> Vec<f32> {
    decode_f32_blob(blob)
        .unwrap_or_default()
        .into_iter()
        .take(DJ_WAVEFORM_PEAK_COUNT)
        .map(|peak| peak.clamp(0.0, 1.0))
        .collect()
}

fn active_transition_for_runtime(
    conn: &Connection,
    session: Option<&player::ActiveListenSession>,
    current: Option<&DjMediaRef>,
    elapsed_ms: Option<i64>,
) -> anyhow::Result<Option<DjActiveTransition>> {
    // The installed overlap's output clock is the evidence of live mixing.
    // Pause flushes listening history, and resume starts a new listen session;
    // neither action removes the audio already installed in the buffer.
    let Some(elapsed_ms) = elapsed_ms else {
        return Ok(None);
    };
    let session_id = session
        .filter(|session| session.transition_visual_valid)
        .and_then(|session| session.dj_transition_event_id);
    let latest_id = if let Some(current) = current {
        let key = current.profile_key();
        conn.query_row("SELECT id FROM dj_transition_events
            WHERE to_media_ref_kind = ?1 AND to_media_ref_id = ?2
              AND actual_start_ms IS NOT NULL AND runtime_rendered_dj_mixer = 1
              AND runtime_renderer_status = 'rendered_handoff' AND timing_status IN ('fired', 'late')
            ORDER BY id DESC LIMIT 1",
            params![key.media_ref_kind, key.media_ref_id], |row| row.get::<_, i64>(0)).optional()?
    } else {
        None
    };
    // Started can open the listening session before promotion timing is
    // persisted. Its remembered event may then belong to an earlier mix of
    // this track. The installed overlap clock still supplies the live proof;
    // prefer a newer confirmed execution for this same incoming track.
    let event_id = match (session_id, latest_id) {
        (Some(session), Some(latest)) => Some(session.max(latest)),
        (session, latest) => session.or(latest),
    };
    event_id
        .map(|id| active_transition_for_event(conn, id, elapsed_ms))
        .transpose()
        .map(Option::flatten)
}

fn active_transition_for_event(
    conn: &Connection,
    id: i64,
    position_ms: i64,
) -> anyhow::Result<Option<DjActiveTransition>> {
    let event = conn
        .query_row(
            "SELECT from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                program_json, COALESCE(runtime_planned_start_ms, planned_start_ms, actual_start_ms), actual_start_ms
         FROM dj_transition_events WHERE id = ?1 AND runtime_rendered_dj_mixer = 1
           AND actual_start_ms IS NOT NULL
           AND runtime_renderer_status = 'rendered_handoff' AND timing_status IN ('fired', 'late')",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            },
        )
        .optional()?;
    let Some((from_kind, from_id, to_kind, to_id, json, start_ms, actual_start_ms)) = event else {
        return Ok(None);
    };
    let Ok(program) = serde_json::from_str::<noor_mix::TransitionProgram>(&json) else {
        return Ok(None);
    };
    let duration_ms =
        program.resolve_at.saturating_mul(1000) / u64::from(program.sample_rate.max(1));
    if position_ms < 0 || position_ms as u64 >= duration_ms {
        return Ok(None);
    }
    let media_ref = |kind: &str, id: &str| {
        let id = id.parse::<i64>().ok()?;
        match kind {
            "library_track" => Some(DjMediaRef::LibraryTrack { track_id: id }),
            "tidal_track" => Some(DjMediaRef::TidalTrack {
                tidal_id: id,
                track_id: None,
            }),
            _ => None,
        }
    };
    let (Some(from), Some(to)) = (media_ref(&from_kind, &from_id), media_ref(&to_kind, &to_id))
    else {
        return Ok(None);
    };
    Ok(Some(DjActiveTransition {
        event_id: id,
        outgoing: deck_status(conn, &from, None, false)?,
        incoming: deck_status(conn, &to, None, false)?,
        program,
        start_ms,
        actual_start_ms,
        elapsed_ms: position_ms,
    }))
}

fn waveform_status(profile_status: &str, peaks: &[f32]) -> String {
    if !peaks.is_empty() {
        "ready".to_string()
    } else if profile_status == "analyzing" || profile_status == "retrying" {
        "analyzing".to_string()
    } else {
        "missing".to_string()
    }
}

fn seconds_markers_ms(values: &[f32]) -> Vec<i64> {
    values
        .iter()
        .filter(|value| value.is_finite() && **value >= 0.0)
        .map(|value| (*value as f64 * 1000.0).round() as i64)
        .collect()
}

fn phrase_markers_ms(phrases: &[u32], downbeats: &[f32]) -> Vec<i64> {
    phrases
        .iter()
        .filter_map(|index| downbeats.get(*index as usize))
        .filter(|value| value.is_finite() && **value >= 0.0)
        .map(|value| (*value as f64 * 1000.0).round() as i64)
        .collect()
}

fn drop_preview_status(
    conn: &rusqlite::Connection,
    enabled: bool,
    current_ref: Option<&DjMediaRef>,
    next_ref: Option<&DjMediaRef>,
    current: Option<&DjDeckStatus>,
    next: Option<&DjDeckStatus>,
    current_duration_ms: Option<i64>,
    actual_fire_ms: Option<i64>,
) -> anyhow::Result<DjDropPreviewStatus> {
    let skipped = |reason: &str| DjDropPreviewStatus {
        status: "skipped".to_string(),
        planned_fire_ms: None,
        actual_fire_ms: None,
        incoming_drop_ms: incoming_drop_marker(next).map(|marker| marker.0),
        source: incoming_drop_marker(next).map(|marker| marker.1.to_string()),
        reason: Some(reason.to_string()),
    };

    if !enabled {
        return Ok(skipped("disabled"));
    }
    let (Some(current_ref), Some(next_ref), Some(current), Some(next)) =
        (current_ref, next_ref, current, next)
    else {
        return Ok(skipped("pair_missing"));
    };
    if current.profile_status == "source_unavailable" {
        return Ok(skipped("current_source_unavailable"));
    }
    if next.profile_status == "source_unavailable" {
        return Ok(skipped("next_source_unavailable"));
    }
    if !current.profile_ready {
        return Ok(skipped(&deck_profile_unavailable_reason(
            "current", current,
        )));
    }
    if !next.profile_ready {
        return Ok(skipped(&deck_profile_unavailable_reason("next", next)));
    }
    if current.safe_crossfade_only || next.safe_crossfade_only {
        return Ok(skipped("safe_crossfade_only"));
    }
    if current
        .profile_confidence
        .is_some_and(|value| value < DJ_PROFILE_CONFIDENCE_FLOOR)
        || next
            .profile_confidence
            .is_some_and(|value| value < DJ_PROFILE_CONFIDENCE_FLOOR)
    {
        return Ok(skipped("profile_low_confidence"));
    }
    if !drop_preview_pair_harmonic_compatible(conn, current_ref, next_ref)? {
        return Ok(skipped("harmonic_incompatible"));
    }
    let Some((incoming_drop_ms, source)) = incoming_drop_marker(Some(next)) else {
        return Ok(skipped("missing_incoming_drop"));
    };
    let Some(planned_fire_ms) = select_drop_preview_fire_ms(current, current_duration_ms) else {
        return Ok(DjDropPreviewStatus {
            status: "skipped".to_string(),
            planned_fire_ms: None,
            actual_fire_ms: None,
            incoming_drop_ms: Some(incoming_drop_ms),
            source: Some(source.to_string()),
            reason: Some("no_safe_mid_song_marker".to_string()),
        });
    };
    Ok(DjDropPreviewStatus {
        status: if actual_fire_ms.is_some() {
            "fired".to_string()
        } else {
            "armed".to_string()
        },
        planned_fire_ms: Some(planned_fire_ms),
        actual_fire_ms,
        incoming_drop_ms: Some(incoming_drop_ms),
        source: Some(source.to_string()),
        reason: None,
    })
}

fn deck_profile_unavailable_reason(prefix: &str, deck: &DjDeckStatus) -> String {
    if deck.profile_status == "retrying" {
        if let Some(reason) = deck.profile_retry_reason.as_deref() {
            return format!("{prefix}_profile_retrying_{reason}");
        }
        return format!("{prefix}_profile_retrying");
    }
    format!("{prefix}_profile_missing")
}

pub(crate) fn drop_preview_plan_for_pair(
    conn: &rusqlite::Connection,
    current_ref: &DjMediaRef,
    next_ref: &DjMediaRef,
    current_duration_ms: Option<i64>,
) -> anyhow::Result<Option<DropPreviewPlan>> {
    let enabled = queries::is_dj_engine_enabled(conn)?;
    let current = deck_status(conn, current_ref, None, false)?;
    let next = deck_status(conn, next_ref, None, false)?;
    let status = drop_preview_status(
        conn,
        enabled,
        Some(current_ref),
        Some(next_ref),
        Some(&current),
        Some(&next),
        current_duration_ms,
        None,
    )?;
    Ok(
        match (
            status.status.as_str(),
            status.planned_fire_ms,
            status.incoming_drop_ms,
            status.source,
        ) {
            ("armed", Some(planned_fire_ms), Some(incoming_drop_ms), Some(source)) => {
                Some(DropPreviewPlan {
                    planned_fire_ms,
                    incoming_drop_ms,
                    source,
                })
            }
            _ => None,
        },
    )
}

fn incoming_drop_marker(next: Option<&DjDeckStatus>) -> Option<(i64, &'static str)> {
    let next = next?;
    next.manual_drop_markers_ms
        .iter()
        .copied()
        .find(|marker| *marker >= 0)
        .map(|marker| (marker, "manual"))
        .or_else(|| {
            next.drop_markers_ms
                .iter()
                .copied()
                .find(|marker| *marker >= 0)
                .map(|marker| (marker, "profile"))
        })
}

fn select_drop_preview_fire_ms(current: &DjDeckStatus, duration_ms: Option<i64>) -> Option<i64> {
    let duration_ms = duration_ms.filter(|duration| *duration > 0)?;
    let min_ms = DROP_PREVIEW_MIN_POSITION_MS.max(duration_ms * 45 / 100);
    let max_ms = (duration_ms * 65 / 100)
        .min(duration_ms - DJ_READY_PAIR_TRANSITION_WINDOW_MS - DROP_PREVIEW_FINAL_WINDOW_GUARD_MS);
    if max_ms < min_ms {
        return None;
    }
    let target_ms = duration_ms * 55 / 100;
    current
        .phrase_markers_ms
        .iter()
        .chain(current.downbeat_markers_ms.iter())
        .copied()
        .filter(|marker| (min_ms..=max_ms).contains(marker))
        .min_by_key(|marker| (*marker - target_ms).abs())
}

fn drop_preview_pair_harmonic_compatible(
    conn: &rusqlite::Connection,
    current_ref: &DjMediaRef,
    next_ref: &DjMediaRef,
) -> anyhow::Result<bool> {
    let current_key = media_ref_camelot_key(conn, current_ref)?;
    let next_key = media_ref_camelot_key(conn, next_ref)?;
    Ok(match (current_key.as_deref(), next_key.as_deref()) {
        (Some(current), Some(next)) => noor_mix::planner::scoring::camelot_distance(current, next)
            .is_some_and(|distance| matches!(distance, 0 | 1 | 7)),
        _ => false,
    })
}

fn media_ref_camelot_key(
    conn: &rusqlite::Connection,
    media_ref: &DjMediaRef,
) -> anyhow::Result<Option<String>> {
    let key = media_ref.profile_key();
    let profile = queries::get_audio_dj_profile(conn, &key)?;
    let track_id = media_ref
        .track_id()
        .or_else(|| profile.as_ref().and_then(|profile| profile.track_id))
        .or_else(|| {
            media_ref
                .tidal_id()
                .and_then(|tidal_id| track_id_for_tidal_id(conn, tidal_id).ok().flatten())
        });
    let Some(track_id) = track_id else {
        return Ok(None);
    };
    Ok(queries::get_audio_dsp_features(conn, track_id)?.and_then(|features| features.camelot_key))
}

fn track_id_for_tidal_id(
    conn: &rusqlite::Connection,
    tidal_id: i64,
) -> anyhow::Result<Option<i64>> {
    conn.query_row(
        "SELECT id FROM tracks WHERE tidal_id = ?1 LIMIT 1",
        [tidal_id],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map_err(anyhow::Error::from)
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
                timing_source, timing_status, rejected_alternatives_json,
                runtime_rendered_dj_mixer, runtime_renderer_status, runtime_renderer_reason
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
            let planned_start_ms: Option<i64> = row.get(4)?;
            let timing_status: Option<String> = row.get(8)?;
            Ok(OpenTransition {
                id: row.get(0)?,
                template: row.get(1)?,
                renderer_template: renderer_template_from_program_json(&program_json),
                fallback_reason: row.get(3)?,
                planned_start_ms,
                actual_start_ms: row.get(5)?,
                timing_delta_ms: row.get(6)?,
                timing_source: row.get(7)?,
                timing_status: timing_status.clone(),
                overlay_details: overlay_details_from_program_json(
                    &program_json,
                    planned_start_ms,
                    timing_status.as_deref(),
                ),
                runtime_rendered_dj_mixer: row.get::<_, Option<i64>>(10)?.map(|value| value != 0),
                runtime_renderer_status: row.get(11)?,
                runtime_renderer_reason: row.get(12)?,
                rejected_alternatives: decode_rejected_alternatives(row.get(9)?),
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
                COALESCE(from_track.title, from_tidal_track.title, from_queue_track.title, from_queue.pending_title, e.from_media_ref_kind || ':' || e.from_media_ref_id),
                COALESCE(from_artist.name, from_tidal_artist.name, from_queue_artist.name, from_queue.pending_artist),
                COALESCE(to_track.title, to_tidal_track.title, to_queue_track.title, to_queue.pending_title, e.to_media_ref_kind || ':' || e.to_media_ref_id),
                COALESCE(to_artist.name, to_tidal_artist.name, to_queue_artist.name, to_queue.pending_artist),
                e.template, e.program_json, e.fallback_reason,
                planned_start_ms, actual_start_ms, timing_delta_ms,
                timing_source, timing_status, e.started_at, e.rejected_alternatives_json,
                e.runtime_rendered_dj_mixer, e.runtime_renderer_status, e.runtime_renderer_reason, e.runtime_planned_start_ms
         FROM dj_transition_events e
         LEFT JOIN tracks from_track ON from_track.id = e.from_track_id
         LEFT JOIN artists from_artist ON from_artist.id = from_track.artist_id
         LEFT JOIN tracks to_track ON to_track.id = e.to_track_id
         LEFT JOIN artists to_artist ON to_artist.id = to_track.artist_id
         LEFT JOIN tracks from_tidal_track
           ON e.from_media_ref_kind = 'tidal_track'
          AND from_tidal_track.tidal_id = CAST(e.from_media_ref_id AS INTEGER)
         LEFT JOIN artists from_tidal_artist ON from_tidal_artist.id = from_tidal_track.artist_id
         LEFT JOIN tracks to_tidal_track
           ON e.to_media_ref_kind = 'tidal_track'
          AND to_tidal_track.tidal_id = CAST(e.to_media_ref_id AS INTEGER)
         LEFT JOIN artists to_tidal_artist ON to_tidal_artist.id = to_tidal_track.artist_id
         LEFT JOIN queue from_queue
           ON e.from_media_ref_kind = 'queue_item'
          AND from_queue.id = CAST(e.from_media_ref_id AS INTEGER)
         LEFT JOIN tracks from_queue_track ON from_queue_track.id = from_queue.track_id
         LEFT JOIN artists from_queue_artist ON from_queue_artist.id = from_queue_track.artist_id
         LEFT JOIN queue to_queue
           ON e.to_media_ref_kind = 'queue_item'
          AND to_queue.id = CAST(e.to_media_ref_id AS INTEGER)
         LEFT JOIN tracks to_queue_track ON to_queue_track.id = to_queue.track_id
         LEFT JOIN artists to_queue_artist ON to_queue_artist.id = to_queue_track.artist_id
         WHERE e.timing_status IN ('fired', 'late', 'missed')
           AND COALESCE(e.runtime_renderer_reason, '') <> 'manual_seek_suppressed'
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
        let rejected_json: Option<String> = row.get(14)?;
        let runtime_rendered_dj_mixer = row.get::<_, Option<i64>>(15)?.map(|value| value != 0);
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
            runtime_planned_start_ms: row.get(18)?,
            actual_start_ms: row.get(9)?,
            timing_delta_ms,
            timing_source: row.get(11)?,
            timing_status: timing_status.clone(),
            timing_quality: timing_quality(timing_status.as_deref(), timing_delta_ms).to_string(),
            timing_direction: timing_direction(timing_status.as_deref(), timing_delta_ms)
                .to_string(),
            runtime_rendered_dj_mixer,
            runtime_renderer_status: row.get(16)?,
            runtime_renderer_reason: row.get(17)?,
            started_at: row.get(13)?,
            rejected_alternatives: decode_rejected_alternatives(rejected_json),
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
    if timing_status == Some("armed") {
        return "pending";
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
            Some(delta_ms) if delta_ms < -150 => "early",
            Some(delta_ms) if delta_ms > 150 => "late",
            Some(_) => "on_time",
            None => "unknown",
        },
        _ => "unknown",
    }
}

fn decode_rejected_alternatives(json: Option<String>) -> Vec<DjRejectedAlternative> {
    let Some(json) = json else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<DjRejectedAlternative>>(&json).unwrap_or_default()
}

fn latest_fired_dj_timing_deltas(
    conn: &rusqlite::Connection,
    limit: i64,
) -> anyhow::Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT timing_delta_ms
         FROM dj_transition_events
         WHERE timing_status = 'fired'
           AND timing_delta_ms IS NOT NULL
           AND ABS(timing_delta_ms) <= ?2
           AND template != 'DropPreview16'
         ORDER BY started_at DESC, id DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map(
        params![limit.max(0), DJ_TIMING_SANITY_MAX_DELTA_MS],
        |row| row.get::<_, i64>(0),
    )?;
    let mut deltas = Vec::new();
    for row in rows {
        deltas.push(row?);
    }
    Ok(deltas)
}

fn summarize_timing_history(
    events: &[DjTimingHistoryEvent],
    tuning_deltas: &[i64],
) -> DjTimingHistorySummary {
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
        if let Some(delta_ms) = event
            .timing_delta_ms
            .filter(|delta| timing_delta_is_sane(*delta))
        {
            delta_sum += delta_ms;
            abs_delta_sum += delta_ms.abs();
            delta_count += 1;
        }
    }

    DjTimingHistorySummary {
        event_count: events.len(),
        average_delta_ms: if delta_count > 0 {
            Some(delta_sum / delta_count)
        } else {
            None
        },
        average_abs_delta_ms: if delta_count > 0 {
            Some(abs_delta_sum / delta_count)
        } else {
            None
        },
        median_abs_delta_ms: median_abs_delta(tuning_deltas),
        worst_abs_delta_ms: worst_abs_delta(tuning_deltas),
        tight_count,
        usable_count,
        loose_count,
        bad_count,
        late_count,
        missed_count,
    }
}

fn timing_delta_is_sane(delta_ms: i64) -> bool {
    delta_ms.abs() <= DJ_TIMING_SANITY_MAX_DELTA_MS
}

fn median_abs_delta(deltas: &[i64]) -> Option<i64> {
    if deltas.is_empty() {
        return None;
    }
    let mut abs_values = deltas.iter().map(|delta| delta.abs()).collect::<Vec<_>>();
    abs_values.sort_unstable();
    let middle = abs_values.len() / 2;
    if abs_values.len() % 2 == 0 {
        Some((abs_values[middle - 1] + abs_values[middle]) / 2)
    } else {
        Some(abs_values[middle])
    }
}

fn worst_abs_delta(deltas: &[i64]) -> Option<i64> {
    deltas.iter().map(|delta| delta.abs()).max()
}

#[allow(dead_code)]
fn fire_ahead_evidence_passes(deltas: &[i64]) -> bool {
    if deltas.len() < 20 {
        return false;
    }
    let positive_count = deltas.iter().filter(|delta| **delta > 0).count();
    positive_count * 10 >= deltas.len() * 7 && median_delta(deltas).is_some_and(|delta| delta > 150)
}

fn median_delta(deltas: &[i64]) -> Option<i64> {
    if deltas.is_empty() {
        return None;
    }
    let mut values = deltas.to_vec();
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        Some((values[middle - 1] + values[middle]) / 2)
    } else {
        Some(values[middle])
    }
}

#[cfg(test)]
fn open_transition_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OpenTransition> {
    let program_json: String = row.get(2)?;
    let planned_start_ms: Option<i64> = row.get(4)?;
    let timing_status: Option<String> = row.get(8)?;
    Ok(OpenTransition {
        id: row.get(0)?,
        template: row.get(1)?,
        renderer_template: renderer_template_from_program_json(&program_json),
        fallback_reason: row.get(3)?,
        planned_start_ms,
        actual_start_ms: row.get(5)?,
        timing_delta_ms: row.get(6)?,
        timing_source: row.get(7)?,
        timing_status: timing_status.clone(),
        overlay_details: overlay_details_from_program_json(
            &program_json,
            planned_start_ms,
            timing_status.as_deref(),
        ),
        runtime_rendered_dj_mixer: row.get::<_, Option<i64>>(9)?.map(|value| value != 0),
        runtime_renderer_status: row.get(10)?,
        runtime_renderer_reason: row.get(11)?,
        rejected_alternatives: Vec::new(),
    })
}

#[cfg(test)]
fn latest_completed_timing_transition(
    conn: &rusqlite::Connection,
) -> anyhow::Result<Option<OpenTransition>> {
    conn.query_row(
        "SELECT id, template, program_json, fallback_reason,
                planned_start_ms, actual_start_ms, timing_delta_ms,
                timing_source, timing_status, runtime_rendered_dj_mixer,
                runtime_renderer_status, runtime_renderer_reason
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
            runtime_rendered_dj_mixer: None,
            runtime_renderer_status: None,
            runtime_renderer_reason: None,
            overlay_details: None,
            rejected_alternatives: Vec::new(),
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
    if transition
        .renderer_template
        .as_deref()
        .is_some_and(is_renderable_template)
    {
        let downgrade_reason = renderer_downgrade_reason(transition);
        return RendererStatus {
            planned_template: Some(transition.template.clone()),
            renderer_template: transition.renderer_template.clone(),
            renderer_mode: Some(
                if transition.renderer_template.as_deref() == Some("DropTease16") {
                    "dj_overlay_program"
                } else {
                    "dj_gain_program"
                }
                .to_string(),
            ),
            downgrade_reason,
            planning_reason: planning_reason_without_renderer_downgrade(transition),
            sync_target: transition.timing_source.clone(),
            planned_start_ms: transition.planned_start_ms,
            actual_start_ms: transition.actual_start_ms,
            timing_delta_ms: transition.timing_delta_ms,
            timing_source: transition.timing_source.clone(),
            timing_status: transition.timing_status.clone(),
            timing_quality: quality,
            timing_direction: direction,
            runtime_rendered_dj_mixer: transition.runtime_rendered_dj_mixer,
            runtime_renderer_status: transition.runtime_renderer_status.clone(),
            runtime_renderer_reason: transition.runtime_renderer_reason.clone(),
            overlay_details: if transition.renderer_template.as_deref() == Some("DropTease16") {
                transition.overlay_details.clone()
            } else {
                None
            },
            rejected_alternatives: transition.rejected_alternatives.clone(),
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
                transition
                    .fallback_reason
                    .as_deref()
                    .filter(|reason| is_renderer_downgrade_reason(reason))
                    .unwrap_or("template_not_renderable")
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
        runtime_rendered_dj_mixer: transition.runtime_rendered_dj_mixer,
        runtime_renderer_status: transition.runtime_renderer_status.clone(),
        runtime_renderer_reason: transition.runtime_renderer_reason.clone(),
        overlay_details: None,
        rejected_alternatives: transition.rejected_alternatives.clone(),
    }
}

fn renderer_downgrade_reason(transition: &OpenTransition) -> Option<String> {
    if transition.renderer_template.as_deref() == Some(transition.template.as_str()) {
        return None;
    }
    Some(
        transition
            .fallback_reason
            .as_deref()
            .filter(|reason| is_renderer_downgrade_reason(reason))
            .unwrap_or("template_not_renderable")
            .to_string(),
    )
}

fn planning_reason_without_renderer_downgrade(transition: &OpenTransition) -> Option<String> {
    transition
        .fallback_reason
        .as_deref()
        .filter(|reason| !is_renderer_downgrade_reason(reason))
        .map(str::to_string)
}

fn is_renderer_downgrade_reason(reason: &str) -> bool {
    matches!(
        reason,
        "template_not_renderable"
            | "timing_unstable"
            | "overlay_not_handoff"
            | "beat_sync_unverified"
    )
}

fn is_renderable_template(template: &str) -> bool {
    matches!(
        template,
        "SafeCrossfade"
            | "FilterSweep"
            | "BassSwap16"
            | "BassSwap32"
            | "SlamCut"
            | "LongHarmonicBlend"
            | "DropTease16"
            | "ClubMix"
            | "QuickMix"
            | "EnergyLift"
            | "EnergyReset"
            | "DropSwap"
    )
}

fn renderer_template_from_program_json(program_json: &str) -> Option<String> {
    let program: noor_mix::TransitionProgram = serde_json::from_str(program_json).ok()?;
    is_renderable_template(program.template.as_str()).then_some(program.template)
}

fn overlay_details_from_program_json(
    program_json: &str,
    planned_start_ms: Option<i64>,
    timing_status: Option<&str>,
) -> Option<DjOverlayDetails> {
    let program: noor_mix::TransitionProgram = serde_json::from_str(program_json).ok()?;
    if program.template != "DropTease16" || program.sample_rate == 0 {
        return None;
    }
    let resolve_ms = ((u128::from(program.resolve_at) * 1_000)
        + (u128::from(program.sample_rate) / 2))
        / u128::from(program.sample_rate);
    let overlay_end_ms = planned_start_ms
        .zip(i64::try_from(resolve_ms).ok())
        .and_then(|(start_ms, duration_ms)| start_ms.checked_add(duration_ms));
    let drop_marker_ms = ((u128::from(
        program
            .deck_b_start_frame
            .saturating_add(program.swap_start),
    ) * 1_000)
        + (u128::from(program.sample_rate) / 2))
        / u128::from(program.sample_rate);
    let tempo_ratio = program
        .automation
        .iter()
        .find(|event| event.param == noor_mix::Param::PlaybackRate(noor_mix::DeckId::B))
        .and_then(|event| event.to.is_finite().then_some(f64::from(event.to)));
    Some(DjOverlayDetails {
        overlay_status: timing_status.unwrap_or("armed").to_string(),
        overlay_start_ms: planned_start_ms,
        overlay_end_ms,
        tempo_ratio,
        deck_b_start_frame: program.deck_b_start_frame,
        drop_marker_ms: i64::try_from(drop_marker_ms).ok(),
        drop_source: program
            .drop_source
            .unwrap_or_else(|| "program_json".to_string()),
    })
}

fn annotate_overlay_drop_source(
    details: Option<DjOverlayDetails>,
    incoming: Option<&DjDeckStatus>,
) -> Option<DjOverlayDetails> {
    let mut details = details?;
    if details.drop_source != "program_json" {
        return Some(details);
    }
    let Some(drop_marker_ms) = details.drop_marker_ms else {
        return Some(details);
    };
    let Some(incoming) = incoming else {
        return Some(details);
    };
    if marker_matches(&incoming.manual_drop_markers_ms, drop_marker_ms) {
        details.drop_source = "manual_drop_cue".to_string();
    } else if marker_matches(&incoming.drop_markers_ms, drop_marker_ms) {
        details.drop_source = "profile_drop_candidate".to_string();
    }
    Some(details)
}

fn marker_matches(markers_ms: &[i64], target_ms: i64) -> bool {
    markers_ms
        .iter()
        .any(|marker| marker.abs_diff(target_ms) <= 25)
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
    // Executed events retain provider identity after queue promotion. They
    // need not retain the local track id to display the original pair.
    if let DjMediaRef::TidalTrack { tidal_id, .. } = media_ref
        && let Some(row) = conn
            .query_row(
                "SELECT t.title, ar.name FROM tracks t
                 LEFT JOIN artists ar ON ar.id = t.artist_id
                 WHERE t.tidal_id = ?1 ORDER BY t.id LIMIT 1",
                params![tidal_id],
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
        "bad" => Some(-1),
        "too_safe" | "too_bold" => Some(0),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
