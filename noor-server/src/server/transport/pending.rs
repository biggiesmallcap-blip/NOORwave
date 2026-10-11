//! Resolving pending queue rows (Last.fm candidates with no library track yet) and skipping rows that cannot resolve.

use super::snapshot::*;
use crate::playback::{pending, player, queue};
use crate::server::routes::{
    PLAYBACK_ADVANCE_PENDING_SKIP_LIMIT, PLAYBACK_PENDING_BUSY_RETRY_DELAY_MS,
    PLAYBACK_PENDING_BUSY_RETRY_LIMIT, TIDAL_RESOLVE_POOL, import_metadata_from_search_track,
    import_metadata_from_tidal_track, load_persisted_tidal_tokens, refresh_dj_after_queue_change,
    reresolve_tidal_id, select_best_tidal_match,
};
use crate::server::transport::generation::is_current as playback_generation_is_current;
use crate::services::tidal::{client::TidalClient, import as tidal_import};
use crate::{AppEvent, SharedState};
use rusqlite::{OptionalExtension, params};
use std::time::Duration;

pub(crate) async fn find_pending_tidal_match(
    client: &TidalClient,
    db: &crate::db::Database,
    pending_artist: &str,
    pending_title: &str,
    tidal_id_hint: Option<i64>,
) -> anyhow::Result<Option<(f64, tidal_import::ImportTrackMetadata)>> {
    if let Some(tidal_id) = tidal_id_hint.filter(|id| *id > 0) {
        let selected = db.with_conn(|conn| {
            let local = crate::db::catalogue::track_id(conn, tidal_id)?;
            Ok(match local {
                Some(id) => conn
                    .query_row("SELECT tidal_id FROM tracks WHERE id=?1", [id], |r| {
                        r.get::<_, Option<i64>>(0)
                    })?
                    .unwrap_or(tidal_id),
                None => tidal_id,
            })
        })?;
        let track = client.get_track(selected).await?;
        return Ok(Some((1.0, import_metadata_from_tidal_track(track))));
    }

    let query = format!("{} {}", pending_artist, pending_title);
    let results = client.search(&query, TIDAL_RESOLVE_POOL).await?;
    Ok(
        select_best_tidal_match(pending_artist, pending_title, results)
            .map(|(score, track)| (score, import_metadata_from_search_track(track))),
    )
}

/// Atomically promote a pending queue row to a resolved library row, then
/// broadcast `QueueUpdated` if this caller won the race.
///
/// Wraps [`pending::promote`] so both resolver paths funnel through one
/// event-emission contract: any successful promotion broadcasts exactly
/// once.
pub(crate) fn promote_pending_row_emit(
    db: &crate::db::Database,
    event_tx: &tokio::sync::broadcast::Sender<AppEvent>,
    queue_item_id: i64,
    local_track_id: i64,
    score_stored: i32,
) -> bool {
    let outcome = db
        .with_conn(move |conn| {
            pending::promote_if_admitted(conn, queue_item_id, local_track_id, score_stored)
        })
        .unwrap_or(pending::Promotion::NotPending);
    match outcome {
        pending::Promotion::Promoted => {
            let _ = event_tx.send(AppEvent::QueueUpdated);
            true
        }
        pending::Promotion::Rejected => {
            tracing::info!(
                queue_item_id,
                local_track_id,
                "pending resolver: dropped a recommended row the gate rejected"
            );
            let _ = event_tx.send(AppEvent::QueueUpdated);
            false
        }
        pending::Promotion::NotPending => false,
    }
}

