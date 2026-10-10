//! User transport commands (next, previous, play, play queue item). Each
//! returns a typed outcome; HTTP handlers only translate it to JSON.

use crate::playback::history::PlayHistoryEntry;
use crate::playback::{automix, player, queue};
use crate::{AppEvent, SharedState};
use rusqlite::OptionalExtension;

use super::events::{report_playback_failure, switch_runtime_to_snapshot_current};
use super::generation;
use super::listen::{record_transition_if_changed, sync_session_after_snapshot};
use super::pending::{resolve_or_skip_pending_current, resolve_or_skip_pending_current_previous};
use super::runtime::current as current_runtime;
use super::settings::{current_crossfade_ms, effective_crossfade_ms};
use super::snapshot::{
    build_live_playback_snapshot, current_live_position_ms, current_playback_track_id,
    overlay_snapshot_with_external_track, recently_cleared, restore_after_previous_failure,
    save_playback_anchor,
};
use super::start::{Dispatch, StartError, StartRequest, start_track};

/// What a successful command settled on.
#[derive(Debug)]
pub(crate) enum Outcome {
    /// This snapshot is the result; respond with it as is.
    Settled(Box<player::PlaybackSnapshot>),
    /// A newer command took over, or a helper already settled playback:
    /// respond with whatever the current persisted snapshot is.
    Current,
}

