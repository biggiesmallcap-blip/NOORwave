use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use chrono::Utc;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::SharedState;
use crate::db::{
    models::{AudioDjProfileCorrectionRow, AudioDjProfileKey, AudioDjProfileRow},
    queries,
};
use crate::playback::dj_lookahead::DjMediaRef;
use crate::playback::player;
use crate::services::audio_analysis::dj_profile::{decode_f32_blob, decode_u32_blob};

const DEFAULT_DJ_LOOKAHEAD_DEADLINE_SAMPLES: u64 = 48_000 * 30;
const DJ_PROFILE_CONFIDENCE_FLOOR: f64 = 0.6;
const SAFE_SUGGESTION_BAD_COUNT: i64 = 3;

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
    fallback_reason: Option<String>,
    profile_confidence_floor: f64,
    last_transition_event_id: Option<i64>,
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
                    player::build_dj_lookahead_start(conn, DEFAULT_DJ_LOOKAHEAD_DEADLINE_SAMPLES)
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
    let response = {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| {
                let enabled = queries::is_dj_engine_enabled(conn)?;
                let pair = crate::playback::dj_lookahead::load_dj_lookahead_pair(conn)?;
                let current = match pair.current {
                    Some(media_ref) => Some(deck_status(conn, &media_ref)?),
                    None => None,
                };
                let next = match pair.next {
                    Some(media_ref) => Some(deck_status(conn, &media_ref)?),
                    None => None,
                };
                let safe_crossfade_suggestion =
                    safe_crossfade_suggestion(conn, current.as_ref(), next.as_ref())?;
                Ok(DjStatusResponse {
                    enabled,
                    current,
                    next,
                    selected_program: None,
                    fallback_reason: if enabled {
                        None
                    } else {
                        Some("disabled".to_string())
                    },
                    profile_confidence_floor: DJ_PROFILE_CONFIDENCE_FLOOR,
                    last_transition_event_id: None,
                    safe_crossfade_suggestion,
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
    let accepted = {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| {
                let key = AudioDjProfileKey {
                    media_ref_kind: payload.media_ref_kind.clone(),
                    media_ref_id: payload.media_ref_id.clone(),
                };
                let pair = crate::playback::dj_lookahead::load_dj_lookahead_pair(conn)?;
                Ok(pair
                    .current
                    .as_ref()
                    .is_some_and(|media_ref| media_ref.profile_key() == key)
                    || pair
                        .next
                        .as_ref()
                        .is_some_and(|media_ref| media_ref.profile_key() == key))
            })
            .unwrap_or(false)
    };
    Ok(Json(RebuildDjProfileResponse {
        accepted,
        status: if accepted {
            "accepted".to_string()
        } else {
            "not_current_pair".to_string()
        },
    }))
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
) -> anyhow::Result<DjDeckStatus> {
    let key = media_ref.profile_key();
    let profile = queries::get_audio_dj_profile(conn, &key)?;
    let correction = queries::get_audio_dj_profile_correction(conn, &key)?;
    let (title, artist) = media_ref_label(conn, media_ref)?;
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