/// Background-eager resolver for a single pending queue row.
///
/// Spawned by `radio_start` after inserting pending rows. Bounded by
/// `Arc<Semaphore>` (RESOLVER_POOL_SIZE permits). Unlike the lazy path, this
/// does **not** update `playback_state.current_track_id`. The playing row
/// may not be the one being resolved.
pub(crate) async fn resolve_pending_row(
    state: SharedState,
    db: crate::db::Database,
    tokens: crate::services::tidal::auth::TidalTokens,
    queue_item_id: i64,
    event_tx: tokio::sync::broadcast::Sender<AppEvent>,
) -> bool {
    let row = db
        .with_conn(move |conn| pending::read_identity(conn, queue_item_id))
        .unwrap_or(None);

    let (pending_artist, pending_title, tidal_id_hint) = match row {
        Some(r) => r,
        None => return false,
    };

    let claimed = db
        .with_conn(move |conn| pending::try_claim(conn, queue_item_id))
        .unwrap_or(false);
    if !claimed {
        return false;
    }

    let release = |db: &crate::db::Database, qid: i64| {
        let _ = db.with_conn(move |conn| {
            pending::release(conn, qid);
            Ok(())
        });
    };

    let client = TidalClient::for_session(state.read().await.tidal.clone(), &tokens.country_code)
        .with_metadata_store(state.read().await.db.clone());
    let resolved = match find_pending_tidal_match(
        &client,
        &db,
        &pending_artist,
        &pending_title,
        tidal_id_hint,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(queue_item_id, error = %e, "background resolver: Tidal resolve failed");
            release(&db, queue_item_id);
            return false;
        }
    };

    let (score, metadata) = match resolved {
        Some(p) => p,
        None => {
            let dropped = db
                .with_conn(move |conn| pending::drop_unmatched(conn, queue_item_id))
                .unwrap_or(false);
            tracing::info!(
                queue_item_id,
                artist = %pending_artist,
                title = %pending_title,
                dropped,
                "background resolver: no TIDAL match above threshold"
            );
            if dropped {
                let _ = event_tx.send(AppEvent::QueueUpdated);
            } else {
                release(&db, queue_item_id);
            }
            return false;
        }
    };

    let artist_tidal_id = metadata.artist_tidal_id;
    let imported = crate::services::tidal::import::import_track_from_metadata(&db, metadata).await;

    let (local_id, artist_local_id) = match imported {
        Ok(imp) => (imp.local_id, imp.artist_id),
        Err(e) => {
            tracing::warn!(queue_item_id, error = %e, "background resolver: import failed");
            release(&db, queue_item_id);
            return false;
        }
    };

    // Fire-and-forget: backfill artist photo when TIDAL track payload didn't
    // include one. Independent of promotion success â€” the artist row now
    // exists either way.
    if let Some(tid) = artist_tidal_id {
        let db_bg = db.clone();
        let client_bg =
            TidalClient::for_session(state.read().await.tidal.clone(), &tokens.country_code);
        tokio::spawn(async move {
            crate::services::tidal::artist_photo::ensure_photo_url(
                client_bg,
                db_bg,
                artist_local_id,
                tid,
            )
            .await;
        });
    }

    let score_stored = (score * 1000.0) as i32;
    let promoted = promote_pending_row_emit(&db, &event_tx, queue_item_id, local_id, score_stored);

    if promoted {
        tracing::info!(
            queue_item_id,
            local_id,
            artist = %pending_artist,
            title = %pending_title,
            score,
            "background resolver: promoted pending row"
        );
    }
    promoted
}