/// Why a command failed. Each variant maps to one fixed HTTP response.
#[derive(Debug)]
pub(crate) enum CommandError {
    /// Updating the persisted playback state failed; the message is user-facing.
    StateUpdate(&'static str),
    /// Starting the chosen track failed.
    Start {
        error: StartError,
        track_id: i64,
        stream_context: &'static str,
        runtime_message: Option<&'static str>,
    },
    /// Skipping past an unplayable track failed (already reported over WS).
    UnplayableAdvance(String),
    /// play: the id is not a positive library track id.
    InvalidTrackId(i64),
    /// play: loading the track failed.
    TrackLookupFailed(i64),
    /// play: no such track.
    TrackNotFound(i64),
    /// play: persisting the new current track failed.
    PlaybackStartFailed(i64),
    /// play queue item: no such queue row.
    QueueItemNotFound,
    /// The live snapshot could not be loaded after the command ran.
    SnapshotUnavailable,
}

/// Hand a dead asset to the skip-aware snapshot switch, which advances past it
/// (and any further dead rows) and starts the next playable track.
async fn skip_unplayable(
    state: &SharedState,
    snapshot: &player::PlaybackSnapshot,
    playback_generation: u64,
) -> Result<Outcome, CommandError> {
    switch_runtime_to_snapshot_current(state, snapshot, playback_generation)
        .await
        .map_err(|error| {
            let message = format!("Failed to advance past an unplayable track: {error}");
            report_playback_failure(state, &message);
            CommandError::UnplayableAdvance(message)
        })?;
    Ok(Outcome::Current)
}

fn emit_track_and_queue_events(state: &crate::AppState, current_track_id: Option<i64>) {
    if let Some(track_id) = current_track_id {
        let _ = state.event_tx.send(AppEvent::TrackChanged { track_id });
    }
    let _ = state.event_tx.send(AppEvent::PlaybackStateChanged);
    let _ = state.event_tx.send(AppEvent::QueueUpdated);
}

/// Advance to the next queue row and start it.
pub(crate) async fn next(state: &SharedState) -> Result<Outcome, CommandError> {
    const FAILED: &str = "Failed to advance playback state.";
    let playback_generation = generation::bump(state).await;
    let previous_track_id = current_playback_track_id(state).await;
    let snapshot = {
        let state = state.read().await;
        let cleared = recently_cleared(&state);
        state
            .db
            .with_conn(|conn| player::next_track(conn, cleared))
            .map_err(|_| CommandError::StateUpdate(FAILED))?
    };

    let snapshot =
        resolve_or_skip_pending_current(state, snapshot, playback_generation, "manual_next_track")
            .await
            .map_err(|error| {
                tracing::error!(
                    target: "noor.playback.advance",
                    event = "manual_next_pending_advance_failed",
                    error = %error,
                    "failed to resolve or skip pending row while advancing playback"
                );
                CommandError::StateUpdate(FAILED)
            })?;

    if !generation::is_current(state, playback_generation).await {
        return Ok(Outcome::Current);
    }

    record_transition_if_changed(state, previous_track_id, &snapshot, "queue", true).await;

    let end_reason = if snapshot.state.current_track.is_some() {
        Some(player::ListenSessionEndReason::Replaced)
    } else {
        Some(player::ListenSessionEndReason::QueueEnded)
    };
    sync_session_after_snapshot(state, &snapshot, end_reason).await;

    if let Some(track) = snapshot.state.current_track.as_ref() {
        let crossfade_ms = effective_crossfade_ms(state, snapshot.state.crossfade_ms).await;
        match start_track(
            state,
            StartRequest {
                track,
                generation: playback_generation,
                dispatch: Dispatch::Switch,
                crossfade_ms,
            },
        )
        .await
        {
            Ok(_) => {}
            Err(StartError::Superseded) => return Ok(Outcome::Current),
            Err(StartError::Stream(error)) if error.is_track_unplayable() => {
                return skip_unplayable(state, &snapshot, playback_generation).await;
            }
            Err(error) => {
                return Err(CommandError::Start {
                    error,
                    track_id: track.id,
                    stream_context: "TIDAL stream could not be resolved while advancing playback.",
                    runtime_message: Some(
                        "Playback runtime was not available for advancing playback.",
                    ),
                });
            }
        }
    } else if let Some(runtime_handle) = current_runtime(state).await {
        let _ = runtime_handle.stop();
    }

    emit_track_and_queue_events(
        &*state.read().await,
        snapshot.state.current_track.as_ref().map(|t| t.id),
    );
    Ok(Outcome::Settled(Box::new(snapshot)))
}

/// Shared "make the anchored queue row audible" tail: resolve (or skip) a
/// pending current row, start it, sync the listen session, and emit events.
/// Callers position the anchor first (start of queue, or an explicit row).
async fn start_current_queue_item(
    state: &SharedState,
    snapshot: player::PlaybackSnapshot,
    playback_generation: u64,
    previous_track_id: Option<i64>,
    context: &'static str,
    transition_source: &'static str,
) -> Result<player::PlaybackSnapshot, CommandError> {
    let snapshot = resolve_or_skip_pending_current(state, snapshot, playback_generation, context)
        .await
        .map_err(|error| {
            tracing::error!(
                target: "noor.playback.advance",
                event = "queue_start_pending_advance_failed",
                context,
                error = %error,
                "failed to resolve or skip the current queue item"
            );
            CommandError::StateUpdate("Failed to start the queue.")
        })?;

    let end_reason = if snapshot.state.current_track.is_some() {
        Some(player::ListenSessionEndReason::Replaced)
    } else {
        Some(player::ListenSessionEndReason::QueueEnded)
    };
    sync_session_after_snapshot(state, &snapshot, end_reason).await;

    if let Some(track) = snapshot.state.current_track.as_ref() {
        let crossfade_ms = effective_crossfade_ms(state, snapshot.state.crossfade_ms).await;
        match start_track(
            state,
            StartRequest {
                track,
                generation: playback_generation,
                dispatch: Dispatch::Play,
                crossfade_ms,
            },
        )
        .await
        {
            Ok(_) => {}
            Err(StartError::Superseded) => {
                return Ok(overlay_snapshot_with_external_track(state, snapshot).await);
            }
            Err(error) => {
                let paused_snapshot = {
                    let state_guard = state.read().await;
                    state_guard.db.with_conn(player::pause).ok()
                };
                // sync_session_after_snapshot above already opened a session
                // for this (local) track; flush+drop it so we don't bill a
                // bogus multi-minute listen the next time the user plays.
                if matches!(error, StartError::LocalUnsupported)
                    && let Some(snap) = paused_snapshot
                {
                    sync_session_after_snapshot(
                        state,
                        &snap,
                        Some(player::ListenSessionEndReason::Stopped),
                    )
                    .await;
                }
                return Err(CommandError::Start {
                    error,
                    track_id: track.id,
                    stream_context: "TIDAL stream could not be resolved while starting radio.",
                    runtime_message: Some("Playback runtime was not available for starting radio."),
                });
            }
        }
    } else if let Some(runtime_handle) = current_runtime(state).await {
        let _ = runtime_handle.stop();
    }

    record_transition_if_changed(state, previous_track_id, &snapshot, transition_source, true)
        .await;
    emit_track_and_queue_events(
        &*state.read().await,
        snapshot.state.current_track.as_ref().map(|t| t.id),
    );
    Ok(overlay_snapshot_with_external_track(state, snapshot).await)
}

/// Start the queue from its first row (radio starts use this).
pub(crate) async fn start_queue_from_beginning(
    state: &SharedState,
) -> Result<player::PlaybackSnapshot, CommandError> {
    let playback_generation = generation::bump(state).await;
    let previous_track_id = current_playback_track_id(state).await;
    let snapshot = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(|conn| player::start_queue_from_beginning(conn, false))
            .map_err(|_| CommandError::StateUpdate("Failed to start the radio queue."))?
    };
    start_current_queue_item(
        state,
        snapshot,
        playback_generation,
        previous_track_id,
        "start_first_radio_queue_item",
        "radio",
    )
    .await
}

