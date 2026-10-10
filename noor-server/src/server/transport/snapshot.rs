//! Persisted playback snapshot stepping, the previous-track anchor, and live position.

use super::pending::*;
use crate::playback::history::PlayHistoryEntry;
use crate::playback::player;
use crate::server::transport::generation::is_current as playback_generation_is_current;
use crate::{AppEvent, SharedState};
use rusqlite::params;

pub(crate) async fn load_persisted_playback_snapshot(
    state: &SharedState,
) -> anyhow::Result<player::PlaybackSnapshot> {
    let state_guard = state.read().await;
    state_guard.db.with_conn(player::load_snapshot)
}

pub(crate) async fn next_persisted_playback_snapshot(
    state: &SharedState,
) -> anyhow::Result<player::PlaybackSnapshot> {
    let state_guard = state.read().await;
    let cleared = recently_cleared(&state_guard);
    state_guard
        .db
        .with_conn(|conn| player::next_track(conn, cleared))
}

pub(crate) async fn previous_persisted_playback_snapshot(
    state: &SharedState,
) -> anyhow::Result<player::PlaybackSnapshot> {
    // Pending-skip stepping: always step back one row (live position 0,
    // no history target). The restart-vs-back decision was already made by
    // the previous_track route before the stepping loop started.
    let state_guard = state.read().await;
    state_guard
        .db
        .with_conn(|conn| Ok(player::previous_track(conn, 0, None)?.snapshot))
}

pub(crate) async fn step_persisted_playback_snapshot(
    state: &SharedState,
    direction: PendingAdvanceDirection,
) -> anyhow::Result<player::PlaybackSnapshot> {
    match direction {
        PendingAdvanceDirection::Next => next_persisted_playback_snapshot(state).await,
        PendingAdvanceDirection::Previous => previous_persisted_playback_snapshot(state).await,
    }
}

pub(crate) async fn save_playback_anchor(
    state: &SharedState,
) -> anyhow::Result<SavedPlaybackAnchor> {
    let state_guard = state.read().await;
    state_guard.db.with_conn(|conn| {
        Ok(conn.query_row(
            "SELECT current_track_id, current_queue_item_id, position_ms, is_playing
             FROM playback_state WHERE id = 1",
            [],
            |row| {
                Ok(SavedPlaybackAnchor {
                    current_track_id: row.get(0)?,
                    current_queue_item_id: row.get(1)?,
                    position_ms: row.get(2)?,
                    is_playing: row.get(3)?,
                })
            },
        )?)
    })
}

pub(crate) async fn restore_playback_anchor(
    state: &SharedState,
    saved: SavedPlaybackAnchor,
) -> anyhow::Result<player::PlaybackSnapshot> {
    let state_guard = state.read().await;
    let snapshot = state_guard.db.with_conn(move |conn| {
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = ?1, current_queue_item_id = ?2,
                 position_ms = ?3, is_playing = ?4
             WHERE id = 1",
            params![
                saved.current_track_id,
                saved.current_queue_item_id,
                saved.position_ms,
                saved.is_playing
            ],
        )?;
        player::load_snapshot(conn)
    })?;
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    Ok(snapshot)
}

pub(crate) async fn stop_persisted_playback_after_advance_failure(
    state: &SharedState,
    context: &'static str,
) -> anyhow::Result<player::PlaybackSnapshot> {
    tracing::warn!(
        target: "noor.playback.advance",
        event = "advance_stopped_after_pending_skip_limit",
        context,
        "stopping playback after pending queue rows failed to resolve"
    );
    let state_guard = state.read().await;
    state_guard.db.with_conn(|conn| {
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = NULL,
                 current_queue_item_id = NULL,
                 position_ms = 0,
                 is_playing = 0
             WHERE id = 1",
            [],
        )?;
        player::load_snapshot(conn)
    })
}

/// Undo a failed previous-track navigation: put the popped history entry
/// back (so retrying targets the same track), disarm the history push
/// suppression, and roll the DB anchor back to what is still audible -
/// unless a newer user action already owns the playback state.
pub(crate) async fn restore_after_previous_failure(
    state: &SharedState,
    saved: SavedPlaybackAnchor,
    popped: Option<PlayHistoryEntry>,
    generation: u64,
) {
    {
        let mut state_guard = state.write().await;
        state_guard.play_history.clear_suppression();
        if let Some(entry) = popped {
            state_guard.play_history.restore_popped(entry);
        }
    }
    if playback_generation_is_current(state, generation).await
        && let Err(error) = restore_playback_anchor(state, saved).await
    {
        tracing::warn!(
            target: "noor.playback.advance",
            event = "previous_anchor_restore_failed",
            error = %error,
            "failed to roll back playback anchor after previous-track failure"
        );
    }
}

/// Audible playhead in ms straight from the runtime, or None when no
/// runtime/device info exists (nothing is playing or the runtime is gone).
/// The DB's `playback_state.position_ms` must never be used for playhead
/// decisions: nothing persists the live position into it during playback,
/// so it reads 0 mid-track.
pub(crate) async fn current_live_position_ms(state: &SharedState) -> Option<i64> {
    let state_guard = state.read().await;
    let pair = state_guard
        .playback_runtime
        .as_ref()
        .zip(state_guard.playback_runtime_info.as_ref());
    pair.map(|(rt, info)| rt.handle.get_position_ms(info.sample_rate, info.channels))
}

pub(crate) async fn overlay_snapshot_with_external_track(
    _state: &SharedState,
    snapshot: player::PlaybackSnapshot,
) -> player::PlaybackSnapshot {
    snapshot
}

pub(crate) async fn overlay_snapshot_with_external_track_and_position(
    _state: &SharedState,
    mut snapshot: player::PlaybackSnapshot,
    live_position_ms: Option<i64>,
) -> player::PlaybackSnapshot {
    if let Some(position_ms) = live_position_ms {
        snapshot.state.position_ms = position_ms;
    }
    snapshot
}

pub(crate) async fn current_playback_track_id(state: &SharedState) -> Option<i64> {
    let guard = state.read().await;
    guard
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT current_track_id FROM playback_state WHERE id = 1",
                [],
                |row| row.get::<_, Option<i64>>(0),
            )
            .map_err(Into::into)
        })
        .ok()
        .flatten()
}

/// True when the user manually cleared the queue within the last 60 seconds.
/// `ensure_automix_queue_depth` reads this so an immediately-following automix
/// pass doesn't refill the queue and visually negate the user's clear.
pub(crate) fn recently_cleared(state: &crate::AppState) -> bool {
    let cleared_at = state
        .user_cleared_at
        .load(std::sync::atomic::Ordering::Relaxed);
    if cleared_at == 0 {
        return false;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    now - cleared_at < 60
}

/// Playback anchor captured before a mutation so failure paths can roll it
/// back. Without the rollback, a previous/next whose stream resolve fails
/// leaves the DB pointing at a track the runtime never switched to, and the
/// next press steps from the wrong place.
#[derive(Clone, Copy)]
pub(crate) struct SavedPlaybackAnchor {
    pub(crate) current_track_id: Option<i64>,
    pub(crate) current_queue_item_id: Option<i64>,
    pub(crate) position_ms: i64,
    pub(crate) is_playing: bool,
}