/// Attempts to resolve the current pending queue item to a Tidal track.
/// Called when the current queue item is a pending row: track_id IS NULL.
/// Claims ownership via resolving_at, searches Tidal with combined Jaro-Winkler scoring
/// (0.60xartist + 0.40xtitle, threshold 0.85), imports the match via
/// import_track_from_metadata, and atomically promotes the queue row.
///
/// Returns the resolved Track on success, or None if no acceptable match or on error.
/// On failure the resolving_at ownership lock is always released.
pub(crate) async fn resolve_pending_current_queue_item(
    state: &SharedState,
    expected_generation: u64,
) -> Option<crate::db::models::Track> {
    let db = {
        let s = state.read().await;
        s.db.clone()
    };

    let (queue_item_id, pending_artist, pending_title, tidal_id_hint) =
        db.with_conn(pending::current_pending).ok().flatten()?;

    if !playback_generation_is_current(state, expected_generation).await {
        return None;
    }

    tracing::debug!(
        target: "noor.playback.resolve",
        event = "pending_current_resolve_start",
        queue_item_id,
        tidal_id_hint,
        "resolving current pending queue row"
    );

    // Claim ownership; bail if another resolver already claimed this row.
    let claimed = db
        .with_conn(|conn| pending::try_claim(conn, queue_item_id))
        .unwrap_or(false);
    if !claimed {
        tracing::debug!(
            target: "noor.playback.resolve",
            event = "pending_current_resolve_claim_skipped",
            queue_item_id,
            "current pending row is already being resolved"
        );
        return None;
    }

    let release_lock = |db: &crate::db::Database, qid: i64| {
        let _ = db.with_conn(move |conn| {
            pending::release(conn, qid);
            Ok(())
        });
    };

    let (tokens, tidal_session) = {
        let persisted = match load_persisted_tidal_tokens(state).await.ok().flatten() {
            Some(t) => t,
            None => {
                tracing::warn!(
                    target: "noor.playback.resolve",
                    event = "pending_current_resolve_no_tokens",
                    queue_item_id,
                    "TIDAL tokens unavailable for current pending queue row"
                );
                release_lock(&db, queue_item_id);
                return None;
            }
        };
        let s = state.read().await;
        (s.tidal.tokens().unwrap_or(persisted), s.tidal.clone())
    };

    let client = TidalClient::for_session(tidal_session.clone(), &tokens.country_code)
        .with_metadata_store(state.read().await.db.clone());
    let resolved = match find_pending_tidal_match(
        &client,
        &db,
        &pending_artist,
        &pending_title,
        tidal_id_hint,
    )
    .await
    {
        Ok(r) => r,
        Err(error) => {
            tracing::warn!(
                target: "noor.playback.resolve",
                event = "pending_current_resolve_api_failed",
                queue_item_id,
                error = %error,
                "TIDAL lookup failed for current pending queue row"
            );
            release_lock(&db, queue_item_id);
            return None;
        }
    };

    let (score, metadata) = match resolved {
        Some(pair) => pair,
        None => {
            tracing::debug!(
                target: "noor.playback.resolve",
                event = "pending_current_resolve_no_match",
                queue_item_id,
                "no acceptable TIDAL match for current pending queue row"
            );
            release_lock(&db, queue_item_id);
            return None;
        }
    };

    let artist_tidal_id = metadata.artist_tidal_id;
    let imported = crate::services::tidal::import::import_track_from_metadata(&db, metadata).await;

    let (local_id, artist_local_id) = match imported {
        Ok(imp) => (imp.local_id, imp.artist_id),
        Err(error) => {
            tracing::warn!(
                target: "noor.playback.resolve",
                event = "pending_current_import_failed",
                queue_item_id,
                error = %error,
                "import failed for current pending queue row"
            );
            release_lock(&db, queue_item_id);
            return None;
        }
    };

    if !playback_generation_is_current(state, expected_generation).await {
        release_lock(&db, queue_item_id);
        return None;
    }

    if let Some(tid) = artist_tidal_id {
        let db_bg = db.clone();
        let client_bg = TidalClient::for_session(tidal_session.clone(), &tokens.country_code);
        tokio::spawn(async move {
            crate::services::tidal::artist_photo::ensure_photo_url(
                client_bg,
                db_bg,
                artist_local_id,
                tid,
            )
            .await;
        });
    }

    let score_stored = (score * 1000.0) as i32;
    // Atomic promotion: only one resolver wins even under a race.
    let event_tx = {
        let s = state.read().await;
        s.event_tx.clone()
    };
    let promoted = promote_pending_row_emit(&db, &event_tx, queue_item_id, local_id, score_stored);

    if !promoted {
        return None;
    }

    // Close the NULL window so playback_state reflects the real track.
    let state_updated = db
        .with_conn(move |conn| {
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = ?1
                 WHERE id = 1 AND current_queue_item_id = ?2",
                rusqlite::params![local_id, queue_item_id],
            )
            .map_err(anyhow::Error::from)
        })
        .unwrap_or(0);
    if state_updated == 0 || !playback_generation_is_current(state, expected_generation).await {
        return None;
    }
    let _ = event_tx.send(AppEvent::TrackChanged { track_id: local_id });
    let _ = event_tx.send(AppEvent::PlaybackStateChanged);

    tracing::info!(
        target: "noor.playback.resolve",
        event = "pending_current_resolve_success",
        queue_item_id,
        local_id,
        score,
        "current pending queue row resolved"
    );

    db.with_conn(move |conn| queue::get_track_by_id(conn, local_id))
        .ok()
        .flatten()
}