/// Jump playback to a specific queue row (library or pending) and start it.
/// Pending rows resolve (import + promote) on the way in, and unlike
/// play-by-track-id this cannot land on the wrong row when the same track
/// appears twice in the queue.
pub(crate) async fn play_queue_item(
    state: &SharedState,
    queue_item_id: i64,
) -> Result<Outcome, CommandError> {
    let playback_generation = generation::bump(state).await;
    let previous_track_id = current_playback_track_id(state).await;
    let snapshot = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(|conn| player::play_queue_item_anchor(conn, queue_item_id))
            .map_err(|_| CommandError::StateUpdate("Failed to jump to that queue item."))?
    };
    let Some(snapshot) = snapshot else {
        return Err(CommandError::QueueItemNotFound);
    };
    let snapshot = start_current_queue_item(
        state,
        snapshot,
        playback_generation,
        previous_track_id,
        "play_queue_item",
        "queue",
    )
    .await?;
    crate::server::routes::refresh_dj_after_queue_change(state.clone(), "play_queue_item").await;
    Ok(Outcome::Settled(Box::new(snapshot)))
}

/// Play one persisted library track now (pending TIDAL rows are played by
/// their queue item id instead).
pub(crate) async fn play(state: &SharedState, track_id: i64) -> Result<Outcome, CommandError> {
    if track_id <= 0 {
        return Err(CommandError::InvalidTrackId(track_id));
    }

    let previous_track_id = current_playback_track_id(state).await;
    let playback_generation = generation::bump(state).await;
    // User-driven play; reset the post-clear suppression so automix
    // re-engages naturally instead of waiting out the 60s window.
    {
        let g = state.read().await;
        g.user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }
    let track = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(|conn| queue::get_track_by_id(conn, track_id))
            .map_err(|error| {
                tracing::error!(
                    target: "noor.playback.tidal",
                    event = "playback_track_lookup_failed",
                    track_id,
                    error = %error,
                    "failed to load track before playback"
                );
                CommandError::TrackLookupFailed(track_id)
            })?
            .ok_or(CommandError::TrackNotFound(track_id))?
    };

    tracing::info!(
        target: "noor.playback.tidal",
        event = "playback_start_requested",
        track_id = track.id,
        source = %player::playback_source_kind(&track),
        "playback start requested"
    );

    let snapshot = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(|conn| player::play_track_now(conn, track_id))
            .map_err(|error| {
                tracing::error!(
                    target: "noor.playback.tidal",
                    event = "playback_start_failed",
                    track_id,
                    error = %error,
                    "failed to start playback"
                );
                CommandError::PlaybackStartFailed(track_id)
            })?
    };

    let crossfade_ms = current_crossfade_ms(state).await;
    let started = match start_track(
        state,
        StartRequest {
            track: &track,
            generation: playback_generation,
            dispatch: Dispatch::Play,
            crossfade_ms,
        },
    )
    .await
    {
        Ok(started) => started,
        Err(StartError::Superseded) => return Ok(Outcome::Current),
        Err(StartError::Stream(error)) if error.is_track_unplayable() => {
            // The track the user picked is a dead TIDAL asset: skip past it
            // instead of failing the whole action.
            return skip_unplayable(state, &snapshot, playback_generation).await;
        }
        Err(error) => {
            let paused_snapshot = {
                let state_guard = state.read().await;
                state_guard.db.with_conn(player::pause).ok()
            };
            // Flush the prior TIDAL session before bailing on a local track,
            // otherwise the active session keeps accumulating against the
            // still-playing previous track and the next successful play
            // records a bogus multi-hour listen.
            if matches!(error, StartError::LocalUnsupported)
                && let Some(snap) = paused_snapshot
            {
                sync_session_after_snapshot(
                    state,
                    &snap,
                    Some(player::ListenSessionEndReason::Stopped),
                )
                .await;
            }
            return Err(CommandError::Start {
                error,
                track_id: track.id,
                stream_context: "TIDAL stream could not be resolved before playback.",
                runtime_message: None,
            });
        }
    };
    tracing::info!(
        target: "noor.playback.tidal",
        event = "playback_stream_ready",
        track_id = track.id,
        "TIDAL stream resolved before playback start"
    );
    report_play_started(state, &track, &started.stream_info.audio_quality).await;
    record_transition_if_changed(state, previous_track_id, &snapshot, "user", false).await;
    sync_session_after_snapshot(
        state,
        &snapshot,
        Some(player::ListenSessionEndReason::Replaced),
    )
    .await;

    // If automix is enabled, fill the queue in the background now that the
    // new current track is committed to DB. Doing this here (rather than at
    // automix-enable time) ensures the fill uses the correct track context and
    // doesn't race with this play's DB operation.
    if snapshot.state.automix_enabled {
        let (bg_db, bg_tx) = {
            let g = state.read().await;
            (g.db.clone(), g.event_tx.clone())
        };
        tokio::spawn(async move {
            // `user_cleared_at` was reset above, so the suppression window
            // cannot apply to this user-driven fill.
            let result = bg_db.with_conn(|conn| {
                automix::ensure_automix_queue_depth(conn, automix::AUTOMIX_MIN_UPCOMING, false)
            });
            if result.is_ok() {
                let _ = bg_tx.send(AppEvent::QueueUpdated);
            }
        });
    }

    {
        let state_guard = state.read().await;
        let _ = state_guard
            .event_tx
            .send(AppEvent::TrackChanged { track_id });
        let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    }
    Ok(Outcome::Settled(Box::new(snapshot)))
}

