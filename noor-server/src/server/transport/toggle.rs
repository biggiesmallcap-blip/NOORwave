//! Transport toggles that do not start a new playback job: pause, resume,
//! seek, and releasing the exclusive device. None of these bump the playback
//! generation (see `pause`).

use crate::playback::player;
use crate::playback::runtime as playback_runtime;
use crate::{AppEvent, SharedState};

use super::events::{
    mark_armed_dj_transition_manual_seek_suppressed_if_needed, report_playback_failure,
    switch_runtime_to_snapshot_current,
};
use super::generation;
use super::listen::{resume_session_after_snapshot, sync_session_after_snapshot};
use super::runtime::{RuntimeUnavailable, current as current_runtime, ensure_for_track};
use super::snapshot::{
    build_live_playback_snapshot, current_live_position_ms, overlay_snapshot_with_external_track,
    overlay_snapshot_with_external_track_and_position,
};

#[derive(Debug)]
pub(crate) enum ToggleError {
    /// Persisting the transport state or talking to the runtime failed.
    Internal,
    /// Resume needed a runtime and none could be had.
    Runtime(RuntimeUnavailable),
    /// Resume rebuilt the runtime but could not restore the current row.
    RecoveryFailed,
}

/// The user-facing message for a runtime that could not be acquired.
pub(crate) fn runtime_unavailable_message(error: &RuntimeUnavailable) -> String {
    match error {
        RuntimeUnavailable::NotConnected => "Connect TIDAL in Settings before playing.".to_string(),
        RuntimeUnavailable::SpawnFailed(message) => message.clone(),
        RuntimeUnavailable::Missing => {
            "Playback runtime was not available after initialization.".to_string()
        }
    }
}

fn emit_state_changed(state: &crate::AppState) {
    let _ = state.event_tx.send(AppEvent::PlaybackStateChanged);
}

pub(crate) async fn pause(state: &SharedState) -> Result<player::PlaybackSnapshot, ToggleError> {
    // Deliberately does NOT bump the playback generation. The generation
    // identifies WHICH playback job is current; a transport toggle does not
    // start one, so bumping here orphaned the engine that is still loaded:
    // it keeps the generation it was created with, and every generation-guarded
    // path then rejected its events for the rest of the track. That silently
    // broke the end-of-track queue advance (the track played to its final
    // sample and froze), prepare-next/gapless, the Started event that sets
    // `audio_active`, and track-error recovery, until the user hit Next.
    //
    // The race this was reaching for -- an in-flight resolve starting audio
    // after the user paused -- is already handled by the play/switch paths
    // through `with_start_paused(!transport_intent_is_playing(..))`, which
    // reads the `is_playing` intent that `player::pause` writes below.
    if let Some(runtime_handle) = current_runtime(state).await
        && let Err(error) = runtime_handle.pause()
    {
        let message = format!("Failed to pause host audio playback: {error}");
        report_playback_failure(state, &message);
        return Err(ToggleError::Internal);
    }

    // Opt-in: free the exclusive WASAPI device on an explicit pause so other
    // apps can take the DAC without waiting out the idle-release grace. No-op
    // when exclusive mode is off (the runtime guards on current_exclusive) or
    // the setting is disabled. Re-grabbed automatically on the next Resume/Play.
    let release_on_pause = {
        let guard = state.read().await;
        guard
            .db
            .with_conn(|conn| crate::db::audio_settings::load(conn).map_err(Into::into))
            .map(|s| s.exclusive_release_on_pause)
            .unwrap_or(false)
    };
    if release_on_pause && let Some(runtime_handle) = current_runtime(state).await {
        let _ = runtime_handle.release_exclusive_now();
    }

    let snapshot = {
        let state = state.read().await;
        state
            .db
            .with_conn(player::pause)
            .map_err(|_| ToggleError::Internal)?
    };

    // Flush the in-progress session to listen_history on pause so analytics
    // shows partial listens without waiting for the next track-change. The
    // snapshot has is_playing=false, so sync_session_after_snapshot won't
    // start a new session. resume_session_after_snapshot will reopen one
    // (reusing the same session_id if the gap is < 30 min).
    sync_session_after_snapshot(
        state,
        &snapshot,
        Some(player::ListenSessionEndReason::Stopped),
    )
    .await;

    emit_state_changed(&*state.read().await);
    let live_position_ms = current_live_position_ms(state).await;
    Ok(overlay_snapshot_with_external_track_and_position(state, snapshot, live_position_ms).await)
}

pub(crate) async fn release_exclusive(state: &SharedState) -> Result<(), ToggleError> {
    if let Some(runtime_handle) = current_runtime(state).await
        && let Err(error) = runtime_handle.release_exclusive_now()
    {
        tracing::warn!(
            target = "noor.playback",
            event = "exclusive_release_failed",
            "Failed to request exclusive release: {error}"
        );
        return Err(ToggleError::Internal);
    }
    Ok(())
}