pub(crate) async fn adopt_resolved_current_queue_item(
    state: &SharedState,
    queue_item_id: i64,
    generation: u64,
) -> anyhow::Result<Option<player::PlaybackSnapshot>> {
    if !playback_generation_is_current(state, generation).await {
        return Ok(None);
    }

    let (db, event_tx) = {
        let state_guard = state.read().await;
        (state_guard.db.clone(), state_guard.event_tx.clone())
    };

    let adopted_track_id = db.with_conn(move |conn| {
        let track_id: Option<i64> = conn
            .query_row(
                "SELECT track_id FROM queue WHERE id = ?1 AND track_id IS NOT NULL",
                params![queue_item_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(track_id) = track_id else {
            return Ok(None);
        };
        let updated = conn.execute(
            "UPDATE playback_state
             SET current_track_id = ?1, position_ms = 0
             WHERE id = 1
               AND current_track_id IS NULL
               AND current_queue_item_id = ?2",
            params![track_id, queue_item_id],
        )?;
        Ok(if updated == 1 { Some(track_id) } else { None })
    })?;

    let Some(track_id) = adopted_track_id else {
        return Ok(None);
    };
    if !playback_generation_is_current(state, generation).await {
        return Ok(None);
    }

    let _ = event_tx.send(AppEvent::TrackChanged { track_id });
    let _ = event_tx.send(AppEvent::PlaybackStateChanged);
    let snapshot = load_persisted_playback_snapshot(state).await?;
    if snapshot.state.current_track.is_some() {
        tracing::info!(
            target: "noor.playback.resolve",
            event = "pending_current_adopted_background_resolution",
            queue_item_id,
            track_id,
            "adopted pending row resolved by background resolver"
        );
        return Ok(Some(snapshot));
    }
    Ok(None)
}

pub(crate) async fn pending_current_resolver_is_busy(
    state: &SharedState,
    queue_item_id: i64,
) -> bool {
    let db = {
        let state_guard = state.read().await;
        state_guard.db.clone()
    };
    db.with_conn(move |conn| pending::has_fresh_resolver_lock(conn, queue_item_id))
        .unwrap_or(false)
}

pub(crate) async fn resolve_or_skip_pending_current(
    state: &SharedState,
    snapshot: player::PlaybackSnapshot,
    generation: u64,
    context: &'static str,
) -> anyhow::Result<player::PlaybackSnapshot> {
    resolve_or_skip_pending_current_in_direction(
        state,
        snapshot,
        generation,
        context,
        PendingAdvanceDirection::Next,
        PendingGiveUp::StopPlayback,
    )
    .await
}

pub(crate) async fn resolve_or_skip_pending_current_previous(
    state: &SharedState,
    snapshot: player::PlaybackSnapshot,
    generation: u64,
    context: &'static str,
    saved_anchor: SavedPlaybackAnchor,
) -> anyhow::Result<player::PlaybackSnapshot> {
    resolve_or_skip_pending_current_in_direction(
        state,
        snapshot,
        generation,
        context,
        PendingAdvanceDirection::Previous,
        PendingGiveUp::RestoreAnchor(saved_anchor),
    )
    .await
}

pub(crate) async fn resolve_or_skip_pending_current_in_direction(
    state: &SharedState,
    mut snapshot: player::PlaybackSnapshot,
    generation: u64,
    context: &'static str,
    direction: PendingAdvanceDirection,
    give_up: PendingGiveUp,
) -> anyhow::Result<player::PlaybackSnapshot> {
    let mut skipped = 0usize;
    let mut busy_waits = 0usize;

    loop {
        if snapshot.state.current_track.is_some() || snapshot.state.current_queue_item_id.is_none()
        {
            return Ok(snapshot);
        }

        let Some(queue_item_id) = snapshot.state.current_queue_item_id else {
            return Ok(snapshot);
        };
        if resolve_pending_current_queue_item(state, generation)
            .await
            .is_some()
        {
            snapshot = load_persisted_playback_snapshot(state).await?;
            if snapshot.state.current_track.is_some() {
                return Ok(snapshot);
            }
        } else {
            let reloaded = load_persisted_playback_snapshot(state).await?;
            if reloaded.state.current_track.is_some()
                || reloaded.state.current_queue_item_id != Some(queue_item_id)
            {
                snapshot = reloaded;
                busy_waits = 0;
                continue;
            }
        }

        if !playback_generation_is_current(state, generation).await {
            return load_persisted_playback_snapshot(state).await;
        }

        if let Some(adopted_snapshot) =
            adopt_resolved_current_queue_item(state, queue_item_id, generation).await?
        {
            return Ok(adopted_snapshot);
        }

        if busy_waits < PLAYBACK_PENDING_BUSY_RETRY_LIMIT
            && pending_current_resolver_is_busy(state, queue_item_id).await
        {
            busy_waits += 1;
            tracing::debug!(
                target: "noor.playback.advance",
                event = "pending_current_busy_wait",
                context,
                queue_item_id,
                busy_waits,
                direction = direction.as_str(),
                "current pending row is still resolving; waiting before skip"
            );
            tokio::time::sleep(Duration::from_millis(PLAYBACK_PENDING_BUSY_RETRY_DELAY_MS)).await;
            snapshot = load_persisted_playback_snapshot(state).await?;
            continue;
        }

        skipped += 1;
        busy_waits = 0;
        tracing::warn!(
            target: "noor.playback.advance",
            event = "pending_current_skipped",
            context,
            queue_item_id,
            skipped,
            direction = direction.as_str(),
            "current pending row did not resolve; stepping over queue item"
        );

        if skipped > PLAYBACK_ADVANCE_PENDING_SKIP_LIMIT {
            return match give_up {
                PendingGiveUp::StopPlayback => {
                    stop_persisted_playback_after_advance_failure(state, context).await
                }
                PendingGiveUp::RestoreAnchor(saved) => {
                    tracing::warn!(
                        target: "noor.playback.advance",
                        event = "previous_restored_after_pending_skip_limit",
                        context,
                        "restoring playback anchor after pending rows failed to resolve on previous"
                    );
                    restore_playback_anchor(state, saved).await
                }
            };
        }

        snapshot = step_persisted_playback_snapshot(state, direction).await?;
    }
}

pub(crate) async fn spawn_pending_queue_resolver(state: &SharedState, queue_item_id: i64) {
    let tokens_opt: Option<crate::services::tidal::auth::TidalTokens> = {
        let s = state.read().await;
        if let Some(t) = s.tidal.tokens() {
            Some(t)
        } else {
            drop(s);
            load_persisted_tidal_tokens(state).await.ok().flatten()
        }
    };

    let Some(tokens) = tokens_opt else {
        return;
    };

    let (db, event_tx) = {
        let s = state.read().await;
        (s.db.clone(), s.event_tx.clone())
    };
    let state = state.clone();
    tokio::spawn(async move {
        if resolve_pending_row(state.clone(), db, tokens, queue_item_id, event_tx).await {
            refresh_dj_after_queue_change(state, "pending_queue_resolved").await;
        }
    });
}

/// After asset-not-ready, search for an equivalent recording and retain a
/// verified candidate as an alias. Switch only with independent evidence that
/// the selected release is unavailable; transient asset errors are insufficient.
pub(crate) fn spawn_tidal_id_reresolve(state: &SharedState, track_id: i64) {
    let state = state.clone();
    tokio::spawn(async move {
        match reresolve_tidal_id(&state, track_id).await {
            Ok(Some(new_id)) => tracing::info!(
                target: "noor.playback.reresolve",
                track_id,
                new_tidal_id = new_id,
                "healed track with a fresh TIDAL id"
            ),
            Ok(None) => tracing::debug!(
                target: "noor.playback.reresolve",
                track_id,
                "no fresh TIDAL id found to heal track"
            ),
            Err(error) => tracing::debug!(
                target: "noor.playback.reresolve",
                track_id,
                error = %error,
                "TIDAL id re-resolve failed"
            ),
        }
    });
}

#[derive(Clone, Copy)]
pub(crate) enum PendingAdvanceDirection {
    Next,
    Previous,
}

/// What to do when pending rows keep failing to resolve during an advance.
#[derive(Clone, Copy)]
pub(crate) enum PendingGiveUp {
    /// Forward advance: stop playback (the queue ahead is unplayable).
    StopPlayback,
    /// Backward navigation: put the anchor back where it was. Pressing
    /// "previous" must never kill the session; the current track keeps
    /// playing.
    RestoreAnchor(SavedPlaybackAnchor),
}

impl PendingAdvanceDirection {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Next => "next",
            Self::Previous => "previous",
        }
    }
}