/// Fire-and-forget TIDAL play event: session health + artist attribution.
async fn report_play_started(
    state: &SharedState,
    track: &crate::db::models::Track,
    audio_quality: &str,
) {
    let Some(tidal_id) = track.tidal_id else {
        return;
    };
    let (http, token) = {
        let g = state.read().await;
        (
            g.http_client.clone(),
            g.tidal.tokens().map(|t| t.access_token),
        )
    };
    let Some(token) = token else {
        return;
    };
    let quality = audio_quality.to_string();
    let duration_ms = track.duration_ms.unwrap_or(0);
    tokio::spawn(async move {
        if let Err(e) = crate::services::tidal::play_reporter::report_play(
            &http,
            &token,
            tidal_id,
            &quality,
            duration_ms,
        )
        .await
        {
            tracing::warn!("play report failed: {e}");
        }
    });
}

/// Restart whatever is audibly playing from the top via a segment-aware
/// runtime seek: no stream re-resolve, no engine cold start. Works for
/// persisted queue playback and preserves pause state. While paused the audio
/// callback does not consume the seek until resume, so the reported position
/// may hold its old value until then.
async fn restart_current_in_place(state: &SharedState) -> Result<Outcome, CommandError> {
    let handle = {
        let state_guard = state.read().await;
        state_guard
            .playback_runtime
            .as_ref()
            .map(|rt| rt.handle.clone())
    };
    if let Some(handle) = handle {
        // Segment-aware restart to 0, same path the seek route uses.
        let _ = tokio::task::spawn_blocking(move || handle.seek_to_segment_aware(0, true)).await;
    }
    {
        let state_guard = state.read().await;
        let _ = state_guard.db.with_conn(|conn| {
            conn.execute("UPDATE playback_state SET position_ms = 0 WHERE id = 1", [])?;
            Ok::<_, anyhow::Error>(())
        });
    }

    let snapshot = build_live_playback_snapshot(state)
        .await
        .map_err(|_| CommandError::SnapshotUnavailable)?;
    {
        let state_guard = state.read().await;
        let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    }
    Ok(Outcome::Settled(Box::new(snapshot)))
}

/// Previous: past the restart threshold, restart the playing track; otherwise
/// step back along play history (falling back to the persisted queue order).
pub(crate) async fn previous(state: &SharedState) -> Result<Outcome, CommandError> {
    let live_position_ms = current_live_position_ms(state).await;
    if live_position_ms.unwrap_or(0) >= player::PREVIOUS_RESTART_THRESHOLD_MS {
        return restart_current_in_place(state).await;
    }
    previous_via_persisted_queue(state, live_position_ms).await
}

