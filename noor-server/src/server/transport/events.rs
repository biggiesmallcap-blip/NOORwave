//! Reacting to audio runtime events: ready, near end, finished, track errors, exit; the skip-aware snapshot switch.

use super::{
    listen::{flush_active_listen_session_locked, sync_session_after_snapshot},
    pending::{resolve_or_skip_pending_current, spawn_tidal_id_reresolve},
    settings::{
        current_user_audio_quality, effective_crossfade_ms,
        runtime_output_settings_from_audio_settings,
        should_skip_prebuffer_for_sample_rate_follow_format_change, transport_intent_is_playing,
    },
    snapshot::recently_cleared,
};
use crate::db::queries;
use crate::playback::history::PlayHistoryEntry;
use crate::playback::{player, runtime as playback_runtime};
use crate::server::routes::{
    NearEndPreparationOutcome, PLAYBACK_ADVANCE_PENDING_SKIP_LIMIT,
    PLAYBACK_FINISH_DB_LOCK_RETRY_DELAY_SECS, PLAYBACK_FINISH_DB_LOCK_RETRY_LIMIT,
    RUNTIME_TRACK_RETRY_MARKER, active_dj_lookahead_start_for_state,
    active_dj_pair_for_state_and_conn, dj_routes, ensure_playback_runtime_for_track,
    queue_missing_dj_profiles_after_pair_change, refresh_dj_after_queue_change,
    remove_unavailable_upcoming_row, should_retry_exclusive_release, sqlite_database_locked,
    start_dj_lookahead_and_queue_profiles_after_pair_change,
};
use crate::server::transport::generation::{
    current as current_playback_generation, is_current as playback_generation_is_current,
};
use crate::server::transport::start::{Dispatch, StartError, StartRequest, start_track};
use crate::server::transport::stream::{TidalPlaybackError, resolve_tidal_playback_stream};
use crate::services::tidal::stream as tidal_stream;
use crate::{AppEvent, PlaybackRuntimeInfo, SharedState};
use std::time::Duration;
use tracing::{error, info, warn};

pub(crate) async fn apply_runtime_ready(
    state: &SharedState,
    handle: &playback_runtime::PlaybackRuntimeHandle,
    device_name: String,
    sample_rate: u32,
    channels: u16,
) -> bool {
    let mut guard = state.write().await;
    if !guard
        .playback_runtime
        .as_ref()
        .is_some_and(|runtime| runtime.handle.is_same_runtime(handle))
    {
        return false;
    }

    // Ready is emitted at startup AND after a device/sample-rate swap. A
    // swapped engine keeps playing, so preserve its active track and flag.
    let previous = guard.playback_runtime_info.as_ref();
    let active_track_id = previous.and_then(|info| info.active_track_id);
    let last_error = previous.and_then(|info| info.last_error.clone());
    let exclusive_engaged = previous.is_some_and(|info| info.exclusive_engaged);
    let exclusive_transport_format = previous
        .filter(|info| info.exclusive_engaged)
        .and_then(|info| info.exclusive_transport_format.clone());
    if active_track_id.is_none() {
        guard
            .audio_active
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }
    guard.playback_runtime_info = Some(PlaybackRuntimeInfo {
        device_name,
        sample_rate,
        channels,
        active_track_id,
        last_error,
        exclusive_engaged,
        exclusive_transport_format,
    });
    true
}