pub(crate) async fn resume(state: &SharedState) -> Result<player::PlaybackSnapshot, ToggleError> {
    // No generation bump, for the same reason as `pause`: resuming does not
    // start a new playback job, and bumping here left the engine that is about
    // to keep playing stranded on a stale generation.
    let (runtime_handle, runtime_active_track_id, persisted_track_id) = {
        let state_guard = state.read().await;
        let runtime_handle = state_guard
            .playback_runtime
            .as_ref()
            .map(|runtime| runtime.handle.clone())
            .filter(playback_runtime::PlaybackRuntimeHandle::is_healthy);
        let runtime_active_track_id = state_guard
            .playback_runtime_info
            .as_ref()
            .and_then(|info| info.active_track_id);
        let persisted_track_id = state_guard
            .db
            .with_conn(player::current_track_id)
            .unwrap_or(None);
        (runtime_handle, runtime_active_track_id, persisted_track_id)
    };
    let runtime_needs_rebuild = if runtime_active_track_id != persisted_track_id {
        true
    } else {
        match runtime_handle {
            Some(runtime_handle) => match runtime_handle.resume() {
                Ok(()) => false,
                Err(error) => {
                    tracing::warn!(
                        target: "noor.playback.recovery",
                        event = "resume_dead_runtime",
                        error = %error,
                        "resume found a closed runtime command channel; rebuilding"
                    );
                    true
                }
            },
            None => true,
        }
    };

    let snapshot = {
        let state = state.read().await;
        state
            .db
            .with_conn(player::resume)
            .map_err(|_| ToggleError::Internal)?
    };

    if runtime_needs_rebuild {
        let Some(track) = snapshot.state.current_track.clone() else {
            let snapshot = {
                let state_guard = state.read().await;
                state_guard
                    .db
                    .with_conn(player::pause)
                    .map_err(|_| ToggleError::Internal)?
            };
            emit_state_changed(&*state.read().await);
            return Ok(overlay_snapshot_with_external_track(state, snapshot).await);
        };

        if let Err(error) = ensure_for_track(state).await {
            let message = runtime_unavailable_message(&error);
            pause_after_failed_resume(state).await;
            report_playback_failure(state, &message);
            return Err(ToggleError::Runtime(error));
        }
        let generation = generation::current(&*state.read().await);
        if let Err(error) = switch_runtime_to_snapshot_current(state, &snapshot, generation).await {
            let message = format!("Playback runtime could not recover on resume: {error}");
            pause_after_failed_resume(state).await;
            report_playback_failure(state, &message);
            return Err(ToggleError::RecoveryFailed);
        }
        tracing::info!(
            target: "noor.playback.recovery",
            event = "runtime_rebuilt_on_resume",
            track_id = track.id,
            generation,
            "restored the current queue row in the playback runtime"
        );
    }

    resume_session_after_snapshot(state, &snapshot).await;
    emit_state_changed(&*state.read().await);
    let live_position_ms = current_live_position_ms(state).await;
    Ok(overlay_snapshot_with_external_track_and_position(state, snapshot, live_position_ms).await)
}

async fn pause_after_failed_resume(state: &SharedState) {
    let state_guard = state.read().await;
    let _ = state_guard.db.with_conn(player::pause);
    emit_state_changed(&state_guard);
}

/// What the runtime did with a seek.
pub(crate) enum SeekResult {
    Accepted(Box<player::PlaybackSnapshot>),
    /// Outside the buffered range and no segment seek allowed (or no runtime).
    Rejected(Box<player::PlaybackSnapshot>),
}

pub(crate) async fn seek(
    state: &SharedState,
    position_ms: i64,
    allow_segment_seek: bool,
) -> Result<SeekResult, ToggleError> {
    // The runtime's SeekTo handler decides in-buffer / segment-restart /
    // reject. No runtime active (pre-first-play boot, or runtime crashed):
    // treat as rejected with the current snapshot; a seek with no runtime is a
    // UI race not worth failing over.
    let handle = {
        let g = state.read().await;
        g.playback_runtime.as_ref().map(|rt| rt.handle.clone())
    };
    let outcome = match handle {
        Some(handle) => {
            // `recv_timeout` inside seek_to_segment_aware blocks; run it on a
            // blocking pool so it doesn't park an async executor thread.
            tokio::task::spawn_blocking(move || {
                handle.seek_to_segment_aware(position_ms, allow_segment_seek)
            })
            .await
            .map_err(|_| ToggleError::Internal)?
        }
        None => playback_runtime::SeekToOutcome::RejectedOutOfBuffer,
    };

    // A fired event remains part of listening history, but an accepted seek
    // means its rendered overlap is no longer the live visual association.
    if matches!(
        outcome,
        playback_runtime::SeekToOutcome::Dispatched
            | playback_runtime::SeekToOutcome::DispatchedCrossfadeSuppressed
    ) && let Some(session) = state.write().await.active_listen_session.as_mut()
    {
        session.transition_visual_valid = false;
    }

    let snapshot = build_live_playback_snapshot(state)
        .await
        .map_err(|_| ToggleError::Internal)?;
    emit_state_changed(&*state.read().await);

    match outcome {
        playback_runtime::SeekToOutcome::DispatchedCrossfadeSuppressed => {
            if let Err(error) =
                mark_armed_dj_transition_manual_seek_suppressed_if_needed(state).await
            {
                tracing::warn!("Failed to suppress armed DJ transition after seek: {error}");
            }
            Ok(SeekResult::Accepted(Box::new(snapshot)))
        }
        playback_runtime::SeekToOutcome::Dispatched => Ok(SeekResult::Accepted(Box::new(snapshot))),
        playback_runtime::SeekToOutcome::RejectedOutOfBuffer => {
            Ok(SeekResult::Rejected(Box::new(snapshot)))
        }
        playback_runtime::SeekToOutcome::Failed => Err(ToggleError::Internal),
    }
}