async fn previous_via_persisted_queue(
    state: &SharedState,
    live_position_ms: Option<i64>,
) -> Result<Outcome, CommandError> {
    const FAILED: &str = "Failed to move to the previous track.";
    let playback_generation = generation::bump(state).await;
    let previous_track_id = current_playback_track_id(state).await;
    let saved_anchor = save_playback_anchor(state)
        .await
        .map_err(|_| CommandError::StateUpdate(FAILED))?;

    // Walk play history for the most recent entry that still resolves
    // against the live queue, so "previous" follows what actually played
    // across shuffle, manual jumps, and automix insertions. Entries whose
    // rows are gone are consumed: retrying them can never succeed.
    const PREVIOUS_HISTORY_POP_LIMIT: usize = 12;
    let mut history_anchor: Option<player::HistoryAnchor> = None;
    let mut popped_entry: Option<PlayHistoryEntry> = None;
    for _ in 0..PREVIOUS_HISTORY_POP_LIMIT {
        let entry = {
            let mut state_guard = state.write().await;
            state_guard.play_history.pop_previous()
        };
        match entry {
            None => break,
            Some(PlayHistoryEntry::Persisted {
                queue_item_id,
                track_id,
            }) => {
                let row_matches = {
                    let state_guard = state.read().await;
                    state_guard
                        .db
                        .with_conn(move |conn| {
                            Ok(conn
                                .query_row(
                                    "SELECT track_id FROM queue WHERE id = ?1",
                                    rusqlite::params![queue_item_id],
                                    |row| row.get::<_, Option<i64>>(0),
                                )
                                .optional()?)
                        })
                        .ok()
                        .flatten()
                        == Some(Some(track_id))
                };
                if row_matches {
                    history_anchor = Some(player::HistoryAnchor {
                        queue_item_id,
                        track_id: Some(track_id),
                    });
                    popped_entry = Some(PlayHistoryEntry::Persisted {
                        queue_item_id,
                        track_id,
                    });
                    break;
                }
            }
        }
    }

    let outcome = {
        let anchor = history_anchor;
        let live = live_position_ms.unwrap_or(0);
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(move |conn| player::previous_track(conn, live, anchor.as_ref()))
            .map_err(|_| CommandError::StateUpdate(FAILED))?
    };

    if outcome.restart_in_place && live_position_ms.is_some() {
        // Head of the queue with no history while audio is live: restart via
        // seek. With no runtime (stopped), fall through to the switch path so
        // pressing previous still starts audio, as before.
        return restart_current_in_place(state).await;
    }
    let snapshot = resolve_or_skip_pending_current_previous(
        state,
        outcome.snapshot,
        playback_generation,
        "manual_previous_track",
        saved_anchor,
    )
    .await
    .map_err(|error| {
        tracing::error!(
            target: "noor.playback.advance",
            event = "manual_previous_pending_advance_failed",
            error = %error,
            "failed to resolve or skip pending row while moving to previous playback item"
        );
        CommandError::StateUpdate(FAILED)
    })?;

    if !generation::is_current(state, playback_generation).await {
        return Ok(Outcome::Current);
    }

    record_transition_if_changed(state, previous_track_id, &snapshot, "user", false).await;
    sync_session_after_snapshot(
        state,
        &snapshot,
        Some(player::ListenSessionEndReason::Replaced),
    )
    .await;

    if let Some(track) = snapshot.state.current_track.as_ref() {
        {
            // Back-navigation: the incoming Started event must not push the
            // track being navigated away from onto play history, or two prev
            // presses would ping-pong between the same two tracks.
            let mut state_guard = state.write().await;
            state_guard
                .play_history
                .suppress_push_for_generation(playback_generation);
        }
        let crossfade_ms = effective_crossfade_ms(state, snapshot.state.crossfade_ms).await;
        match start_track(
            state,
            StartRequest {
                track,
                generation: playback_generation,
                dispatch: Dispatch::Switch,
                crossfade_ms,
            },
        )
        .await
        {
            Ok(_) => {}
            Err(StartError::Superseded) => return Ok(Outcome::Current),
            Err(error) => {
                // The anchor moved but the audio did not: roll back so state
                // and audio agree instead of leaving the DB pointing at a
                // track the runtime never switched to.
                restore_after_previous_failure(
                    state,
                    saved_anchor,
                    popped_entry,
                    playback_generation,
                )
                .await;
                return Err(CommandError::Start {
                    error,
                    track_id: track.id,
                    stream_context: "TIDAL stream could not be resolved while moving to the previous track.",
                    runtime_message: Some(
                        "Playback runtime was not available for moving to the previous track.",
                    ),
                });
            }
        }
    }

    {
        let state_guard = state.read().await;
        if let Some(track_id) = snapshot.state.current_track.as_ref().map(|t| t.id) {
            let _ = state_guard
                .event_tx
                .send(AppEvent::TrackChanged { track_id });
        }
        let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    }

    Ok(Outcome::Settled(Box::new(
        overlay_snapshot_with_external_track(state, snapshot).await,
    )))
}