pub(crate) fn spawn_playback_runtime_listener(
    state: SharedState,
    handle: playback_runtime::PlaybackRuntimeHandle,
) {
    // Subscribe before scheduling the task: native runtime events can arrive
    // before Tokio first polls it. Replay device metadata if startup Ready
    // was already sent before the handle reached this layer.
    let mut rx = handle.subscribe();
    if let Err(error) = handle.request_ready() {
        warn!("Could not request runtime device metadata: {error}");
    }
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(playback_runtime::PlaybackRuntimeEvent::Finished {
                    track_id,
                    generation,
                }) => {
                    // A late terminal from the previous track or runtime must
                    // not mark a newer, already audible track as paused.
                    let state_guard = state.read().await;
                    if current_playback_generation(&state_guard) != generation
                        || !state_guard
                            .playback_runtime
                            .as_ref()
                            .is_some_and(|runtime| runtime.handle.is_same_runtime(&handle))
                    {
                        continue;
                    }
                    state_guard
                        .audio_active
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    drop(state_guard);
                    if let Err(error) =
                        handle_runtime_finished_with_retry(state.clone(), track_id, generation)
                            .await
                    {
                        let message =
                            format!("Failed to advance playback after track end: {error}");
                        report_playback_failure(&state, &message);
                        error!("{message}");
                    }
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::DjTransitionPromoted {
                    transition_event_id,
                    actual_start_ms,
                    runtime_planned_start_ms,
                    timing_status,
                    runtime_rendered_dj_mixer,
                    runtime_renderer_status,
                    runtime_renderer_reason,
                    runtime_program_json,
                    ..
                }) => {
                    let state_guard = state.read().await;
                    match state_guard.db.with_conn(|conn| {
                        queries::update_dj_transition_fire_timing_with_runtime_target(
                            conn,
                            transition_event_id,
                            actual_start_ms,
                            runtime_planned_start_ms,
                            timing_status.as_str(),
                            runtime_rendered_dj_mixer,
                            runtime_renderer_status.as_str(),
                            runtime_renderer_reason.as_str(),
                        )?;
                        if let Some(program_json) = runtime_program_json.as_ref() {
                            // Keep the event identity/timing, but display its
                            // executed cue, rate and fallback instead of an
                            // earlier planner hypothesis.
                            conn.execute(
                                "UPDATE dj_transition_events SET program_json=?1,
                                 fallback_reason=CASE WHEN template!=json_extract(?1,'$.template')
                                   AND json_extract(?1,'$.template') IN ('SafeCrossfade','SlamCut')
                                   THEN 'beat_sync_unverified' ELSE fallback_reason END WHERE id=?2",
                                rusqlite::params![program_json, transition_event_id],
                            )?;
                        }
                        Ok(())
                    }) {
                        Ok(()) => {
                            info!(
                                transition_event_id,
                                actual_start_ms,
                                timing_status = %timing_status,
                                "Recorded DJ transition timing"
                            );
                        }
                        Err(error) => {
                            warn!("Failed to record DJ transition timing: {error}");
                        }
                    }
                    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::Error { message }) => {
                    handle_runtime_error(state.clone(), &message).await;
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::Exited { message }) => {
                    handle_runtime_exit(&state, &handle, message.as_deref()).await;
                    break;
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::TrackError {
                    track_id,
                    generation,
                    message,
                }) => {
                    if let Err(error) =
                        handle_runtime_track_error(state.clone(), track_id, generation, &message)
                            .await
                    {
                        let message =
                            format!("Failed to advance playback after track error: {error}");
                        report_playback_failure(&state, &message);
                        error!("{message}");
                    }
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::PreparedTrackError {
                    track_id,
                    generation,
                    tidal_id,
                    message,
                }) => {
                    handle_prepared_runtime_track_error_for_runtime(
                        &state,
                        Some(&handle),
                        track_id,
                        generation,
                        tidal_id,
                        &message,
                    )
                    .await;
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::Ready {
                    device_name,
                    sample_rate,
                    channels,
                }) => {
                    if !apply_runtime_ready(&state, &handle, device_name, sample_rate, channels)
                        .await
                    {
                        break;
                    }
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::Started {
                    track_id,
                    generation,
                    ..
                }) => {
                    // Read the persisted queue anchor before taking the global
                    // write lock. A briefly busy SQLite connection must not
                    // block pause, resume, queue, or runtime-health updates.
                    let history_row = {
                        let state_guard = state.read().await;
                        if current_playback_generation(&state_guard) != generation {
                            continue;
                        }
                        state_guard
                            .db
                            .with_conn(|conn| {
                                Ok(conn
                                    .query_row(
                                        "SELECT current_track_id, current_queue_item_id
                                         FROM playback_state WHERE id = 1",
                                        [],
                                        |row| {
                                            Ok((
                                                row.get::<_, Option<i64>>(0)?,
                                                row.get::<_, Option<i64>>(1)?,
                                            ))
                                        },
                                    )
                                    .ok()
                                    .and_then(|(track, queue_item)| track.zip(queue_item)))
                            })
                            .ok()
                            .flatten()
                    };
                    let mut state_guard = state.write().await;
                    if current_playback_generation(&state_guard) != generation {
                        continue;
                    }
                    // CPAL buffer threshold crossed. Samples are actually flowing now.
                    state_guard
                        .audio_active
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    if let Some(info) = state_guard.playback_runtime_info.as_mut() {
                        info.active_track_id = Some(track_id);
                        info.last_error = None;
                    }
                    if let Some(pending) = state_guard.pending_stream_display.take() {
                        state_guard.current_stream_display = Some(pending);
                    }
                    // Record history only when the active persisted queue row matches.
                    // A missing record is safer than attributing a play to another row.
                    if let Some((anchored_track_id, queue_item_id)) = history_row
                        && anchored_track_id == track_id
                    {
                        state_guard.play_history.note_started(
                            PlayHistoryEntry::Persisted {
                                queue_item_id,
                                track_id: anchored_track_id,
                            },
                            generation,
                        );
                    }
                    drop(state_guard);
                    let state_guard = state.read().await;
                    let _ = state_guard
                        .event_tx
                        .send(AppEvent::TrackChanged { track_id });
                    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
                    drop(state_guard);
                    start_dj_lookahead_and_queue_profiles_after_pair_change(
                        state.clone(),
                        handle.clone(),
                        "playback_started",
                    )
                    .await;
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::Paused { .. }) => {
                    let state_guard = state.read().await;
                    if !state_guard
                        .playback_runtime
                        .as_ref()
                        .is_some_and(|runtime| runtime.handle.is_same_runtime(&handle))
                    {
                        break;
                    }
                    drop(state_guard);
                    // The runtime acknowledged pause (user command or the
                    // advance-cascade breaker). Reconcile DB/UI to it so the
                    // pause button always reflects what is actually audible.
                    reconcile_runtime_transport_state(&state, false).await;
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::Resumed { .. }) => {
                    let state_guard = state.read().await;
                    if !state_guard
                        .playback_runtime
                        .as_ref()
                        .is_some_and(|runtime| runtime.handle.is_same_runtime(&handle))
                    {
                        break;
                    }
                    drop(state_guard);
                    reconcile_runtime_transport_state(&state, true).await;
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::Preparing { .. }) => {}
                Ok(playback_runtime::PlaybackRuntimeEvent::Stalled { track_id }) => {
                    // The audible engine froze (hung stream). Stop listen-time
                    // accrual now: the session timer is wall-clock based and
                    // would otherwise keep counting silence as listening
                    // (observed: 2795 s recorded on a 334 s track). Playback
                    // recovery itself is the watchdog's force-advance.
                    let mut state_guard = state.write().await;
                    let now = chrono::Utc::now();
                    if let Some(session) = state_guard.active_listen_session.as_mut()
                        && session.track_id == track_id
                    {
                        session.pause(now);
                    }
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::StallRecovered { track_id }) => {
                    let mut state_guard = state.write().await;
                    let now = chrono::Utc::now();
                    if let Some(session) = state_guard.active_listen_session.as_mut()
                        && session.track_id == track_id
                    {
                        session.resume(now);
                    }
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::DropPreviewStarted {
                    track_id,
                    generation,
                    actual_start_ms,
                    queue_generation,
                }) => {
                    let mut state_guard = state.write().await;
                    if current_playback_generation(&state_guard) != generation
                        || !state_guard
                            .playback_runtime
                            .as_ref()
                            .is_some_and(|runtime| runtime.handle.is_same_runtime(&handle))
                    {
                        continue;
                    }
                    state_guard.last_drop_preview = Some(crate::DropPreviewRuntimeState {
                        track_id,
                        generation,
                        queue_generation,
                        actual_fire_ms: Some(actual_start_ms),
                        skipped_reason: None,
                    });
                    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::DropPreviewSkipped {
                    track_id,
                    generation,
                    queue_generation,
                    reason,
                }) => {
                    let mut state_guard = state.write().await;
                    if current_playback_generation(&state_guard) != generation
                        || !state_guard
                            .playback_runtime
                            .as_ref()
                            .is_some_and(|runtime| runtime.handle.is_same_runtime(&handle))
                    {
                        continue;
                    }
                    state_guard.last_drop_preview = Some(crate::DropPreviewRuntimeState {
                        track_id,
                        generation,
                        queue_generation,
                        actual_fire_ms: None,
                        skipped_reason: Some(reason),
                    });
                    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::Stopped) => {
                    let mut state_guard = state.write().await;
                    if !state_guard
                        .playback_runtime
                        .as_ref()
                        .is_some_and(|runtime| runtime.handle.is_same_runtime(&handle))
                    {
                        break;
                    }
                    state_guard
                        .audio_active
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    if let Some(info) = state_guard.playback_runtime_info.as_mut() {
                        info.active_track_id = None;
                    }
                    state_guard.current_stream_display = None;
                    state_guard.pending_stream_display = None;
                    state_guard.next_prebuffer_inflight = None;
                    state_guard.last_drop_preview = None;
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::NearEnd {
                    track_id,
                    generation,
                }) => {
                    // Pre-decode the next track so the transition is gapless.
                    let next_state = state.clone();
                    tokio::spawn(async move {
                        if let Err(err) = handle_near_end(next_state, track_id, generation).await {
                            warn!("Failed to pre-buffer next track: {err:?}");
                        }
                    });
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::ExclusiveModeEngaged {
                    device_name,
                    transport_format,
                }) => {
                    let mut state_guard = state.write().await;
                    if let Some(info) = state_guard.playback_runtime_info.as_mut() {
                        info.exclusive_engaged = true;
                        info.exclusive_transport_format = Some(transport_format.clone());
                    }
                    let _ = state_guard.event_tx.send(AppEvent::AudioExclusiveEngaged {
                        device: device_name,
                        transport_format,
                    });
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::ExclusiveModeFailed {
                    reason,
                    device_name,
                }) => {
                    let mut state_guard = state.write().await;
                    if let Some(info) = state_guard.playback_runtime_info.as_mut() {
                        info.exclusive_engaged = false;
                        info.exclusive_transport_format = None;
                    }
                    let _ = state_guard.event_tx.send(AppEvent::AudioExclusiveFailed {
                        device: device_name,
                        reason,
                    });
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::ExclusiveModeReleased {
                    device_name,
                }) => {
                    let mut state_guard = state.write().await;
                    if let Some(info) = state_guard.playback_runtime_info.as_mut() {
                        info.exclusive_engaged = false;
                        info.exclusive_transport_format = None;
                    }
                    let _ = state_guard.event_tx.send(AppEvent::AudioExclusiveReleased {
                        device: device_name,
                    });
                    let retry = {
                        let settings = state_guard
                            .db
                            .with_conn(|conn| {
                                crate::db::audio_settings::load(conn).map_err(anyhow::Error::from)
                            })
                            .ok();
                        let is_playing = state_guard
                            .db
                            .with_conn(|conn| player::load_state(conn).map(|s| s.is_playing))
                            .unwrap_or(false);
                        let runtime = state_guard
                            .playback_runtime
                            .as_ref()
                            .map(|runtime| runtime.handle.clone());
                        settings.and_then(|settings| {
                            if should_retry_exclusive_release(is_playing, settings.exclusive_mode) {
                                runtime.map(|runtime| {
                                    (
                                        runtime,
                                        runtime_output_settings_from_audio_settings(&settings),
                                    )
                                })
                            } else {
                                None
                            }
                        })
                    };
                    drop(state_guard);
                    if let Some((runtime, output)) = retry
                        && let Err(error) = runtime.device_swap(
                            output.device,
                            output.exclusive_mode,
                            output.sample_rate_follow,
                            None,
                            output.exclusive_release_grace_secs,
                            output.exclusive_latency_mode,
                        )
                    {
                        warn!("Failed to recover released WASAPI exclusive stream: {error}");
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!("Playback runtime listener lagged by {skipped} events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Peek at what would play next without advancing the queue, then send `PrepareNext` to the
/// runtime so it can pre-decode the track and swap it in gaplessly when the current one ends.
pub(crate) async fn handle_near_end(
    state: SharedState,
    current_track_id: i64,
    generation: u64,
) -> anyhow::Result<bool> {
    let mut skipped = false;
    let mut result = Ok(false);
    for _ in 0..PLAYBACK_ADVANCE_PENDING_SKIP_LIMIT {
        match prepare_near_end_once(state.clone(), current_track_id, generation).await {
            Ok(NearEndPreparationOutcome::SkippedUnavailable) => {
                skipped = true;
                result = Ok(true);
            }
            Ok(NearEndPreparationOutcome::Attempted) => {
                result = Ok(true);
                break;
            }
            Ok(NearEndPreparationOutcome::Idle) => {
                result = Ok(skipped);
                break;
            }
            Err(error) => {
                result = Err(error);
                break;
            }
        }
    }
    if skipped {
        refresh_dj_after_queue_change(state, "skip_unavailable_prebuffer").await;
    }
    result
}

pub(crate) async fn prepare_near_end_once(
    state: SharedState,
    current_track_id: i64,
    generation: u64,
) -> anyhow::Result<NearEndPreparationOutcome> {
    let (next_track, expected_pair, runtime_handle, crossfade_ms) = {
        let state_guard = state.read().await;

        // Guard: only proceed if the current track is still the one that fired NearEnd.
        let active_id = state_guard
            .playback_runtime_info
            .as_ref()
            .and_then(|info| info.active_track_id);
        if active_id != Some(current_track_id) {
            return Ok(NearEndPreparationOutcome::Idle);
        }
        if current_playback_generation(&state_guard) != generation {
            return Ok(NearEndPreparationOutcome::Idle);
        }

        let cleared = recently_cleared(&state_guard);
        let (next, pair) = state_guard.db.with_conn(|conn| {
            let next = player::peek_next_track(conn, cleared)?;
            Ok((
                next,
                crate::playback::dj_lookahead::load_dj_lookahead_pair(conn)?,
            ))
        })?;
        let handle = state_guard
            .playback_runtime
            .as_ref()
            .map(|r| r.handle.clone());
        let crossfade = state_guard.db.with_conn(|conn| {
            conn.query_row(
                "SELECT crossfade_ms FROM playback_state WHERE id = 1",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map_err(anyhow::Error::from)
        })?;

        (next, pair, handle, crossfade)
    };

    let (Some(next), Some(handle)) = (next_track, runtime_handle) else {
        return Ok(NearEndPreparationOutcome::Idle);
    };
    if matches!(
        handle.track_status(next.id, generation),
        playback_runtime::PlaybackTrackStatus::Active
            | playback_runtime::PlaybackTrackStatus::Prepared
    ) {
        return Ok(NearEndPreparationOutcome::Idle);
    }
    let prebuffer_key = crate::NextPrebufferKey {
        current_track_id,
        next_track_id: next.id,
        generation,
    };
    {
        let mut state_guard = state.write().await;
        if !claim_next_prebuffer_slot(&mut state_guard.next_prebuffer_inflight, prebuffer_key) {
            return Ok(NearEndPreparationOutcome::Idle);
        }
    }

    let result = handle_near_end_prebuffer_next(
        state.clone(),
        current_track_id,
        generation,
        expected_pair,
        next,
        handle,
        crossfade_ms,
    )
    .await;
    {
        let mut state_guard = state.write().await;
        release_next_prebuffer_slot(&mut state_guard.next_prebuffer_inflight, prebuffer_key);
    }
    result.map(|skipped| {
        if skipped {
            NearEndPreparationOutcome::SkippedUnavailable
        } else {
            NearEndPreparationOutcome::Attempted
        }
    })
}

pub(crate) async fn handle_near_end_prebuffer_next(
    state: SharedState,
    current_track_id: i64,
    generation: u64,
    expected_pair: crate::playback::dj_lookahead::DjLookaheadPair,
    next: crate::db::models::Track,
    handle: playback_runtime::PlaybackRuntimeHandle,
    crossfade_ms: i32,
) -> anyhow::Result<bool> {
    // Resolve the stream URL for the next track (we need a live access token).
    let user_quality = current_user_audio_quality(&state).await;
    let stream_request = match player::build_tidal_stream_request(&next, user_quality.clone()) {
        Some(req) => req,
        None => return Ok(false), // local library: skip pre-buffer for now
    };

    let stream_info = match resolve_tidal_playback_stream(&state, &next, &stream_request).await {
        Ok(info) => Some(info),
        Err(error) => {
            warn!(
                "Skipping pre-buffer for next track {}: {}",
                next.id,
                describe_tidal_playback_error(&error)
            );
            if error.is_track_unplayable() {
                let skipped = {
                    let mut guard = state.write().await;
                    if guard
                        .playback_runtime
                        .as_ref()
                        .is_some_and(|runtime| runtime.handle.is_same_runtime(&handle))
                    {
                        let skipped = remove_unavailable_upcoming_row(
                            &mut guard,
                            current_track_id,
                            generation,
                            &expected_pair,
                            &next,
                            &describe_tidal_playback_error(&error),
                        )?;
                        if skipped {
                            dj_routes::record_unavailable_tidal_source(stream_request.track_id);
                        }
                        skipped
                    } else {
                        false
                    }
                };
                if skipped && error.is_asset_not_ready() {
                    spawn_tidal_id_reresolve(&state, next.id);
                }
                return Ok(skipped);
            }
            return Ok(false);
        }
    };

    {
        let state_guard = state.read().await;
        let runtime_token = state_guard
            .playback_runtime
            .as_ref()
            .map(|runtime| runtime.access_token.as_str());
        let current_token = state_guard.tidal.tokens().map(|tokens| tokens.access_token);
        if runtime_token != current_token.as_deref() {
            info!(
                "Skipping pre-buffer for next track {} after TIDAL session refresh; next transition will cold-start",
                next.id
            );
            return Ok(false);
        }
    };

    {
        let state_guard = state.read().await;
        let active_id = state_guard
            .playback_runtime_info
            .as_ref()
            .and_then(|info| info.active_track_id);
        let db_current = state_guard
            .db
            .with_conn(player::current_track_id)
            .unwrap_or(None);
        let cleared = recently_cleared(&state_guard);
        let still_next = state_guard
            .db
            .with_conn(|conn| player::peek_next_track(conn, cleared))
            .ok()
            .flatten()
            .map(|track| track.id);
        if active_id != Some(current_track_id)
            || db_current != Some(current_track_id)
            || current_playback_generation(&state_guard) != generation
            || still_next != Some(next.id)
        {
            return Ok(false);
        }
    }

    // If sample_rate_follow is enabled and the next track's rate differs from current,
    // rebuild the output device at the new rate before PrepareNext.
    {
        let state_guard = state.read().await;
        if let (Some(stream), Some(info)) =
            (stream_info.as_ref(), &state_guard.playback_runtime_info)
        {
            let audio_settings = state_guard
                .db
                .with_conn(|conn| {
                    crate::db::audio_settings::load(conn).map_err(anyhow::Error::from)
                })
                .ok();
            if let Some(settings) = audio_settings
                && settings.sample_rate_follow
                && let Some(next_rate) = stream.sample_rate
            {
                let current_rate = info.sample_rate;
                let current_bit_depth = state_guard
                    .current_stream_display
                    .as_ref()
                    .and_then(|display| display.bit_depth);
                if should_skip_prebuffer_for_sample_rate_follow_format_change(
                    settings.exclusive_mode,
                    settings.sample_rate_follow,
                    current_rate,
                    Some(next_rate),
                    current_bit_depth,
                    stream.bit_depth,
                ) {
                    let current_depth_label = current_bit_depth
                        .map(|depth| depth.to_string())
                        .unwrap_or_else(|| "unknown".to_string());
                    let next_depth_label = stream
                        .bit_depth
                        .map(|depth| depth.to_string())
                        .unwrap_or_else(|| "unknown".to_string());
                    info!(
                        "Skipping pre-buffer for next track {}: sample-rate-follow will switch native format from {} Hz/{} bit to {} Hz/{} bit at track start",
                        next.id, current_rate, current_depth_label, next_rate, next_depth_label
                    );
                    return Ok(false);
                }
                if next_rate as u32 != current_rate {
                    let device_sel = match settings.output_device {
                        Some(device_id) => {
                            playback_runtime::OutputDeviceSelection::Named(device_id)
                        }
                        None => playback_runtime::OutputDeviceSelection::Default,
                    };
                    // StreamInfo.sample_rate is Option<i32>; cast is safe (fits in u32).
                    if let Err(e) = handle.device_swap(
                        device_sel,
                        settings.exclusive_mode,
                        settings.sample_rate_follow,
                        Some(next_rate as u32),
                        settings.exclusive_release_grace_secs,
                        settings.exclusive_latency_mode,
                    ) {
                        warn!(
                            "Failed to rebuild stream for next track {} at {} Hz: {e}",
                            next.id, next_rate
                        );
                    }
                }
            }
        }
    }

    let effective_crossfade = effective_crossfade_ms(&state, crossfade_ms).await;
    let _gapless = crate::playback::gapless::plan_from_stream(
        stream_info.as_ref(),
        crate::playback::gapless::GaplessSettings::new(true, effective_crossfade),
    );
    let job = player::build_playback_preparation(
        &next,
        stream_info.as_ref(),
        effective_crossfade,
        user_quality,
    )
    .with_generation(generation);
    let (job, lookahead_start) = {
        let state_guard = state.read().await;
        let channels = state_guard
            .playback_runtime_info
            .as_ref()
            .map(|info| info.channels)
            .unwrap_or(2);
        let pair = state_guard
            .db
            .with_conn(|conn| active_dj_pair_for_state_and_conn(&state_guard, conn))?;
        let engine = crate::playback::dj_engine::DjEngine::new(state_guard.db.clone());
        let job = player::attach_dj_transition_plan_for_pair(
            &engine,
            job,
            pair,
            stream_info
                .as_ref()
                .and_then(|info| info.sample_rate_hz())
                .unwrap_or(48_000),
            channels,
        )?;
        let lookahead_start = if job.prepared_transition.is_some() {
            active_dj_lookahead_start_for_state(&state_guard)
        } else {
            None
        };
        (job, lookahead_start)
    };

    {
        let state_guard = state.read().await;
        let active_id = state_guard
            .playback_runtime_info
            .as_ref()
            .and_then(|info| info.active_track_id);
        let db_current = state_guard
            .db
            .with_conn(player::current_track_id)
            .unwrap_or(None);
        if active_id != Some(current_track_id) || db_current != Some(current_track_id) {
            return Ok(false);
        }
        if current_playback_generation(&state_guard) != generation {
            return Ok(false);
        }
    }

    if job.prepared_transition.is_some()
        && let Some(start) = lookahead_start
    {
        let _ = start.dispatch(&handle);
        queue_missing_dj_profiles_after_pair_change(state.clone(), "prepared_next_transition")
            .await;
    }
    let _ = handle.prepare_next(job);
    if let Some(ref si) = stream_info {
        let mut state_guard = state.write().await;
        state_guard.pending_stream_display = Some(crate::StreamDisplayInfo {
            audio_quality: si.audio_quality.clone(),
            sample_rate: si.sample_rate,
            bit_depth: si.bit_depth,
        });
    }
    info!("Pre-buffering next track: {} (id {})", next.title, next.id);
    Ok(false)
}

pub(crate) fn claim_next_prebuffer_slot(
    slot: &mut Option<crate::NextPrebufferKey>,
    key: crate::NextPrebufferKey,
) -> bool {
    if *slot == Some(key) {
        return false;
    }
    *slot = Some(key);
    true
}

pub(crate) fn release_next_prebuffer_slot(
    slot: &mut Option<crate::NextPrebufferKey>,
    key: crate::NextPrebufferKey,
) {
    if *slot == Some(key) {
        *slot = None;
    }
}

/// The runtime acknowledged a transport change (its Pause/Resume handler ran,
/// or the advance-cascade breaker latched pause). Align the DB's `is_playing`
/// with that acknowledgment and, when it was out of sync, broadcast
/// `PlaybackStateChanged` so every client re-pulls authoritative state. The
/// runtime is the source of truth for what is audible; this is the
/// reconciliation channel that closes the "button says paused, audio still
/// playing" gap after command interleavings.
pub(crate) async fn reconcile_runtime_transport_state(state: &SharedState, runtime_playing: bool) {
    let state_guard = state.read().await;
    let db_playing = state_guard
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT is_playing FROM playback_state WHERE id = 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(Into::into)
        })
        .map(|value: i64| value != 0);
    let Ok(db_playing) = db_playing else {
        return;
    };
    if db_playing == runtime_playing {
        return;
    }
    let updated = state_guard.db.with_conn(|conn| {
        conn.execute(
            "UPDATE playback_state SET is_playing = ?1 WHERE id = 1",
            rusqlite::params![i64::from(runtime_playing)],
        )?;
        Ok(())
    });
    if updated.is_ok() {
        tracing::info!(
            target: "noor.playback",
            runtime_playing,
            "reconciled playback_state.is_playing to the runtime's transport acknowledgment"
        );
        let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    }
}

pub(crate) async fn switch_runtime_to_snapshot_current(
    state: &SharedState,
    snapshot: &player::PlaybackSnapshot,
    generation: u64,
) -> anyhow::Result<()> {
    // A track whose TIDAL asset won't resolve (pulled from the catalog, 4005
    // "asset not ready", or a hard rejection) used to wedge playback here: the
    // resolve error propagated up as fatal and the runtime sat frozen on a dead
    // row. Instead, skip past it and try the next queue item, bounded so a real
    // TIDAL outage doesn't chew silently through the whole queue.
    const MAX_UNPLAYABLE_SKIPS: u32 = 8;
    let mut snapshot = snapshot.clone();
    let mut unplayable_skips: u32 = 0;

    loop {
        let current_queue_item_id = snapshot.state.current_queue_item_id;
        let queue_len = snapshot.queue.len();

        let Some(track) = snapshot.state.current_track.clone() else {
            tracing::info!(
                target: "noor.playback.runtime",
                event = "runtime_snapshot_empty",
                generation,
                ?current_queue_item_id,
                queue_len,
                "snapshot has no current track; clearing runtime active track"
            );
            if unplayable_skips > 0 {
                sync_session_after_snapshot(
                    state,
                    &snapshot,
                    Some(player::ListenSessionEndReason::QueueEnded),
                )
                .await;
            }
            let mut state_guard = state.write().await;
            if let Some(info) = state_guard.playback_runtime_info.as_mut() {
                info.active_track_id = None;
            }
            let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
            let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
            return Ok(());
        };

        let user_quality = current_user_audio_quality(state).await;
        let runtime_handle = ensure_playback_runtime_for_track(state, &track)
            .await
            .map_err(|(status, body)| {
                anyhow::anyhow!("playback runtime unavailable ({status}): {}", body.0)
            })?;
        let prepared_status = runtime_handle.track_status(track.id, generation);
        if matches!(
            prepared_status,
            playback_runtime::PlaybackTrackStatus::Active
                | playback_runtime::PlaybackTrackStatus::Prepared
        ) {
            let job = player::build_playback_preparation(
                &track,
                None,
                effective_crossfade_ms(state, snapshot.state.crossfade_ms).await,
                user_quality,
            )
            .with_generation(generation)
            .with_start_paused(!transport_intent_is_playing(state).await);
            runtime_handle.switch_to(job).map_err(|error| {
                tracing::warn!(
                    target: "noor.playback.runtime",
                    event = "runtime_snapshot_switch_failed",
                    generation,
                    track_id = track.id,
                    ?current_queue_item_id,
                    queue_len,
                    runtime_track_status = ?prepared_status,
                    stream_resolved = false,
                    error = %error,
                    "failed to switch runtime to prepared snapshot current track"
                );
                error
            })?;
            {
                let mut state_guard = state.write().await;
                if let Some(info) = state_guard.playback_runtime_info.as_mut() {
                    info.active_track_id = Some(track.id);
                    info.last_error = None;
                }
                if let Some(pending) = state_guard.pending_stream_display.take() {
                    state_guard.current_stream_display = Some(pending);
                }
            }
            tracing::info!(
                target: "noor.playback.runtime",
                event = "runtime_snapshot_switch",
                generation,
                track_id = track.id,
                ?current_queue_item_id,
                queue_len,
                runtime_track_status = ?prepared_status,
                stream_resolved = false,
                "switched runtime to prepared snapshot current track"
            );
        } else {
            let crossfade_ms = effective_crossfade_ms(state, snapshot.state.crossfade_ms).await;
            match start_track(
                state,
                StartRequest {
                    track: &track,
                    generation,
                    dispatch: Dispatch::Switch,
                    crossfade_ms,
                },
            )
            .await
            {
                Ok(_) => {
                    let mut state_guard = state.write().await;
                    if let Some(info) = state_guard.playback_runtime_info.as_mut() {
                        info.active_track_id = Some(track.id);
                        info.last_error = None;
                    }
                }
                Err(StartError::Superseded) => return Ok(()),
                Err(StartError::LocalUnsupported) => {
                    handle_runtime_error(
                        state.clone(),
                        "Local library playback is not wired into the host audio runtime yet.",
                    )
                    .await;
                    return Ok(());
                }
                Err(StartError::Stream(err))
                    if err.is_track_unplayable() && unplayable_skips < MAX_UNPLAYABLE_SKIPS =>
                {
                    unplayable_skips += 1;
                    if let Some(tidal_id) = track.tidal_id {
                        dj_routes::record_unavailable_tidal_source(tidal_id);
                    }
                    let reason = if err.is_asset_not_ready() {
                        "Not available on TIDAL right now"
                    } else {
                        "TIDAL wouldn't play this track"
                    };
                    tracing::warn!(
                        target: "noor.playback.advance",
                        event = "skip_unplayable_track",
                        generation,
                        track_id = track.id,
                        skip = unplayable_skips,
                        error = %describe_tidal_playback_error(&err),
                        "skipping unplayable track and advancing to the next queue row"
                    );
                    emit_track_skipped(state, track.id, &track.title, reason).await;
                    if err.is_asset_not_ready() {
                        spawn_tidal_id_reresolve(state, track.id);
                    }
                    // Advance the persisted queue past the dead row and retry.
                    let cleared = {
                        let s = state.read().await;
                        recently_cleared(&s)
                    };
                    let advanced = {
                        let s = state.read().await;
                        s.db.with_conn(|conn| player::next_track(conn, cleared))
                    }?;
                    snapshot = resolve_or_skip_pending_current(
                        state,
                        advanced,
                        generation,
                        "skip_unplayable",
                    )
                    .await?;
                    if !playback_generation_is_current(state, generation).await {
                        return Ok(());
                    }
                    continue;
                }
                Err(StartError::Stream(err)) => {
                    return Err(anyhow::anyhow!(
                        "playback stream resolve failed: {}",
                        describe_tidal_playback_error(&err)
                    ));
                }
                Err(StartError::Runtime(error)) => {
                    return Err(anyhow::anyhow!("playback runtime unavailable: {error:?}"));
                }
                Err(StartError::Dispatch { error, .. }) => {
                    tracing::warn!(
                        target: "noor.playback.runtime",
                        event = "runtime_snapshot_switch_failed",
                        generation,
                        track_id = track.id,
                        ?current_queue_item_id,
                        queue_len,
                        runtime_track_status = ?prepared_status,
                        stream_resolved = true,
                        error = %error,
                        "failed to switch runtime after resolving snapshot stream"
                    );
                    return Err(error);
                }
            }
            tracing::info!(
                target: "noor.playback.runtime",
                event = "runtime_snapshot_switch",
                generation,
                track_id = track.id,
                ?current_queue_item_id,
                queue_len,
                runtime_track_status = ?prepared_status,
                stream_resolved = true,
                "switched runtime after resolving snapshot stream"
            );
        }

        // If we skipped past dead rows, the caller's pre-switch session sync was
        // for a track we never played. Re-anchor the listen session onto the row
        // that actually started so completion + transition learning is correct.
        if unplayable_skips > 0 {
            sync_session_after_snapshot(
                state,
                &snapshot,
                Some(player::ListenSessionEndReason::Replaced),
            )
            .await;
        }

        let state_guard = state.read().await;
        let _ = state_guard
            .event_tx
            .send(AppEvent::TrackChanged { track_id: track.id });
        let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
        let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
        return Ok(());
    }
}

/// Advance through persisted queue rows after a runtime track completes.
///
/// Skip-and-retry advance: a single TIDAL hiccup (especially a 429 rate-limit a
/// few tracks into a mix) used to nuke the entire remaining queue. 429 is
/// recoverable (sleep + retry the same track once); any other failure is
/// track-specific (skip to the next item). Only gives up when the continuation
/// is exhausted or MAX_CONSEC_FAILURES distinct tracks fail in a row.
pub(crate) async fn handle_runtime_finished(
    state: SharedState,
    finished_track_id: i64,
    generation: u64,
) -> anyhow::Result<()> {
    {
        let state_guard = state.read().await;
        let current_generation = current_playback_generation(&state_guard);
        if current_generation != generation {
            // Not necessarily a fault (a user skip legitimately bumps the
            // generation mid-flight), but it abandons an end-of-track advance,
            // and doing that silently is what made the freeze undiagnosable.
            tracing::warn!(
                target: "noor.playback.advance",
                event = "runtime_finished_stale_generation",
                finished_track_id,
                generation,
                current_generation,
                "end-of-track advance dropped: playback generation moved on; queue left unchanged"
            );
            return Ok(());
        }
    }
    tracing::info!(
        target: "noor.playback.advance",
        event = "runtime_finished",
        finished_track_id,
        generation,
        "runtime finished track; advancing queue"
    );
    if let Err(error) = mark_armed_dj_transition_missed_if_needed(&state).await {
        warn!("Failed to mark missed DJ transition timing: {error}");
    }

    let snapshot = {
        let state_guard = state.read().await;
        let cleared = recently_cleared(&state_guard);
        state_guard.db.with_conn(|conn| {
            let current_track_id = player::current_track_id(conn)?;
            let current_state = player::load_state(conn)?;
            if current_track_id != Some(finished_track_id) || !current_state.is_playing {
                tracing::warn!(
                    target: "noor.playback.advance",
                    event = "runtime_finished_state_mismatch",
                    finished_track_id,
                    generation,
                    db_current_track_id = ?current_track_id,
                    db_is_playing = current_state.is_playing,
                    "end-of-track advance dropped: persisted state no longer matches the finished track; queue left unchanged"
                );
                return Ok(None);
            }
            Ok(Some(player::next_track(conn, cleared)?))
        })?
    };

    let Some(snapshot) = snapshot else {
        let mut state_guard = state.write().await;
        let track_id_for_event = match flush_active_listen_session_locked(
            &mut state_guard,
            chrono::Utc::now(),
            player::ListenSessionEndReason::Stopped,
        ) {
            Ok(outcome) => outcome.flushed_track_id,
            Err(err) => {
                tracing::warn!("flush on runtime-finished mismatch failed: {err}");
                None
            }
        };
        if let Some(track_id) = track_id_for_event {
            let _ = state_guard
                .event_tx
                .send(AppEvent::ListenHistoryUpdated { track_id });
        }
        return Ok(());
    };

    let snapshot =
        resolve_or_skip_pending_current(&state, snapshot, generation, "runtime_finished").await?;
    if !playback_generation_is_current(&state, generation).await {
        tracing::warn!(
            target: "noor.playback.advance",
            event = "runtime_finished_stale_after_resolve",
            finished_track_id,
            generation,
            "end-of-track advance dropped after pending resolve: generation moved on"
        );
        return Ok(());
    }
    let end_reason = if snapshot.state.current_track.is_some() {
        Some(player::ListenSessionEndReason::Replaced)
    } else {
        Some(player::ListenSessionEndReason::QueueEnded)
    };
    sync_session_after_snapshot(&state, &snapshot, end_reason).await;
    switch_runtime_to_snapshot_current(&state, &snapshot, generation).await
}

pub(crate) async fn mark_armed_dj_transition_missed_if_needed(
    state: &SharedState,
) -> anyhow::Result<()> {
    let pair = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(|conn| active_dj_pair_for_state_and_conn(&state_guard, conn))?
    };
    let (Some(current), Some(next)) = (pair.current.as_ref(), pair.next.as_ref()) else {
        return Ok(());
    };
    let current_key = current.profile_key();
    let next_key = next.profile_key();
    let updated = {
        let state_guard = state.read().await;
        state_guard.db.with_conn(|conn| {
            queries::mark_dj_transition_timing_status_for_pair(
                conn,
                current_key.media_ref_kind.as_str(),
                current_key.media_ref_id.as_str(),
                next_key.media_ref_kind.as_str(),
                next_key.media_ref_id.as_str(),
                "missed",
            )
        })?
    };
    if updated > 0 {
        let state_guard = state.read().await;
        let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    }
    Ok(())
}

pub(crate) async fn mark_armed_dj_transition_manual_seek_suppressed_if_needed(
    state: &SharedState,
) -> anyhow::Result<()> {
    let pair = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(|conn| active_dj_pair_for_state_and_conn(&state_guard, conn))?
    };
    let (Some(current), Some(next)) = (pair.current.as_ref(), pair.next.as_ref()) else {
        return Ok(());
    };
    let current_key = current.profile_key();
    let next_key = next.profile_key();
    let updated = {
        let state_guard = state.read().await;
        state_guard.db.with_conn(|conn| {
            queries::mark_dj_transition_manual_seek_suppressed_for_pair(
                conn,
                current_key.media_ref_kind.as_str(),
                current_key.media_ref_id.as_str(),
                next_key.media_ref_kind.as_str(),
                next_key.media_ref_id.as_str(),
            )
        })?
    };
    if updated > 0 {
        let state_guard = state.read().await;
        let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    }
    Ok(())
}

pub(crate) async fn handle_runtime_finished_with_retry(
    state: SharedState,
    finished_track_id: i64,
    generation: u64,
) -> anyhow::Result<()> {
    for attempt in 0..=PLAYBACK_FINISH_DB_LOCK_RETRY_LIMIT {
        match handle_runtime_finished(state.clone(), finished_track_id, generation).await {
            Ok(()) => return Ok(()),
            Err(error)
                if sqlite_database_locked(&error)
                    && attempt < PLAYBACK_FINISH_DB_LOCK_RETRY_LIMIT =>
            {
                let next_attempt = attempt + 1;
                warn!(
                    finished_track_id,
                    generation, next_attempt, "Playback advance hit a locked database; retrying"
                );
                tokio::time::sleep(Duration::from_secs(
                    PLAYBACK_FINISH_DB_LOCK_RETRY_DELAY_SECS,
                ))
                .await;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(crate) fn runtime_track_error_is_retryable(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("dash stream prebuffer failed")
        || lower.contains("tidal stream download failed")
        || lower.contains("timed out")
        || lower.contains("request timeout")
        || lower.contains("too many requests")
        || lower.contains("backoff active")
        || lower.contains("error sending request")
}

pub(crate) async fn handle_runtime_track_error(
    state: SharedState,
    failed_track_id: i64,
    generation: u64,
    message: &str,
) -> anyhow::Result<()> {
    let retryable = runtime_track_error_is_retryable(message);
    let retry_already_attempted = {
        let mut state_guard = state.write().await;
        if current_playback_generation(&state_guard) != generation {
            return Ok(());
        }
        let retry_already_attempted = state_guard
            .playback_runtime_info
            .as_ref()
            .and_then(|info| info.last_error.as_deref())
            == Some(RUNTIME_TRACK_RETRY_MARKER);
        state_guard
            .audio_active
            .store(false, std::sync::atomic::Ordering::Relaxed);
        if let Some(info) = state_guard.playback_runtime_info.as_mut() {
            info.last_error = Some(if retryable && !retry_already_attempted {
                RUNTIME_TRACK_RETRY_MARKER.to_string()
            } else {
                message.to_string()
            });
            if info.active_track_id == Some(failed_track_id) {
                info.active_track_id = None;
            }
        }
        // Truthfulness: the active engine failed and its stream is gone, so the
        // now-playing "source" metadata (quality/rate/bit-depth) must not keep
        // advertising either the track that just failed (`pending`) or the prior
        // track that is no longer audible (`current`). Leaving a stale display
        // here is what let the sidebar show a ghost source (e.g. the previous
        // track) while the header claimed a different, non-playing track. The
        // next successful `Started` repopulates it from the real stream.
        state_guard.pending_stream_display = None;
        state_guard.current_stream_display = None;
        retry_already_attempted
    };

    if retryable && !retry_already_attempted {
        let snapshot = {
            let state_guard = state.read().await;
            state_guard.db.with_conn(player::load_snapshot)?
        };
        match switch_runtime_to_snapshot_current(&state, &snapshot, generation).await {
            Ok(()) => {
                tracing::warn!(
                    target: "noor.playback.recovery",
                    event = "runtime_track_retry_dispatched",
                    failed_track_id,
                    generation,
                    error = %message,
                    "retrying the current queue row after a transient runtime failure"
                );
                return Ok(());
            }
            Err(error) => {
                tracing::warn!(
                    target: "noor.playback.recovery",
                    event = "runtime_track_retry_failed",
                    failed_track_id,
                    generation,
                    original_error = %message,
                    retry_error = %error,
                    "current-row playback recovery failed; pausing without advancing"
                );
            }
        }
    }

    if retryable {
        let paused_snapshot = {
            let state_guard = state.read().await;
            state_guard.db.with_conn(player::pause)?
        };
        sync_session_after_snapshot(
            &state,
            &paused_snapshot,
            Some(player::ListenSessionEndReason::Stopped),
        )
        .await;
        {
            let state_guard = state.read().await;
            let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
        }
        tracing::warn!(
            target: "noor.playback.recovery",
            event = "runtime_track_retry_exhausted",
            failed_track_id,
            generation,
            error = %message,
            "transient playback recovery was exhausted; paused without advancing the queue"
        );
        report_playback_failure(&state, message);
        return Ok(());
    }

    tracing::warn!(
        target: "noor.playback.advance",
        event = "runtime_track_error",
        failed_track_id,
        generation,
        error = %message,
        "runtime track error; advancing queue"
    );
    report_playback_failure(&state, message);
    handle_runtime_finished_with_retry(state, failed_track_id, generation).await
}

pub(crate) async fn handle_prepared_runtime_track_error_for_runtime(
    state: &SharedState,
    expected_runtime: Option<&playback_runtime::PlaybackRuntimeHandle>,
    track_id: i64,
    generation: u64,
    failed_tidal_id: Option<i64>,
    message: &str,
) {
    tracing::warn!(
        target: "noor.playback.advance",
        event = "prepared_track_error",
        track_id,
        generation,
        tidal_id = failed_tidal_id,
        error = %message,
        "prepared track failed; keeping current playback"
    );
    let skipped = {
        let mut state_guard = state.write().await;
        if current_playback_generation(&state_guard) != generation
            || expected_runtime.is_some_and(|expected| {
                !state_guard
                    .playback_runtime
                    .as_ref()
                    .is_some_and(|runtime| runtime.handle.is_same_runtime(expected))
            })
        {
            return;
        }
        // Only explicit TIDAL 4005 is a catalog failure here. The exact source
        // and playback generation travel with the event, so a delayed failure
        // cannot discard a freshly healed or re-prepared next track.
        let rejected = tidal_stream::StreamResolveError::StreamRejected {
            message: message.to_string(),
        };
        let mut skipped = false;
        if rejected.is_asset_not_ready() {
            let current = state_guard
                .playback_runtime_info
                .as_ref()
                .and_then(|info| info.active_track_id);
            let captured = state_guard.db.with_conn(|conn| {
                let next = player::peek_next_track(conn, recently_cleared(&state_guard))?;
                let pair = crate::playback::dj_lookahead::load_dj_lookahead_pair(conn)?;
                Ok((pair, next))
            });
            if let (Some(current), Ok((pair, Some(next)))) = (current, captured)
                && next.id == track_id
                && let Some(tidal_id) = next.tidal_id
            {
                if failed_tidal_id != Some(tidal_id) {
                    return;
                }
                match remove_unavailable_upcoming_row(
                    &mut state_guard,
                    current,
                    generation,
                    &pair,
                    &next,
                    message,
                ) {
                    Ok(true) => {
                        dj_routes::record_unavailable_tidal_source(tidal_id);
                        skipped = true;
                    }
                    Ok(false) => {}
                    Err(error) => warn!("Failed to skip unavailable prepared next row: {error}"),
                }
            }
        }
        if let Some(info) = state_guard.playback_runtime_info.as_mut() {
            info.last_error = Some(message.to_string());
        }
        let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
        skipped
    };
    if skipped {
        spawn_tidal_id_reresolve(state, track_id);
        refresh_dj_after_queue_change(state.clone(), "skip_unavailable_prepared_next").await;
    }
    report_playback_failure(state, message);
}

pub(crate) async fn handle_runtime_error(state: SharedState, message: &str) {
    {
        let mut state_guard = state.write().await;
        if let Some(info) = state_guard.playback_runtime_info.as_mut() {
            info.last_error = Some(message.to_string());
            info.active_track_id = None;
        }
    }
    report_playback_failure(&state, message);

    let snapshot = {
        let state_guard = state.read().await;
        state_guard.db.with_conn(player::pause).ok()
    };

    if let Some(snapshot) = snapshot {
        sync_session_after_snapshot(
            &state,
            &snapshot,
            Some(player::ListenSessionEndReason::Stopped),
        )
        .await;
    }

    let state_guard = state.read().await;
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
}

pub(crate) async fn handle_runtime_exit(
    state: &SharedState,
    exited_handle: &playback_runtime::PlaybackRuntimeHandle,
    message: Option<&str>,
) {
    let removed_current_runtime = {
        let mut state_guard = state.write().await;
        let is_current = state_guard
            .playback_runtime
            .as_ref()
            .is_some_and(|runtime| runtime.handle.is_same_runtime(exited_handle));
        if !is_current {
            false
        } else {
            state_guard.playback_runtime = None;
            state_guard.playback_runtime_info = None;
            state_guard
                .audio_active
                .store(false, std::sync::atomic::Ordering::Release);
            state_guard.current_stream_display = None;
            state_guard.pending_stream_display = None;
            state_guard.next_prebuffer_inflight = None;
            true
        }
    };
    if !removed_current_runtime {
        return;
    }

    let user_message = match message {
        Some(detail) => format!("Playback runtime stopped and will restart on resume: {detail}"),
        None => "Playback runtime stopped and will restart on resume.".to_string(),
    };
    tracing::error!(
        target: "noor.playback.recovery",
        event = "runtime_exited",
        error = message.unwrap_or("runtime command loop closed"),
        "discarded dead playback runtime handle"
    );
    report_playback_failure(state, &user_message);

    let snapshot = {
        let state_guard = state.read().await;
        state_guard.db.with_conn(player::pause).ok()
    };
    if let Some(snapshot) = snapshot {
        sync_session_after_snapshot(
            state,
            &snapshot,
            Some(player::ListenSessionEndReason::Stopped),
        )
        .await;
    }
    let state_guard = state.read().await;
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
}

/// Broadcast a "we skipped this track" notice so the UI can toast which track
/// dropped out and why, instead of freezing on a silent dead row.
pub(crate) async fn emit_track_skipped(
    state: &SharedState,
    track_id: i64,
    title: &str,
    reason: &str,
) {
    let state_guard = state.read().await;
    let _ = state_guard.event_tx.send(AppEvent::TrackSkipped {
        track_id,
        title: title.to_string(),
        reason: reason.to_string(),
    });
}

pub(crate) fn report_playback_failure(state: &SharedState, message: &str) {
    let state = state.clone();
    let message = message.to_string();
    tokio::spawn(async move {
        let state = state.read().await;
        let _ = state.event_tx.send(AppEvent::PlaybackFailed { message });
    });
}

pub(crate) fn describe_tidal_playback_error(error: &TidalPlaybackError) -> String {
    match error {
        TidalPlaybackError::NotConnected => "TIDAL is not connected.".to_string(),
        TidalPlaybackError::SessionRefreshFailed(message) => message.clone(),
        TidalPlaybackError::StreamResolve(error) => error.to_string(),
    }
}
