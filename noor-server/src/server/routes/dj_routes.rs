use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use chrono::Utc;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64};
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
};
use crate::services::tidal::stream as tidal_stream;

const DEFAULT_DJ_LOOKAHEAD_DEADLINE_SAMPLES: u64 = 48_000 * 30;
const DJ_PROFILE_CONFIDENCE_FLOOR: f64 = 0.65;
const SAFE_SUGGESTION_BAD_COUNT: i64 = 3;
const DJ_PROFILE_AUTO_REBUILD_RETRY_SECS: u64 = 300;
const DJ_TIMING_HISTORY_LIMIT: i64 = 5;
const DJ_READY_PAIR_TRANSITION_WINDOW_MS: i64 = 30_000;
const DJ_TIMING_SANITY_MAX_DELTA_MS: i64 = 30_000;
const DROP_PREVIEW_MIN_POSITION_MS: i64 = 60_000;
const DROP_PREVIEW_FINAL_WINDOW_GUARD_MS: i64 = 45_000;
#[cfg(test)]
const DJ_READY_PAIR_PLANNING_RETRY_SECS: u64 = 15;
const DJ_PROFILE_REBUILD_FAILURE_TTL_SECS: u64 = 300;
const DJ_PROFILE_ANALYSIS_TIDAL_QUALITY: &str = "LOW";

#[cfg(test)]
type ReadyPairPlanningKey = (i64, u64);

#[cfg(test)]
#[allow(dead_code)]
static READY_PAIR_PLANNING_ATTEMPTS: OnceLock<Mutex<HashMap<ReadyPairPlanningKey, Instant>>> =
    OnceLock::new();
static DJ_PROFILE_REBUILD_FAILURES: OnceLock<Mutex<HashMap<String, DjProfileRebuildFailure>>> =
    OnceLock::new();

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
    planning_status: String,
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
    beat_count: Option<usize>,
    downbeat_count: Option<usize>,
    phrase_count: Option<usize>,
    waveform_status: String,
    waveform_peaks: Vec<f32>,
    beat_markers_ms: Vec<i64>,
    downbeat_markers_ms: Vec<i64>,
    phrase_markers_ms: Vec<i64>,
    mix_in_markers_ms: Vec<i64>,
    mix_out_markers_ms: Vec<i64>,
    drop_markers_ms: Vec<i64>,
    manual_drop_markers_ms: Vec<i64>,
    passive_analysis_status: Option<String>,
    passive_analysis_reason: Option<String>,
    safe_crossfade_only: bool,
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
        let ephemeral_pair = super::active_ephemeral_tidal_mix_dj_pair(&state);
        let ephemeral_labels = super::active_ephemeral_tidal_mix_dj_labels(&state);
        let active_track_id = state
            .playback_runtime_info
            .as_ref()
            .and_then(|info| info.active_track_id);
        let active_generation = super::current_playback_generation(&state);
        let drop_preview_actual_fire_ms = state.last_drop_preview.and_then(|preview| {
            (Some(preview.track_id) == active_track_id && preview.generation == active_generation)
                .then_some(preview.actual_fire_ms)
        });
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
                let renderer_status = renderer_status_for_transition(latest_transition.as_ref());
                let drop_preview = drop_preview_status(
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
                Ok(DjStatusResponse {
                    enabled,
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
) -> Result<(), StatusCode> {
    let missing_profile_refs = {
        let state_guard = state.read().await;
        let ephemeral_pair = super::active_ephemeral_tidal_mix_dj_pair(&state_guard);
        let ephemeral_labels = super::active_ephemeral_tidal_mix_dj_labels(&state_guard);
        state_guard
            .db
            .with_conn(|conn| {
                if !queries::is_dj_engine_enabled(conn)? {
                    return Ok(Vec::new());
                }
                if foreground_playback_is_buffering(conn, &state_guard)? {
                    tracing::debug!("Deferring DJ profile rebuilds while playback is buffering");
                    return Ok(Vec::new());
                }
                let pair = match ephemeral_pair {
                    Some(pair) => pair,
                    None => crate::playback::dj_lookahead::load_dj_lookahead_pair(conn)?,
                };
                let mut missing = Vec::new();
                for media_ref in [pair.current, pair.next].into_iter().flatten() {
                    let key = media_ref.profile_key();
                    let label = ephemeral_labels
                        .iter()
                        .find(|(candidate, _)| candidate == &key)
                        .map(|(_, label)| label);
                    let inflight_key = dj_profile_inflight_key(&key);
                    let rebuild_inflight = dj_profile_rebuild_is_inflight(
                        &state_guard.dj_profile_rebuild_inflight,
                        &inflight_key,
                    );
                    let deck = deck_status(conn, &media_ref, label, rebuild_inflight)?;
                    if deck_needs_profile_rebuild(&deck) {
                        missing.push(media_ref);
                    }
                }
                Ok(missing)
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    for media_ref in missing_profile_refs {
        queue_tidal_profile_rebuild_if_idle(state.clone(), media_ref).await?;
    }
    Ok(())
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
    let retry_after = if force {
        std::time::Duration::ZERO
    } else {
        std::time::Duration::from_secs(DJ_PROFILE_AUTO_REBUILD_RETRY_SECS)
    };
    match mark_dj_profile_rebuild_inflight(&inflight, &inflight_key, retry_after)? {
        ProfileRebuildInflightDecision::Start => {
            clear_dj_profile_rebuild_failure(&inflight_key);
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

    let request = dj_profile_analysis_stream_request(tidal_id);
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

    let inflight_for_decode = inflight.clone();
    let inflight_key_for_decode = inflight_key.clone();
    let failure_key = inflight_key.clone();
    let retry_state = state.clone();
    let event_tx = {
        let state_guard = state.read().await;
        state_guard.event_tx.clone()
    };
    let runtime_handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        if let Err(error) = decode_and_buffer_job(config, job, shared, 48_000, 2) {
            let status = profile_rebuild_failure_status(&error);
            let message = profile_rebuild_error_message(&error, status);
            finish_dj_profile_rebuild_failure(
                &inflight_for_decode,
                &inflight_key_for_decode,
                status,
                message.clone(),
            );
            let _ = event_tx.send(crate::AppEvent::PlaybackStateChanged);
            if status == "retrying" {
                schedule_dj_profile_retry(
                    &runtime_handle,
                    retry_state,
                    Duration::from_secs(DJ_PROFILE_AUTO_REBUILD_RETRY_SECS),
                );
            }
            tracing::warn!(tidal_id, error = %message, "DJ profile rebuild decode failed");
        } else {
            clear_dj_profile_rebuild_failure(&failure_key);
            tracing::info!(tidal_id, "DJ profile rebuild decode queued analysis");
        }
    });

    Ok(RebuildDjProfileResponse {
        accepted: true,
        status: "accepted".to_string(),
    })
}

fn dj_profile_analysis_stream_request(tidal_id: i64) -> tidal_stream::StreamRequest {
    tidal_stream::StreamRequest::new(tidal_id, DJ_PROFILE_ANALYSIS_TIDAL_QUALITY)
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

fn foreground_playback_is_buffering(
    conn: &rusqlite::Connection,
    state: &crate::AppState,
) -> anyhow::Result<bool> {
    if state
        .audio_active
        .load(std::sync::atomic::Ordering::Relaxed)
        || state.playback_runtime.is_none()
    {
        return Ok(false);
    }
    Ok(player::load_state(conn)?.is_playing)
}

fn deck_needs_profile_rebuild(deck: &DjDeckStatus) -> bool {
    (!deck.profile_ready
        && (deck.profile_status == "missing"
            || (deck.profile_status == "retrying"
                && deck.profile_retry_after_ms.unwrap_or(0) <= 0)))
        || (deck.profile_ready && deck.waveform_status == "missing")
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

fn finish_dj_profile_rebuild_failure(
    inflight: &Arc<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>>,
    key: &str,
    status: &str,
    message: String,
) {
    record_dj_profile_rebuild_failure(key, status, message);
    clear_dj_profile_inflight(inflight, key);
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

fn record_dj_profile_rebuild_failure(key: &str, status: &str, message: String) {
    if let Ok(mut guard) = profile_rebuild_failures().lock() {
        let retry_reason = profile_rebuild_retry_reason(status, &message);
        let next_retry_at = retry_reason
            .as_ref()
            .map(|_| Instant::now() + Duration::from_secs(DJ_PROFILE_AUTO_REBUILD_RETRY_SECS));
        guard.insert(
            key.to_string(),
            DjProfileRebuildFailure {
                status: status.to_string(),
                message,
                retry_reason,
                next_retry_at,
                recorded_at: Instant::now(),
            },
        );
    }
}

fn clear_dj_profile_rebuild_failure(key: &str) {
    if let Ok(mut guard) = profile_rebuild_failures().lock() {
        guard.remove(key);
    }
}

fn recent_dj_profile_rebuild_failure(key: &str) -> Option<DjProfileRebuildFailure> {
    let mut guard = profile_rebuild_failures().lock().ok()?;
    match guard.get(key) {
        Some(failure)
            if failure.recorded_at.elapsed()
                <= Duration::from_secs(DJ_PROFILE_REBUILD_FAILURE_TTL_SECS) =>
        {
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
    let message = error.to_string();
    if profile_rebuild_error_is_retryable(message.as_str()) {
        "retrying"
    } else {
        "decode_failed"
    }
}

fn profile_rebuild_error_is_retryable(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    message.contains("DASH stream prebuffer failed")
        || message.contains("DASH segment")
        || lower.contains("asset is not ready for playback")
        || lower.contains("\"substatus\":4005")
        || lower.contains("timed out")
        || lower.contains("request failed")
        || lower.contains("chunk error")
        || lower.contains("returned error status")
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
    if status == "retrying" {
        let message = error.to_string().to_ascii_lowercase();
        if message.contains("asset is not ready for playback")
            || message.contains("\"substatus\":4005")
        {
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
        notes: row.notes,
        applies: "next_transition".to_string(),
    }
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
    let rebuild_failure = if profile.is_some() {
        clear_dj_profile_rebuild_failure(&rebuild_key);
        None
    } else {
        recent_dj_profile_rebuild_failure(&rebuild_key)
    };
    let profile_status = if profile.is_some() {
        "ready".to_string()
    } else if let Some(failure) = rebuild_failure.as_ref() {
        failure.status.clone()
    } else if rebuild_inflight {
        "analyzing".to_string()
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
        mix_in_markers_ms,
        mix_out_markers_ms,
        drop_markers_ms,
        manual_drop_markers_ms,
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
            seconds_markers_ms(&decode_f32_blob(&profile.mix_in_blob).unwrap_or_default()),
            seconds_markers_ms(&decode_f32_blob(&profile.mix_out_blob).unwrap_or_default()),
            seconds_markers_ms(&decode_f32_blob(&profile.drop_blob).unwrap_or_default()),
            Vec::new(),
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
            Vec::new(),
        )
    };
    let waveform_status = waveform_status(&profile_status, &waveform_peaks);
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
        beat_count,
        downbeat_count,
        phrase_count,
        waveform_status,
        waveform_peaks,
        beat_markers_ms,
        downbeat_markers_ms,
        phrase_markers_ms,
        mix_in_markers_ms,
        mix_out_markers_ms,
        drop_markers_ms,
        manual_drop_markers_ms,
        passive_analysis_status: passive_analysis
            .as_ref()
            .map(|snapshot| snapshot.status.to_string()),
        passive_analysis_reason: passive_analysis
            .as_ref()
            .map(|snapshot| snapshot.reason.to_string()),
        safe_crossfade_only: correction.is_some_and(|row| row.safe_crossfade_only),
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
                e.runtime_rendered_dj_mixer, e.runtime_renderer_status, e.runtime_renderer_reason
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
    if values.len() % 2 == 0 {
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
        "template_not_renderable" | "timing_unstable" | "overlay_not_handoff"
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
        drop_source: "program_json".to_string(),
    })
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
    use crate::services::audio_analysis::dj_profile::{DJ_PROFILE_VERSION, encode_f32_blob};

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
            waveform_peaks_blob: encode_f32_blob(&[0.0, 0.5, 1.0]),
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

    fn test_deck_status(profile_ready: bool, profile_status: &str) -> DjDeckStatus {
        DjDeckStatus {
            media_ref_kind: "tidal_track".to_string(),
            media_ref_id: "123".to_string(),
            title: "Test Track".to_string(),
            artist: Some("Test Artist".to_string()),
            profile_ready,
            profile_status: profile_status.to_string(),
            profile_error: None,
            profile_retry_after_ms: None,
            profile_retry_reason: None,
            profile_confidence: profile_ready.then_some(0.85),
            beat_count: profile_ready.then_some(128),
            downbeat_count: profile_ready.then_some(32),
            phrase_count: profile_ready.then_some(8),
            waveform_status: if profile_ready { "ready" } else { "missing" }.to_string(),
            waveform_peaks: if profile_ready {
                vec![0.0, 0.5, 1.0]
            } else {
                Vec::new()
            },
            beat_markers_ms: Vec::new(),
            downbeat_markers_ms: Vec::new(),
            phrase_markers_ms: Vec::new(),
            mix_in_markers_ms: Vec::new(),
            mix_out_markers_ms: Vec::new(),
            drop_markers_ms: Vec::new(),
            manual_drop_markers_ms: Vec::new(),
            passive_analysis_status: None,
            passive_analysis_reason: None,
            safe_crossfade_only: false,
        }
    }

    fn seed_dsp_key(conn: &rusqlite::Connection, track_id: i64, camelot_key: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO artists (id, name) VALUES (1, 'Test Artist')",
            [],
        )
        .expect("artist");
        conn.execute(
            "INSERT OR IGNORE INTO tracks (id, title, artist_id, source)
             VALUES (?1, ?2, 1, 'tidal')",
            params![track_id, format!("Track {track_id}")],
        )
        .expect("track");
        queries::upsert_audio_dsp_features(
            conn,
            &crate::db::models::AudioDspFeatures {
                track_id,
                bpm: Some(120.0),
                key_signature: None,
                camelot_key: Some(camelot_key.to_string()),
                loudness_lufs: Some(-12.0),
                energy: Some(0.7),
                danceability: Some(0.7),
                beat_strength: Some(0.7),
                spectral_centroid: None,
                stereo_width: None,
                is_instrumental: false,
                analysis_source: "test".to_string(),
                analysis_offset_ms: 0,
                samples_analyzed: None,
                analyzed_at: "now".to_string(),
                analysis_version: "test".to_string(),
            },
        )
        .expect("dsp features");
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
        let mut current_without_waveform = test_profile_row(&key, DJ_PROFILE_VERSION);
        current_without_waveform.waveform_peaks_blob = encode_f32_blob(&[]);
        queries::upsert_audio_dj_profile(&conn, &current_without_waveform)
            .expect("profile missing waveform");
        assert!(!dj_profile_is_current_version(&conn, &key).expect("missing waveform"));
    }

    #[test]
    fn deck_status_exposes_capped_waveform_peaks() {
        let conn = rusqlite::Connection::open_in_memory().expect("db");
        crate::db::schema::run_migrations(&conn).expect("migrations");
        let key = AudioDjProfileKey {
            media_ref_kind: "tidal_track".to_string(),
            media_ref_id: "123".to_string(),
        };
        let mut row = test_profile_row(&key, DJ_PROFILE_VERSION);
        row.waveform_peaks_blob = encode_f32_blob(&vec![0.75; DJ_WAVEFORM_PEAK_COUNT + 4]);
        queries::upsert_audio_dj_profile(&conn, &row).expect("profile");

        let deck = deck_status(
            &conn,
            &DjMediaRef::TidalTrack {
                tidal_id: 123,
                track_id: None,
            },
            Some(&("Track".to_string(), Some("Artist".to_string()))),
            false,
        )
        .expect("deck status");

        assert_eq!(deck.waveform_status, "ready");
        assert_eq!(deck.waveform_peaks.len(), DJ_WAVEFORM_PEAK_COUNT);
        assert!(deck.waveform_peaks.iter().all(|peak| *peak == 0.75));
    }

    #[test]
    fn deck_status_marks_missing_waveform() {
        let conn = rusqlite::Connection::open_in_memory().expect("db");
        crate::db::schema::run_migrations(&conn).expect("migrations");
        let key = AudioDjProfileKey {
            media_ref_kind: "tidal_track".to_string(),
            media_ref_id: "123".to_string(),
        };
        let mut row = test_profile_row(&key, DJ_PROFILE_VERSION);
        row.waveform_peaks_blob = encode_f32_blob(&[]);
        queries::upsert_audio_dj_profile(&conn, &row).expect("profile");

        let deck = deck_status(
            &conn,
            &DjMediaRef::TidalTrack {
                tidal_id: 123,
                track_id: None,
            },
            Some(&("Track".to_string(), Some("Artist".to_string()))),
            false,
        )
        .expect("deck status");

        assert_eq!(deck.waveform_status, "missing");
        assert!(deck.waveform_peaks.is_empty());
        assert!(deck_needs_profile_rebuild(&deck));
    }

    #[test]
    fn dj_profile_rebuild_uses_low_quality_analysis_stream() {
        let request = dj_profile_analysis_stream_request(28051328);

        assert_eq!(request.track_id, 28051328);
        assert_eq!(request.audio_quality, "LOW");
    }

    #[test]
    fn renderer_status_keeps_non_renderable_template_out_of_main_renderer() {
        let status = renderer_status_for_transition(Some(&OpenTransition {
            id: 17,
            template: "UnknownTemplate".to_string(),
            renderer_template: None,
            fallback_reason: None,
            planned_start_ms: None,
            actual_start_ms: None,
            timing_delta_ms: None,
            timing_source: None,
            timing_status: None,
            overlay_details: None,
            runtime_rendered_dj_mixer: None,
            runtime_renderer_status: None,
            runtime_renderer_reason: None,
            rejected_alternatives: Vec::new(),
        }));

        assert_eq!(status.planned_template.as_deref(), Some("UnknownTemplate"));
        assert_eq!(status.renderer_template, None);
        assert_eq!(status.renderer_mode.as_deref(), Some("legacy_overlap"));
        assert_eq!(
            status.downgrade_reason.as_deref(),
            Some("template_not_renderable")
        );
    }

    #[test]
    fn renderer_status_exposes_drop_tease_overlay_renderer() {
        let status = renderer_status_for_transition(Some(&OpenTransition {
            id: 171,
            template: "DropTease16".to_string(),
            renderer_template: Some("DropTease16".to_string()),
            fallback_reason: None,
            planned_start_ms: None,
            actual_start_ms: None,
            timing_delta_ms: None,
            timing_source: None,
            timing_status: None,
            overlay_details: Some(DjOverlayDetails {
                overlay_status: "armed".to_string(),
                overlay_start_ms: Some(120_000),
                overlay_end_ms: Some(151_000),
                tempo_ratio: Some(1.02),
                deck_b_start_frame: 384_000,
                drop_source: "program_json".to_string(),
            }),
            runtime_rendered_dj_mixer: None,
            runtime_renderer_status: None,
            runtime_renderer_reason: None,
            rejected_alternatives: Vec::new(),
        }));

        assert_eq!(status.planned_template.as_deref(), Some("DropTease16"));
        assert_eq!(status.renderer_template.as_deref(), Some("DropTease16"));
        assert_eq!(status.renderer_mode.as_deref(), Some("dj_overlay_program"));
        assert_eq!(status.downgrade_reason, None);
        assert_eq!(
            status.overlay_details,
            Some(DjOverlayDetails {
                overlay_status: "armed".to_string(),
                overlay_start_ms: Some(120_000),
                overlay_end_ms: Some(151_000),
                tempo_ratio: Some(1.02),
                deck_b_start_frame: 384_000,
                drop_source: "program_json".to_string(),
            })
        );
    }

    #[test]
    fn renderer_status_exposes_filter_sweep_runtime_program() {
        let status = renderer_status_for_transition(Some(&OpenTransition {
            id: 18,
            template: "FilterSweep".to_string(),
            renderer_template: Some("FilterSweep".to_string()),
            fallback_reason: None,
            planned_start_ms: Some(112_000),
            actual_start_ms: Some(112_144),
            timing_delta_ms: Some(144),
            timing_source: Some("downbeat_sync".to_string()),
            timing_status: Some("fired".to_string()),
            overlay_details: None,
            runtime_rendered_dj_mixer: None,
            runtime_renderer_status: None,
            runtime_renderer_reason: None,
            rejected_alternatives: Vec::new(),
        }));

        assert_eq!(status.planned_template.as_deref(), Some("FilterSweep"));
        assert_eq!(status.renderer_template.as_deref(), Some("FilterSweep"));
        assert_eq!(status.renderer_mode.as_deref(), Some("dj_gain_program"));
        assert_eq!(status.downgrade_reason, None);
        assert_eq!(status.timing_direction, "on_time");
    }

    #[test]
    fn renderer_status_exposes_bass_swap_16_runtime_program() {
        let status = renderer_status_for_transition(Some(&OpenTransition {
            id: 22,
            template: "BassSwap16".to_string(),
            renderer_template: Some("BassSwap16".to_string()),
            fallback_reason: None,
            planned_start_ms: Some(112_000),
            actual_start_ms: Some(112_144),
            timing_delta_ms: Some(144),
            timing_source: Some("downbeat_sync".to_string()),
            timing_status: Some("fired".to_string()),
            overlay_details: None,
            runtime_rendered_dj_mixer: None,
            runtime_renderer_status: None,
            runtime_renderer_reason: None,
            rejected_alternatives: Vec::new(),
        }));

        assert_eq!(status.planned_template.as_deref(), Some("BassSwap16"));
        assert_eq!(status.renderer_template.as_deref(), Some("BassSwap16"));
        assert_eq!(status.renderer_mode.as_deref(), Some("dj_gain_program"));
        assert_eq!(status.downgrade_reason, None);
    }

    #[test]
    fn renderer_status_exposes_new_runtime_programs() {
        for template in ["BassSwap32", "SlamCut", "LongHarmonicBlend"] {
            let status = renderer_status_for_transition(Some(&OpenTransition {
                id: 23,
                template: template.to_string(),
                renderer_template: Some(template.to_string()),
                fallback_reason: None,
                planned_start_ms: Some(112_000),
                actual_start_ms: Some(112_144),
                timing_delta_ms: Some(144),
                timing_source: Some("downbeat_sync".to_string()),
                timing_status: Some("fired".to_string()),
                overlay_details: None,
                runtime_rendered_dj_mixer: None,
                runtime_renderer_status: None,
                runtime_renderer_reason: None,
                rejected_alternatives: Vec::new(),
            }));

            assert_eq!(status.planned_template.as_deref(), Some(template));
            assert_eq!(status.renderer_template.as_deref(), Some(template));
            assert_eq!(status.renderer_mode.as_deref(), Some("dj_gain_program"));
            assert_eq!(status.downgrade_reason, None);
        }
    }

    #[test]
    fn renderer_template_from_program_json_accepts_new_runtime_programs() {
        for template in ["BassSwap32", "SlamCut", "LongHarmonicBlend"] {
            let program = noor_mix::TransitionProgram {
                tier: noor_mix::Tier::FullBlend,
                template: template.to_string(),
                sample_rate: 48_000,
                channels: 2,
                deck_a_start_frame: 0,
                deck_b_start_frame: 0,
                sync_start: 0,
                intro_start: 0,
                swap_start: 1,
                fade_start: 1,
                resolve_at: 1,
                loops: vec![],
                automation: vec![],
            };
            let json = serde_json::to_string(&program).expect("program json");

            assert_eq!(
                renderer_template_from_program_json(&json).as_deref(),
                Some(template)
            );
        }
    }

    #[test]
    fn overlay_details_from_program_json_exposes_drop_tease_facts() {
        let program = noor_mix::TransitionProgram {
            tier: noor_mix::Tier::FullBlend,
            template: "DropTease16".to_string(),
            sample_rate: 48_000,
            channels: 2,
            deck_a_start_frame: 0,
            deck_b_start_frame: 384_000,
            sync_start: 0,
            intro_start: 0,
            swap_start: 24_000,
            fade_start: 24_000,
            resolve_at: 48_000,
            loops: vec![],
            automation: vec![noor_mix::AutomationEvent {
                param: noor_mix::Param::PlaybackRate(noor_mix::DeckId::B),
                start_sample: 0,
                end_sample: 48_000,
                from: 1.02,
                to: 1.02,
                curve: noor_mix::Curve::Linear,
            }],
        };
        let json = serde_json::to_string(&program).expect("program json");

        assert_eq!(
            overlay_details_from_program_json(&json, Some(120_000), Some("fired")),
            Some(DjOverlayDetails {
                overlay_status: "fired".to_string(),
                overlay_start_ms: Some(120_000),
                overlay_end_ms: Some(121_000),
                tempo_ratio: Some(1.0199999809265137),
                deck_b_start_frame: 384_000,
                drop_source: "program_json".to_string(),
            })
        );
    }

    #[test]
    fn renderer_status_marks_safe_crossfade_renderer_as_pending() {
        let status = renderer_status_for_transition(Some(&OpenTransition {
            id: 19,
            template: "SafeCrossfade".to_string(),
            renderer_template: None,
            fallback_reason: None,
            planned_start_ms: None,
            actual_start_ms: None,
            timing_delta_ms: None,
            timing_source: None,
            timing_status: None,
            overlay_details: None,
            runtime_rendered_dj_mixer: None,
            runtime_renderer_status: None,
            runtime_renderer_reason: None,
            rejected_alternatives: Vec::new(),
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
            id: 20,
            template: "SlamCut".to_string(),
            renderer_template: Some("SafeCrossfade".to_string()),
            fallback_reason: None,
            planned_start_ms: Some(112_000),
            actual_start_ms: Some(112_144),
            timing_delta_ms: Some(144),
            timing_source: Some("downbeat_sync".to_string()),
            timing_status: Some("fired".to_string()),
            overlay_details: None,
            runtime_rendered_dj_mixer: Some(true),
            runtime_renderer_status: Some("rendered_handoff".to_string()),
            runtime_renderer_reason: Some("none".to_string()),
            rejected_alternatives: Vec::new(),
        }));

        assert_eq!(status.planned_template.as_deref(), Some("SlamCut"));
        assert_eq!(status.renderer_template.as_deref(), Some("SafeCrossfade"));
        assert_eq!(status.renderer_mode.as_deref(), Some("dj_gain_program"));
        assert_eq!(
            status.downgrade_reason.as_deref(),
            Some("template_not_renderable")
        );
        assert_eq!(status.planning_reason, None);
        assert_eq!(status.planned_start_ms, Some(112_000));
        assert_eq!(status.actual_start_ms, Some(112_144));
        assert_eq!(status.timing_delta_ms, Some(144));
        assert_eq!(status.timing_source.as_deref(), Some("downbeat_sync"));
        assert_eq!(status.timing_status.as_deref(), Some("fired"));
        assert_eq!(status.runtime_rendered_dj_mixer, Some(true));
        assert_eq!(
            status.runtime_renderer_status.as_deref(),
            Some("rendered_handoff")
        );
        assert_eq!(status.runtime_renderer_reason.as_deref(), Some("none"));
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
            overlay_details: None,
            runtime_rendered_dj_mixer: None,
            runtime_renderer_status: None,
            runtime_renderer_reason: None,
            rejected_alternatives: Vec::new(),
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
    fn renderer_status_reports_timing_unstable_as_downgrade_reason() {
        let status = renderer_status_for_transition(Some(&OpenTransition {
            id: 21,
            template: "FilterSweep".to_string(),
            renderer_template: Some("SafeCrossfade".to_string()),
            fallback_reason: Some("timing_unstable".to_string()),
            planned_start_ms: Some(202_091),
            actual_start_ms: Some(202_640),
            timing_delta_ms: Some(549),
            timing_source: Some("downbeat_sync".to_string()),
            timing_status: Some("fired".to_string()),
            overlay_details: None,
            runtime_rendered_dj_mixer: Some(false),
            runtime_renderer_status: Some("legacy_overlap".to_string()),
            runtime_renderer_reason: Some("prepared_mixer_missing".to_string()),
            rejected_alternatives: Vec::new(),
        }));

        assert_eq!(status.planned_template.as_deref(), Some("FilterSweep"));
        assert_eq!(status.renderer_template.as_deref(), Some("SafeCrossfade"));
        assert_eq!(status.downgrade_reason.as_deref(), Some("timing_unstable"));
        assert_eq!(status.planning_reason, None);
        assert_eq!(status.runtime_rendered_dj_mixer, Some(false));
        assert_eq!(
            status.runtime_renderer_status.as_deref(),
            Some("legacy_overlap")
        );
        assert_eq!(
            status.runtime_renderer_reason.as_deref(),
            Some("prepared_mixer_missing")
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
    fn timing_history_filters_manual_seek_suppressed_boundary_rows() {
        let conn = rusqlite::Connection::open_in_memory().expect("db");
        crate::db::schema::run_migrations(&conn).expect("migrations");
        conn.execute(
            "INSERT INTO dj_transition_events (
                from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                template, program_json, planner_version, planned_start_ms,
                actual_start_ms, timing_delta_ms, timing_source, timing_status,
                runtime_rendered_dj_mixer, runtime_renderer_status, runtime_renderer_reason
             ) VALUES (
                'tidal_track', '1', 'tidal_track', '2',
                'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
                222000, 240000, 18000, 'downbeat_sync', 'late',
                0, 'boundary_fallback', 'manual_seek_suppressed'
             )",
            [],
        )
        .expect("insert manual seek row");
        conn.execute(
            "INSERT INTO dj_transition_events (
                from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                template, program_json, planner_version, planned_start_ms,
                actual_start_ms, timing_delta_ms, timing_source, timing_status,
                runtime_rendered_dj_mixer, runtime_renderer_status, runtime_renderer_reason
             ) VALUES (
                'tidal_track', '2', 'tidal_track', '3',
                'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
                222000, 222040, 40, 'downbeat_sync', 'fired',
                1, 'rendered_handoff', 'none'
             )",
            [],
        )
        .expect("insert rendered row");

        let history = latest_dj_transition_timing_history(&conn, 5).expect("history");

        assert_eq!(history.len(), 1);
        assert_eq!(history[0].timing_delta_ms, Some(40));
        assert_eq!(history[0].runtime_renderer_reason.as_deref(), Some("none"));
    }

    #[test]
    fn latest_fired_timing_deltas_exclude_impossible_and_preview_rows() {
        let conn = rusqlite::Connection::open_in_memory().expect("db");
        crate::db::schema::run_migrations(&conn).expect("migrations");
        conn.execute(
            "INSERT INTO dj_transition_events (
                from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                template, program_json, planner_version, planned_start_ms,
                actual_start_ms, timing_delta_ms, timing_source, timing_status
             ) VALUES
             (
                'tidal_track', '1', 'tidal_track', '2',
                'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
                100000, 100120, 120, 'downbeat_sync', 'fired'
             ),
             (
                'tidal_track', '2', 'tidal_track', '3',
                'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
                100000, 140001, 40001, 'downbeat_sync', 'fired'
             ),
             (
                'tidal_track', '3', 'tidal_track', '4',
                'DropPreview16', '{\"template\":\"DropPreview16\"}', 'dj-v1',
                100000, 100240, 240, 'drop_preview', 'fired'
             )",
            [],
        )
        .expect("insert timing rows");

        let deltas = latest_fired_dj_timing_deltas(&conn, 20).expect("deltas");

        assert_eq!(deltas, vec![120]);
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
    fn timing_history_resolves_tidal_media_ref_labels_without_track_ids() {
        let conn = rusqlite::Connection::open_in_memory().expect("db");
        crate::db::schema::run_migrations(&conn).expect("migrations");
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (10, 'Outgoing Artist'), (11, 'Incoming Artist')",
            [],
        )
        .expect("insert artists");
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, source, tidal_id)
             VALUES (100, 'Outgoing Track', 10, 'tidal', 2501),
                    (101, 'Incoming Track', 11, 'tidal', 2502)",
            [],
        )
        .expect("insert tracks");
        conn.execute(
            "INSERT INTO dj_transition_events (
                from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                template, program_json, planner_version, planned_start_ms,
                actual_start_ms, timing_delta_ms, timing_source, timing_status
             ) VALUES (
                'tidal_track', '2501', 'tidal_track', '2502',
                'BassSwap16', '{\"template\":\"BassSwap16\"}', 'dj-v1',
                10000, 10120, 120, 'downbeat_sync', 'fired'
             )",
            [],
        )
        .expect("insert event");

        let history = latest_dj_transition_timing_history(&conn, 5).expect("history");

        assert_eq!(history[0].from_title.as_deref(), Some("Outgoing Track"));
        assert_eq!(history[0].from_artist.as_deref(), Some("Outgoing Artist"));
        assert_eq!(history[0].to_title.as_deref(), Some("Incoming Track"));
        assert_eq!(history[0].to_artist.as_deref(), Some("Incoming Artist"));
    }

    #[test]
    fn timing_quality_labels_delta_bands_and_missed() {
        assert_eq!(timing_quality(Some("fired"), Some(150)), "tight");
        assert_eq!(timing_quality(Some("fired"), Some(-500)), "usable");
        assert_eq!(timing_quality(Some("late"), Some(1000)), "loose");
        assert_eq!(timing_quality(Some("late"), Some(1001)), "bad");
        assert_eq!(timing_quality(Some("missed"), None), "bad");
        assert_eq!(timing_quality(Some("armed"), None), "pending");
        assert_eq!(timing_quality(Some("fired"), None), "bad");
    }

    #[test]
    fn timing_direction_labels_delta_direction_and_status() {
        assert_eq!(timing_direction(Some("fired"), Some(150)), "on_time");
        assert_eq!(timing_direction(Some("fired"), Some(-151)), "early");
        assert_eq!(timing_direction(Some("fired"), Some(151)), "late");
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
                runtime_rendered_dj_mixer: Some(true),
                runtime_renderer_status: Some("rendered_handoff".to_string()),
                runtime_renderer_reason: Some("none".to_string()),
                started_at: "now".to_string(),
                rejected_alternatives: Vec::new(),
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
                runtime_rendered_dj_mixer: Some(false),
                runtime_renderer_status: Some("legacy_overlap".to_string()),
                runtime_renderer_reason: Some("next_deck_not_decoded".to_string()),
                started_at: "now".to_string(),
                rejected_alternatives: Vec::new(),
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
                runtime_rendered_dj_mixer: None,
                runtime_renderer_status: None,
                runtime_renderer_reason: None,
                started_at: "now".to_string(),
                rejected_alternatives: Vec::new(),
            },
        ];

        let summary = summarize_timing_history(&events, &[100, 800, -1_200, 40]);

        assert_eq!(summary.event_count, 3);
        assert_eq!(summary.average_delta_ms, Some(450));
        assert_eq!(summary.average_abs_delta_ms, Some(450));
        assert_eq!(summary.median_abs_delta_ms, Some(450));
        assert_eq!(summary.worst_abs_delta_ms, Some(1_200));
        assert_eq!(summary.tight_count, 1);
        assert_eq!(summary.loose_count, 1);
        assert_eq!(summary.bad_count, 1);
        assert_eq!(summary.late_count, 1);
        assert_eq!(summary.missed_count, 1);
    }

    #[test]
    fn timing_history_summary_excludes_impossible_deltas_from_averages() {
        let events = vec![
            DjTimingHistoryEvent {
                event_id: 1,
                from_title: None,
                from_artist: None,
                to_title: None,
                to_artist: None,
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
                runtime_rendered_dj_mixer: Some(true),
                runtime_renderer_status: Some("rendered_handoff".to_string()),
                runtime_renderer_reason: Some("none".to_string()),
                started_at: "now".to_string(),
                rejected_alternatives: Vec::new(),
            },
            DjTimingHistoryEvent {
                event_id: 2,
                from_title: None,
                from_artist: None,
                to_title: None,
                to_artist: None,
                planned_template: "SafeCrossfade".to_string(),
                renderer_template: Some("SafeCrossfade".to_string()),
                planning_reason: None,
                planned_start_ms: Some(10_000),
                actual_start_ms: Some(50_001),
                timing_delta_ms: Some(40_001),
                timing_source: Some("downbeat_sync".to_string()),
                timing_status: Some("fired".to_string()),
                timing_quality: "bad".to_string(),
                timing_direction: "late".to_string(),
                runtime_rendered_dj_mixer: Some(true),
                runtime_renderer_status: Some("rendered_handoff".to_string()),
                runtime_renderer_reason: Some("none".to_string()),
                started_at: "now".to_string(),
                rejected_alternatives: Vec::new(),
            },
        ];

        let summary = summarize_timing_history(&events, &[100]);

        assert_eq!(summary.event_count, 2);
        assert_eq!(summary.average_delta_ms, Some(100));
        assert_eq!(summary.average_abs_delta_ms, Some(100));
        assert_eq!(summary.tight_count, 1);
        assert_eq!(summary.bad_count, 1);
    }

    #[test]
    fn fire_ahead_evidence_requires_positive_majority_and_median() {
        let passing = vec![
            220, 210, 205, 200, 195, 190, 185, 180, 175, 170, 165, 160, 155, 151, 149, -20, -40,
            -60, -80, -100,
        ];
        let mixed = vec![
            220, 210, 205, 200, 195, 190, 185, 180, 175, 170, -165, -160, -155, -151, -149, -20,
            -40, -60, -80, -100,
        ];
        let low_median = vec![
            151, 151, 150, 150, 149, 149, 148, 148, 147, 147, 146, 146, 145, 145, 144, -20, -40,
            -60, -80, -100,
        ];

        assert!(fire_ahead_evidence_passes(&passing));
        assert!(!fire_ahead_evidence_passes(&mixed));
        assert!(!fire_ahead_evidence_passes(&low_median));
        assert!(!fire_ahead_evidence_passes(&passing[..19]));
    }

    #[test]
    fn ready_pair_transition_is_due_only_near_track_end() {
        assert!(!ready_pair_transition_due(90_000, Some(180_000)));
        assert!(ready_pair_transition_due(151_000, Some(180_000)));
        assert!(ready_pair_transition_due(180_000, Some(180_000)));
        assert!(!ready_pair_transition_due(151_000, None));
    }

    #[test]
    fn drop_preview_selects_nearest_safe_mid_song_marker() {
        let mut current = test_deck_status(true, "ready");
        current.phrase_markers_ms = vec![40_000, 120_000];
        current.downbeat_markers_ms = vec![132_000, 190_000];

        assert_eq!(
            select_drop_preview_fire_ms(&current, Some(240_000)),
            Some(132_000)
        );
    }

    #[test]
    fn drop_preview_rejects_unsafe_mid_song_window() {
        let mut current = test_deck_status(true, "ready");
        current.phrase_markers_ms = vec![40_000, 190_000];
        current.downbeat_markers_ms = vec![59_000];

        assert_eq!(select_drop_preview_fire_ms(&current, Some(240_000)), None);
    }

    #[test]
    fn drop_preview_prefers_manual_drop_marker() {
        let mut next = test_deck_status(true, "ready");
        next.drop_markers_ms = vec![32_000];
        next.manual_drop_markers_ms = vec![24_000];

        assert_eq!(incoming_drop_marker(Some(&next)), Some((24_000, "manual")));
    }

    #[test]
    fn drop_preview_status_arms_compatible_ready_pair() {
        let conn = rusqlite::Connection::open_in_memory().expect("db");
        crate::db::schema::run_migrations(&conn).expect("migrations");
        seed_dsp_key(&conn, 1, "8A");
        seed_dsp_key(&conn, 2, "8B");
        let current_ref = DjMediaRef::TidalTrack {
            tidal_id: 111,
            track_id: Some(1),
        };
        let next_ref = DjMediaRef::TidalTrack {
            tidal_id: 222,
            track_id: Some(2),
        };
        let mut current = test_deck_status(true, "ready");
        current.phrase_markers_ms = vec![128_000];
        let mut next = test_deck_status(true, "ready");
        next.drop_markers_ms = vec![32_000];

        let status = drop_preview_status(
            &conn,
            true,
            Some(&current_ref),
            Some(&next_ref),
            Some(&current),
            Some(&next),
            Some(240_000),
            None,
        )
        .expect("preview status");

        assert_eq!(
            status,
            DjDropPreviewStatus {
                status: "armed".to_string(),
                planned_fire_ms: Some(128_000),
                actual_fire_ms: None,
                incoming_drop_ms: Some(32_000),
                source: Some("profile".to_string()),
                reason: None,
            }
        );
    }

    #[test]
    fn drop_preview_status_reports_actual_fire() {
        let conn = rusqlite::Connection::open_in_memory().expect("db");
        crate::db::schema::run_migrations(&conn).expect("migrations");
        seed_dsp_key(&conn, 1, "8A");
        seed_dsp_key(&conn, 2, "8B");
        let current_ref = DjMediaRef::TidalTrack {
            tidal_id: 111,
            track_id: Some(1),
        };
        let next_ref = DjMediaRef::TidalTrack {
            tidal_id: 222,
            track_id: Some(2),
        };
        let mut current = test_deck_status(true, "ready");
        current.phrase_markers_ms = vec![128_000];
        let mut next = test_deck_status(true, "ready");
        next.drop_markers_ms = vec![32_000];

        let status = drop_preview_status(
            &conn,
            true,
            Some(&current_ref),
            Some(&next_ref),
            Some(&current),
            Some(&next),
            Some(240_000),
            Some(128_008),
        )
        .expect("preview status");

        assert_eq!(status.status, "fired");
        assert_eq!(status.planned_fire_ms, Some(128_000));
        assert_eq!(status.actual_fire_ms, Some(128_008));
    }

    #[test]
    fn drop_preview_status_skips_harmonic_mismatch() {
        let conn = rusqlite::Connection::open_in_memory().expect("db");
        crate::db::schema::run_migrations(&conn).expect("migrations");
        seed_dsp_key(&conn, 1, "8A");
        seed_dsp_key(&conn, 2, "2B");
        let current_ref = DjMediaRef::TidalTrack {
            tidal_id: 111,
            track_id: Some(1),
        };
        let next_ref = DjMediaRef::TidalTrack {
            tidal_id: 222,
            track_id: Some(2),
        };
        let mut current = test_deck_status(true, "ready");
        current.phrase_markers_ms = vec![128_000];
        let mut next = test_deck_status(true, "ready");
        next.drop_markers_ms = vec![32_000];

        let status = drop_preview_status(
            &conn,
            true,
            Some(&current_ref),
            Some(&next_ref),
            Some(&current),
            Some(&next),
            Some(240_000),
            None,
        )
        .expect("preview status");

        assert_eq!(status.status, "skipped");
        assert_eq!(status.reason.as_deref(), Some("harmonic_incompatible"));
    }

    #[test]
    fn drop_preview_status_reports_retrying_asset_unavailable() {
        let conn = rusqlite::Connection::open_in_memory().expect("db");
        crate::db::schema::run_migrations(&conn).expect("migrations");
        let current_ref = DjMediaRef::TidalTrack {
            tidal_id: 111,
            track_id: Some(1),
        };
        let next_ref = DjMediaRef::TidalTrack {
            tidal_id: 222,
            track_id: Some(2),
        };
        let mut current = test_deck_status(true, "ready");
        current.phrase_markers_ms = vec![128_000];
        let mut next = test_deck_status(false, "retrying");
        next.profile_retry_reason = Some("asset_not_ready".to_string());

        let status = drop_preview_status(
            &conn,
            true,
            Some(&current_ref),
            Some(&next_ref),
            Some(&current),
            Some(&next),
            Some(240_000),
            None,
        )
        .expect("preview status");

        assert_eq!(status.status, "skipped");
        assert_eq!(
            status.reason.as_deref(),
            Some("next_profile_retrying_asset_not_ready")
        );
    }

    #[test]
    fn ready_pair_transition_planning_cooldown_suppresses_same_generation() {
        let mut attempts = HashMap::new();
        let now = Instant::now();
        let inside_retry = now + Duration::from_secs(DJ_READY_PAIR_PLANNING_RETRY_SECS - 1);
        let after_retry = now + Duration::from_secs(DJ_READY_PAIR_PLANNING_RETRY_SECS + 1);

        assert!(claim_ready_pair_transition_planning_at(
            &mut attempts,
            32335,
            4,
            now
        ));
        assert!(!claim_ready_pair_transition_planning_at(
            &mut attempts,
            32335,
            4,
            inside_retry
        ));
        assert!(claim_ready_pair_transition_planning_at(
            &mut attempts,
            32335,
            5,
            inside_retry
        ));
        assert!(claim_ready_pair_transition_planning_at(
            &mut attempts,
            32335,
            4,
            after_retry
        ));
    }

    #[test]
    fn pair_planning_status_reports_server_state() {
        let ready_current = test_deck_status(true, "ready");
        let ready_next = test_deck_status(true, "ready");
        let missing_next = test_deck_status(false, "missing");
        let retrying_next = test_deck_status(false, "retrying");
        let failed_next = test_deck_status(false, "decode_failed");
        let armed_transition = OpenTransition {
            id: 31,
            template: "SafeCrossfade".to_string(),
            renderer_template: Some("SafeCrossfade".to_string()),
            fallback_reason: None,
            planned_start_ms: Some(180_000),
            actual_start_ms: None,
            timing_delta_ms: None,
            timing_source: Some("downbeat_sync".to_string()),
            timing_status: Some("armed".to_string()),
            overlay_details: None,
            runtime_rendered_dj_mixer: None,
            runtime_renderer_status: None,
            runtime_renderer_reason: None,
            rejected_alternatives: Vec::new(),
        };

        assert_eq!(
            pair_planning_status(false, Some(&ready_current), Some(&ready_next), None, false),
            "disabled"
        );
        assert_eq!(
            pair_planning_status(true, None, Some(&ready_next), None, false),
            "pair_missing"
        );
        assert_eq!(
            pair_planning_status(true, Some(&ready_current), Some(&failed_next), None, false),
            "profile_failed"
        );
        assert_eq!(
            pair_planning_status(true, Some(&ready_current), Some(&missing_next), None, false),
            "waiting_for_profiles"
        );
        assert_eq!(
            pair_planning_status(true, Some(&ready_current), Some(&retrying_next), None, true),
            "waiting_for_profiles"
        );
        assert!(ready_pair_can_request_transition_planning(
            Some(&ready_current),
            Some(&retrying_next)
        ));
        assert!(!ready_pair_can_request_transition_planning(
            Some(&ready_current),
            Some(&failed_next)
        ));
        assert_eq!(
            pair_planning_status(
                true,
                Some(&ready_current),
                Some(&ready_next),
                Some(&armed_transition),
                true
            ),
            "armed"
        );
        assert_eq!(
            pair_planning_status(true, Some(&ready_current), Some(&ready_next), None, false),
            "waiting_for_window"
        );
        assert_eq!(
            pair_planning_status(true, Some(&ready_current), Some(&ready_next), None, true),
            "ready_to_plan"
        );
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

    #[test]
    fn forced_profile_rebuild_bypasses_recent_inflight_marker() {
        let inflight = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let first = mark_dj_profile_rebuild_inflight(
            &inflight,
            "tidal_track:250295728",
            std::time::Duration::from_secs(60),
        )
        .expect("first mark");
        let forced = mark_dj_profile_rebuild_inflight(
            &inflight,
            "tidal_track:250295728",
            std::time::Duration::ZERO,
        )
        .expect("force mark");

        assert_eq!(first, ProfileRebuildInflightDecision::Start);
        assert_eq!(forced, ProfileRebuildInflightDecision::Start);
    }

    #[test]
    fn retryable_profile_rebuild_errors_are_retrying() {
        let error = anyhow::anyhow!("DASH stream prebuffer failed");

        assert_eq!(profile_rebuild_failure_status(&error), "retrying");
        assert_eq!(
            profile_rebuild_error_message(&error, "retrying"),
            "DASH stream prebuffer failed. Retrying analysis."
        );
    }

    #[test]
    fn asset_not_ready_profile_rebuild_errors_are_retrying() {
        let error = anyhow::Error::msg(
            r#"TIDAL playback request was rejected: TIDAL rejected playback request with 401 Unauthorized: {"status":401,"subStatus":4005,"userMessage":"Asset is not ready for playback"}"#,
        );

        assert_eq!(profile_rebuild_failure_status(&error), "retrying");
        assert_eq!(
            profile_rebuild_error_message(&error, "retrying"),
            "TIDAL asset is not ready. Retrying analysis."
        );
    }

    #[test]
    fn deck_status_exposes_recent_profile_decode_failure() {
        let conn = rusqlite::Connection::open_in_memory().expect("db");
        crate::db::schema::run_migrations(&conn).expect("migrations");
        let media_ref = DjMediaRef::TidalTrack {
            tidal_id: 12198473,
            track_id: None,
        };
        let key = media_ref.profile_key();
        let rebuild_key = dj_profile_inflight_key(&key);
        clear_dj_profile_rebuild_failure(&rebuild_key);
        record_dj_profile_rebuild_failure(
            &rebuild_key,
            "decode_failed",
            "DASH stream prebuffer failed".to_string(),
        );

        let deck = deck_status(&conn, &media_ref, None, false).expect("deck status");

        assert!(!deck.profile_ready);
        assert_eq!(deck.profile_status, "decode_failed");
        assert_eq!(
            deck.profile_error.as_deref(),
            Some("DASH stream prebuffer failed")
        );
        assert!(!deck_needs_profile_rebuild(&deck));

        clear_dj_profile_rebuild_failure(&rebuild_key);
    }

    #[test]
    fn deck_status_exposes_recent_profile_retrying_failure() {
        let conn = rusqlite::Connection::open_in_memory().expect("db");
        crate::db::schema::run_migrations(&conn).expect("migrations");
        let media_ref = DjMediaRef::TidalTrack {
            tidal_id: 12198475,
            track_id: None,
        };
        let key = media_ref.profile_key();
        let rebuild_key = dj_profile_inflight_key(&key);
        clear_dj_profile_rebuild_failure(&rebuild_key);
        record_dj_profile_rebuild_failure(
            &rebuild_key,
            "retrying",
            "DASH stream prebuffer failed. Retrying analysis.".to_string(),
        );

        let deck = deck_status(&conn, &media_ref, None, false).expect("deck status");

        assert!(!deck.profile_ready);
        assert_eq!(deck.profile_status, "retrying");
        assert_eq!(
            deck.profile_error.as_deref(),
            Some("DASH stream prebuffer failed. Retrying analysis.")
        );
        assert!(deck.profile_retry_after_ms.is_some_and(|ms| ms > 0));
        assert_eq!(deck.profile_retry_reason.as_deref(), Some("dash_prebuffer"));
        assert!(!deck_needs_profile_rebuild(&deck));

        clear_dj_profile_rebuild_failure(&rebuild_key);
    }

    #[test]
    fn due_retrying_profile_failure_needs_rebuild() {
        let mut deck = test_deck_status(false, "retrying");
        deck.profile_retry_after_ms = Some(0);

        assert!(deck_needs_profile_rebuild(&deck));
    }

    #[test]
    fn asset_not_ready_retrying_profile_reports_retry_reason() {
        let conn = rusqlite::Connection::open_in_memory().expect("db");
        crate::db::schema::run_migrations(&conn).expect("migrations");
        let media_ref = DjMediaRef::TidalTrack {
            tidal_id: 12198476,
            track_id: None,
        };
        let key = media_ref.profile_key();
        let rebuild_key = dj_profile_inflight_key(&key);
        clear_dj_profile_rebuild_failure(&rebuild_key);
        record_dj_profile_rebuild_failure(
            &rebuild_key,
            "retrying",
            "TIDAL asset is not ready. Retrying analysis.".to_string(),
        );

        let deck = deck_status(&conn, &media_ref, None, false).expect("deck status");

        assert_eq!(deck.profile_status, "retrying");
        assert_eq!(
            deck.profile_retry_reason.as_deref(),
            Some("asset_not_ready")
        );
        assert!(deck.profile_retry_after_ms.is_some_and(|ms| ms > 0));

        clear_dj_profile_rebuild_failure(&rebuild_key);
    }

    #[test]
    fn retryable_profile_rebuild_failure_clears_inflight() {
        let inflight = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let key = "tidal_track:250295729";
        let first = mark_dj_profile_rebuild_inflight(
            &inflight,
            key,
            std::time::Duration::from_secs(DJ_PROFILE_AUTO_REBUILD_RETRY_SECS),
        )
        .expect("first mark");

        finish_dj_profile_rebuild_failure(
            &inflight,
            key,
            "retrying",
            "TIDAL asset is not ready. Retrying analysis.".to_string(),
        );

        let second = mark_dj_profile_rebuild_inflight(
            &inflight,
            key,
            std::time::Duration::from_secs(DJ_PROFILE_AUTO_REBUILD_RETRY_SECS),
        )
        .expect("second mark");

        assert_eq!(first, ProfileRebuildInflightDecision::Start);
        assert_eq!(second, ProfileRebuildInflightDecision::Start);
        clear_dj_profile_rebuild_failure(key);
    }

    #[test]
    fn deck_status_exposes_inflight_profile_as_analyzing() {
        let conn = rusqlite::Connection::open_in_memory().expect("db");
        crate::db::schema::run_migrations(&conn).expect("migrations");
        let media_ref = DjMediaRef::TidalTrack {
            tidal_id: 12198474,
            track_id: None,
        };

        let deck = deck_status(&conn, &media_ref, None, true).expect("deck status");

        assert!(!deck.profile_ready);
        assert_eq!(deck.profile_status, "analyzing");
        assert!(deck.profile_error.is_none());
    }
}
