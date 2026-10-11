//! Queue edit endpoints: add, append, play next, replace, remove, move, clear, and save as playlist. See CONTEXT.md "Queue edit".

use super::*;

#[derive(Debug, Deserialize)]
pub struct QueueReplaceRequest {
    /// Ordered library and external rows. Every replacement producer uses this
    /// shape so the queue never changes representation by source.
    pub(super) items: Vec<MixedQueueItemRequest>,
    #[serde(default)]
    pub(super) shuffle_mode: Option<String>,
    #[serde(default)]
    pub(super) start_playback: bool,
}

#[derive(Debug, Deserialize)]
pub struct MixedQueueItemRequest {
    #[serde(default)]
    pub(super) track_id: Option<i64>,
    #[serde(default)]
    pub(super) tidal_id: Option<i64>,
    #[serde(default)]
    pub(super) artist: Option<String>,
    #[serde(default)]
    pub(super) title: Option<String>,
    // Display metadata persisted on the pending row so the queue renders
    // artwork/album/duration before the resolver imports a library track.
    #[serde(default)]
    pub(super) album_title: Option<String>,
    #[serde(default)]
    pub(super) artwork_url: Option<String>,
    #[serde(default)]
    pub(super) duration_ms: Option<i64>,
    #[serde(default)]
    pub(super) artist_tidal_id: Option<i64>,
    #[serde(default)]
    pub(super) album_tidal_id: Option<i64>,
    /// Optional queue provenance, primarily used by radio rows.
    #[serde(default)]
    pub(super) reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct QueueRemoveRequest {
    pub(super) queue_item_id: i64,
}

#[derive(Debug, Deserialize)]
pub struct QueueMoveRequest {
    pub(super) item_id: i64,
    pub(super) new_pos: i32,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueueExternalKind {
    Library,
    Tidal,
    External,
}

#[derive(Debug, Deserialize)]
pub struct QueueExternalRequest {
    pub(super) kind: QueueExternalKind,
    #[serde(default)]
    pub(super) track_id: Option<i64>,
    #[serde(default)]
    pub(super) tidal_id: Option<i64>,
    #[serde(default)]
    pub(super) artist: Option<String>,
    #[serde(default)]
    pub(super) title: Option<String>,
    // Display + identity metadata, used only when folding a TIDAL pick into a live
    // mix as an ephemeral row so the queued row renders with art/album/duration and
    // keeps clickable artist/album links. Ignored on the library/pending paths.
    #[serde(default)]
    pub(super) album_title: Option<String>,
    #[serde(default)]
    pub(super) artwork_url: Option<String>,
    #[serde(default)]
    pub(super) duration_ms: Option<i64>,
    #[serde(default)]
    pub(super) artist_tidal_id: Option<i64>,
    #[serde(default)]
    pub(super) album_tidal_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct QueueExternalManyRequest {
    pub(super) items: Vec<QueueExternalRequest>,
}

#[derive(Debug, Deserialize)]
pub struct PlaylistFromQueueRequest {
    pub(super) name: String,
    #[serde(default)]
    pub(super) include_tidal_only: Option<bool>,
}

#[derive(Debug)]
pub(super) enum PlaylistFromQueueSource {
    Local(i64),
    Tidal(tidal_import::ImportTrackMetadata),
}

pub(super) async fn add_queue_track(
    State(state): State<SharedState>,
    Json(payload): Json<PlaybackTrackRequest>,
) -> Result<Json<Value>, StatusCode> {
    let response = {
        let state_guard = state.read().await;
        // User-driven enqueue; clear the post-clear suppression window.
        state_guard
            .user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
        state_guard
            .db
            .with_conn(|conn| {
                let queue = player::enqueue_track(conn, payload.track_id, "user")?;
                let queue_revision = player::queue_revision(conn);
                let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
                Ok(Json(json!({
                    "queue": queue,
                    "queue_revision": queue_revision
                })))
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    refresh_dj_after_queue_change(state, "add_queue_track").await;
    Ok(response)
}

pub(super) fn queue_external_insert<'a>(
    payload: &'a QueueExternalRequest,
    source: &'a str,
) -> Result<queue::ExternalTrackInsert<'a>, String> {
    let positive = |value: Option<i64>, field: &str| {
        value
            .filter(|id| *id > 0)
            .ok_or_else(|| format!("{field} must be a positive id"))
    };
    let non_empty = |value: &'a Option<String>, field: &str| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("{field} must not be empty"))
    };

    match payload.kind {
        QueueExternalKind::Library => Ok(queue::ExternalTrackInsert {
            artist: payload.artist.as_deref().unwrap_or(""),
            title: payload.title.as_deref().unwrap_or(""),
            source,
            local_track_id: Some(positive(payload.track_id, "track_id")?),
            ..Default::default()
        }),
        // TIDAL rows keep their display metadata so the pending queue row
        // renders artwork/album/duration immediately, before resolution.
        QueueExternalKind::Tidal => Ok(queue::ExternalTrackInsert {
            artist: non_empty(&payload.artist, "artist")?,
            title: non_empty(&payload.title, "title")?,
            source,
            tidal_id_hint: Some(positive(payload.tidal_id, "tidal_id")?),
            album_title: payload.album_title.as_deref(),
            artwork_url: payload.artwork_url.as_deref(),
            duration_ms: payload.duration_ms.filter(|ms| *ms > 0),
            artist_tidal_id: payload.artist_tidal_id.filter(|id| *id > 0),
            album_tidal_id: payload.album_tidal_id.filter(|id| *id > 0),
            ..Default::default()
        }),
        QueueExternalKind::External => Ok(queue::ExternalTrackInsert {
            artist: non_empty(&payload.artist, "artist")?,
            title: non_empty(&payload.title, "title")?,
            source,
            ..Default::default()
        }),
    }
}

/// Return the active persisted queue position, preferring the queue-item anchor.
pub(super) fn current_queue_position(conn: &rusqlite::Connection) -> anyhow::Result<Option<i32>> {
    let (current_queue_item_id, current_track_id): (Option<i64>, Option<i64>) = conn.query_row(
        "SELECT current_queue_item_id, current_track_id FROM playback_state WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    if let Some(queue_item_id) = current_queue_item_id
        && queue_item_matches_current_track(conn, queue_item_id, current_track_id)?
        && let Some(position) = queue_item_position(conn, queue_item_id)?
    {
        return Ok(Some(position));
    }

    if let Some(track_id) = current_track_id
        && let Some((queue_item_id, position)) = first_queue_item_for_track(conn, track_id)?
    {
        conn.execute(
            "UPDATE playback_state SET current_queue_item_id = ?1 WHERE id = 1",
            params![queue_item_id],
        )?;
        return Ok(Some(position));
    }

    if current_queue_item_id.is_some() {
        conn.execute(
            "UPDATE playback_state SET current_queue_item_id = NULL WHERE id = 1",
            [],
        )?;
    }

    Ok(None)
}

/// Look up a persisted queue row position by its stable item id.
pub(super) fn queue_item_position(
    conn: &rusqlite::Connection,
    queue_item_id: i64,
) -> anyhow::Result<Option<i32>> {
    Ok(conn
        .query_row(
            "SELECT position FROM queue WHERE id = ?1",
            params![queue_item_id],
            |row| row.get(0),
        )
        .optional()?)
}

pub(super) fn first_queue_item_for_track(
    conn: &rusqlite::Connection,
    track_id: i64,
) -> anyhow::Result<Option<(i64, i32)>> {
    Ok(conn
        .query_row(
            "SELECT id, position
             FROM queue
             WHERE track_id = ?1
             ORDER BY position ASC, id ASC
             LIMIT 1",
            params![track_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

pub(super) fn first_queue_item_id_for_track(
    conn: &rusqlite::Connection,
    track_id: i64,
) -> anyhow::Result<Option<i64>> {
    Ok(first_queue_item_for_track(conn, track_id)?.map(|(id, _)| id))
}

pub(super) fn preserve_only_queue_item(
    conn: &rusqlite::Connection,
    queue_item_id: i64,
) -> anyhow::Result<()> {
    conn.execute("DELETE FROM queue WHERE id != ?1", params![queue_item_id])?;
    crate::server::radio_continuation::forget_seed(conn);
    conn.execute(
        "UPDATE playback_state SET current_queue_item_id = ?1 WHERE id = 1",
        params![queue_item_id],
    )?;
    Ok(())
}

pub(super) fn preserve_current_track_queue_row(
    conn: &rusqlite::Connection,
    track_id: i64,
) -> anyhow::Result<()> {
    if let Some(queue_item_id) = first_queue_item_id_for_track(conn, track_id)? {
        preserve_only_queue_item(conn, queue_item_id)?;
    } else {
        queue::clear_queue(conn)?;
        conn.execute(
            "UPDATE playback_state SET current_queue_item_id = NULL WHERE id = 1",
            [],
        )?;
    }
    Ok(())
}

pub(super) fn queue_item_track_id(
    conn: &rusqlite::Connection,
    queue_item_id: i64,
) -> anyhow::Result<Option<Option<i64>>> {
    Ok(conn
        .query_row(
            "SELECT track_id FROM queue WHERE id = ?1",
            params![queue_item_id],
            |row| row.get(0),
        )
        .optional()?)
}

pub(super) fn queue_item_matches_current_track(
    conn: &rusqlite::Connection,
    queue_item_id: i64,
    current_track_id: Option<i64>,
) -> anyhow::Result<bool> {
    Ok(queue_item_track_id(conn, queue_item_id)?
        .map(|track_id| track_id == current_track_id)
        .unwrap_or(false))
}

pub(super) fn repair_moved_queue_current_anchor(
    conn: &rusqlite::Connection,
    moved_queue_item_id: i64,
) -> anyhow::Result<bool> {
    let (current_track_id, current_queue_item_id): (Option<i64>, Option<i64>) = conn.query_row(
        "SELECT current_track_id, current_queue_item_id FROM playback_state WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if let Some(queue_item_id) = current_queue_item_id
        && queue_item_matches_current_track(conn, queue_item_id, current_track_id)?
    {
        return Ok(false);
    }

    let repaired_queue_item_id = match current_track_id {
        Some(track_id) => {
            let moved_track_id = queue_item_track_id(conn, moved_queue_item_id)?.flatten();
            if moved_track_id == Some(track_id) {
                Some(moved_queue_item_id)
            } else {
                first_queue_item_id_for_track(conn, track_id)?
            }
        }
        None => None,
    };

    if repaired_queue_item_id == current_queue_item_id {
        return Ok(false);
    }

    conn.execute(
        "UPDATE playback_state SET current_queue_item_id = ?1 WHERE id = 1",
        params![repaired_queue_item_id],
    )?;
    Ok(true)
}

pub(super) async fn queue_append(
    State(state): State<SharedState>,
    Json(payload): Json<QueueExternalRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let insert = queue_external_insert(&payload, "user_queue")
        .map_err(|message| (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))))?;

    let (queue, inserted, queue_revision) = {
        let state_guard = state.read().await;
        state_guard
            .user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let event_tx = state_guard.event_tx.clone();
        state_guard
            .db
            .with_conn(|conn| {
                let inserted = queue::append_external_track(conn, &insert)?;
                let queue = queue::load_queue(conn)?;
                let queue_revision = player::queue_revision(conn);
                let _ = event_tx.send(AppEvent::QueueUpdated);
                Ok((queue, Some(inserted), queue_revision))
            })
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "failed to append queue item" })),
                )
            })?
    };

    let pending_count = matches!(inserted, Some(queue::InsertResult::Pending { .. })) as usize;
    tracing::info!(
        target: "noor.playback.queue",
        event = "queue_append",
        item_count = 1,
        pending_count,
        "appended queue item"
    );

    if let Some(queue::InsertResult::Pending { queue_id }) = inserted {
        spawn_pending_queue_resolver(&state, queue_id).await;
    }
    refresh_dj_after_queue_change(state, "queue_append").await;

    Ok(Json(json!({
        "queue": queue,
        "queue_revision": queue_revision
    })))
}

pub(super) async fn queue_append_many(
    State(state): State<SharedState>,
    Json(payload): Json<QueueExternalManyRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let inserts: Vec<_> = payload
        .items
        .iter()
        .map(|item| queue_external_insert(item, "user_queue"))
        .collect::<Result<_, _>>()
        .map_err(|message| (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))))?;

    let (queue, inserted, queue_revision) = {
        let state_guard = state.read().await;
        state_guard
            .user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let event_tx = state_guard.event_tx.clone();
        state_guard
            .db
            .with_conn(|conn| {
                let inserted = queue::append_external_tracks(conn, &inserts)?;
                let queue = queue::load_queue(conn)?;
                let queue_revision = player::queue_revision(conn);
                let _ = event_tx.send(AppEvent::QueueUpdated);
                Ok((queue, inserted, queue_revision))
            })
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "failed to append queue items" })),
                )
            })?
    };

    let pending_count = inserted
        .iter()
        .filter(|item| matches!(**item, queue::InsertResult::Pending { .. }))
        .count();
    tracing::info!(
        target: "noor.playback.queue",
        event = "queue_append_many",
        item_count = inserts.len(),
        pending_count,
        "appended queue items"
    );

    for item in inserted {
        if let queue::InsertResult::Pending { queue_id } = item {
            spawn_pending_queue_resolver(&state, queue_id).await;
        }
    }
    refresh_dj_after_queue_change(state, "queue_append_many").await;

    Ok(Json(json!({
        "queue": queue,
        "queue_revision": queue_revision
    })))
}

/// Insert after the active persisted queue row. With no active row, append.
pub(super) fn play_next_after_position(current_pos: Option<i32>) -> Option<i32> {
    current_pos
}

pub(super) async fn queue_play_next(
    State(state): State<SharedState>,
    Json(payload): Json<QueueExternalRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let insert = queue_external_insert(&payload, "user_play_next")
        .map_err(|message| (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))))?;

    let (queue, inserted, queue_revision) = {
        let state_guard = state.read().await;
        state_guard
            .user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let event_tx = state_guard.event_tx.clone();
        state_guard
            .db
            .with_conn(|conn| {
                let after = play_next_after_position(current_queue_position(conn)?);
                let inserted = match after {
                    Some(after) => queue::insert_external_track_after(conn, &insert, after)?,
                    None => queue::append_external_track(conn, &insert)?,
                };
                let queue = queue::load_queue(conn)?;
                let queue_revision = player::queue_revision(conn);
                let _ = event_tx.send(AppEvent::QueueUpdated);
                Ok((queue, Some(inserted), queue_revision))
            })
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "failed to insert queue item" })),
                )
            })?
    };

    let pending_count = matches!(inserted, Some(queue::InsertResult::Pending { .. })) as usize;
    tracing::info!(
        target: "noor.playback.queue",
        event = "queue_play_next",
        item_count = 1,
        pending_count,
        "inserted queue item after current"
    );

    if let Some(queue::InsertResult::Pending { queue_id }) = inserted {
        spawn_pending_queue_resolver(&state, queue_id).await;
    }
    refresh_dj_after_queue_change(state, "queue_play_next").await;

    Ok(Json(json!({
        "queue": queue,
        "queue_revision": queue_revision
    })))
}

pub(super) async fn queue_play_next_many(
    State(state): State<SharedState>,
    Json(payload): Json<QueueExternalManyRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let inserts: Vec<_> = payload
        .items
        .iter()
        .map(|item| queue_external_insert(item, "user_play_next"))
        .collect::<Result<_, _>>()
        .map_err(|message| (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))))?;

    let (queue, inserted, queue_revision) = {
        let state_guard = state.read().await;
        state_guard
            .user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let event_tx = state_guard.event_tx.clone();
        state_guard
            .db
            .with_conn(|conn| {
                let after = play_next_after_position(current_queue_position(conn)?);
                let inserted = match after {
                    Some(after) => queue::insert_external_tracks_after(conn, &inserts, after)?,
                    None => queue::append_external_tracks(conn, &inserts)?,
                };
                let queue = queue::load_queue(conn)?;
                let queue_revision = player::queue_revision(conn);
                let _ = event_tx.send(AppEvent::QueueUpdated);
                Ok((queue, inserted, queue_revision))
            })
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "failed to insert queue items" })),
                )
            })?
    };

    let pending_count = inserted
        .iter()
        .filter(|item| matches!(**item, queue::InsertResult::Pending { .. }))
        .count();
    tracing::info!(
        target: "noor.playback.queue",
        event = "queue_play_next_many",
        item_count = inserts.len(),
        pending_count,
        "inserted queue items after current"
    );

    for item in inserted {
        if let queue::InsertResult::Pending { queue_id } = item {
            spawn_pending_queue_resolver(&state, queue_id).await;
        }
    }
    refresh_dj_after_queue_change(state, "queue_play_next_many").await;

    Ok(Json(json!({
        "queue": queue,
        "queue_revision": queue_revision
    })))
}

pub(super) async fn replace_playback_queue(
    State(state): State<SharedState>,
    Json(payload): Json<QueueReplaceRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use crate::server::radio_pipeline::OrderedQueueCandidate;

    let candidates: Vec<OrderedQueueCandidate> = payload
        .items
        .iter()
        .filter(|item| {
            item.track_id.is_some_and(|id| id > 0)
                || item.tidal_id.is_some_and(|id| id > 0)
                || (item.artist.as_deref().is_some_and(|s| !s.trim().is_empty())
                    && item.title.as_deref().is_some_and(|s| !s.trim().is_empty()))
        })
        .map(|item| OrderedQueueCandidate {
            track_id: item.track_id.filter(|id| *id > 0),
            tidal_id: item.tidal_id.filter(|id| *id > 0),
            artist: item.artist.clone().unwrap_or_default(),
            title: item.title.clone().unwrap_or_default(),
            album_title: item.album_title.clone(),
            artwork_url: item.artwork_url.clone(),
            duration_ms: item.duration_ms.filter(|ms| *ms > 0),
            artist_tidal_id: item.artist_tidal_id.filter(|id| *id > 0),
            album_tidal_id: item.album_tidal_id.filter(|id| *id > 0),
            reason: item.reason.clone(),
        })
        .collect();
    if candidates.is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "no queueable tracks in request" })),
        ));
    }
    let queued_count = candidates.len();
    let shuffle_mode = payload
        .shuffle_mode
        .as_deref()
        .map(queue::ShuffleMode::parse)
        .unwrap_or(queue::ShuffleMode::Off);

    let db = {
        let state_guard = state.read().await;
        state_guard.db.clone()
    };
    let (build, shuffle_debug) = db
        .with_conn(move |conn| {
            let build = crate::server::radio_pipeline::replace_queue_with_ordered_candidates(
                conn,
                &candidates,
            )?;
            let mut shuffle_debug = None;
            if shuffle_mode != queue::ShuffleMode::Off {
                let seed = crate::playback::shuffle::generate_shuffle_seed();
                let result = crate::playback::queue::apply_shuffle_with_seed(
                    conn,
                    shuffle_mode,
                    None,
                    seed,
                    "replace_playback_queue",
                )?;
                shuffle_debug = result.debug;
            }
            Ok((build, shuffle_debug))
        })
        .map_err(|e| {
            tracing::error!("replace_playback_queue: queue build failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to build queue" })),
            )
        })?;

    let pending_count = build.pending_item_ids.len();
    spawn_pending_resolvers_for_queue_items(
        &state,
        &db,
        build.pending_item_ids,
        "replace_playback_queue",
    )
    .await;
    {
        let s = state.read().await;
        let _ = s.event_tx.send(AppEvent::QueueUpdated);
    }

    let snapshot = if payload.start_playback {
        start_first_radio_queue_item(&state).await?
    } else {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(player::load_snapshot)
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "failed to load queue snapshot" })),
                )
            })?
    };
    refresh_dj_after_queue_change(state, "replace_playback_queue").await;
    Ok(Json(json!({
        "queued_count": queued_count,
        "pending_count": pending_count,
        "shuffle_debug": shuffle_debug,
        "state": snapshot.state,
        "queue": snapshot.queue,
        "queue_revision": snapshot.queue_revision,
    })))
}

pub(super) async fn remove_queue_track(
    State(state): State<SharedState>,
    Json(payload): Json<QueueRemoveRequest>,
) -> Result<Json<Value>, StatusCode> {
    let outcome = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(|conn| player::remove_queue_item_and_reconcile(conn, payload.queue_item_id))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    let include_playback_state = outcome.removed_current;
    let mut snapshot = outcome.snapshot;
    if outcome.removed_current && outcome.was_playing {
        snapshot = transport_command::continue_after_current_removed(&state, snapshot)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    } else if outcome.removed_current {
        let state_guard = state.read().await;
        let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
        let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
    } else {
        let state_guard = state.read().await;
        let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
    }

    refresh_dj_after_queue_change(state.clone(), "remove_queue_track").await;
    let snapshot = overlay_snapshot_with_external_track(&state, snapshot).await;
    if include_playback_state {
        Ok(Json(json!({
            "queue": snapshot.queue,
            "playback_state": snapshot.state,
            "queue_revision": snapshot.queue_revision
        })))
    } else {
        Ok(Json(json!({
            "queue": snapshot.queue,
            "queue_revision": snapshot.queue_revision
        })))
    }
}

pub(super) async fn move_queue_track(
    State(state): State<SharedState>,
    Json(payload): Json<QueueMoveRequest>,
) -> Result<Json<Value>, StatusCode> {
    let response = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(|conn| {
                queue::move_queue_item(conn, payload.item_id, payload.new_pos)?;
                let repaired_anchor = repair_moved_queue_current_anchor(conn, payload.item_id)?;
                let snapshot = if repaired_anchor {
                    Some(player::load_snapshot(conn)?)
                } else {
                    None
                };
                let queue = match &snapshot {
                    Some(snapshot) => snapshot.queue.clone(),
                    None => queue::load_queue(conn)?,
                };
                if repaired_anchor {
                    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
                }
                let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
                Ok(match snapshot {
                    Some(snapshot) => Json(json!({
                        "queue": queue,
                        "playback_state": snapshot.state,
                        "queue_revision": snapshot.queue_revision
                    })),
                    None => Json(json!({
                        "queue": queue,
                        "queue_revision": player::queue_revision(conn)
                    })),
                })
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    refresh_dj_after_queue_change(state, "move_queue_track").await;
    Ok(response)
}

pub(super) async fn clear_queue_route(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let response = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(|conn| {
                let (current_track_id, current_queue_item_id): (Option<i64>, Option<i64>) =
                    conn.query_row(
                        "SELECT current_track_id, current_queue_item_id FROM playback_state WHERE id = 1",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )?;
                match (current_queue_item_id, current_track_id) {
                    (Some(qid), track_id) => {
                        if queue_item_matches_current_track(conn, qid, track_id)? {
                            preserve_only_queue_item(conn, qid)?;
                        } else if let Some(track_id) = track_id {
                            preserve_current_track_queue_row(conn, track_id)?;
                        } else {
                            queue::clear_queue(conn)?;
                            conn.execute(
                                "UPDATE playback_state SET current_queue_item_id = NULL WHERE id = 1",
                                [],
                            )?;
                        }
                    }
                    (None, Some(track_id)) => {
                        preserve_current_track_queue_row(conn, track_id)?;
                    }
                    (None, None) => {
                        queue::clear_queue(conn)?;
                    }
                }
                // Return the full PlaybackSnapshot ({state, queue}) so the UI can
                // refresh both at once - additive over the prior `{queue}` shape:
                // existing consumers keep reading `queue`, new ones read
                // `playback_state`.
                let snapshot = player::load_snapshot(conn)?;
                // Stamp now() so `ensure_automix_queue_depth` suppresses refill
                // for ~60s; otherwise automix would immediately repopulate the
                // queue and negate the user's manual clear (current_track is
                // still set, which is the only gate the helper checks).
                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                state_guard
                    .user_cleared_at
                    .store(now_secs, std::sync::atomic::Ordering::Relaxed);
                let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
                Ok(Json(json!({
                    "queue": snapshot.queue,
                    "playback_state": snapshot.state,
                    "queue_revision": snapshot.queue_revision,
                })))
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    refresh_dj_after_queue_change(state, "clear_queue_route").await;
    Ok(response)
}

pub(super) fn non_empty_or_default(value: Option<String>, fallback: &str) -> String {
    value
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

pub(super) fn load_persisted_queue_playlist_sources(
    conn: &rusqlite::Connection,
    include_tidal_only: bool,
) -> anyhow::Result<Vec<PlaylistFromQueueSource>> {
    let mut stmt = conn.prepare(
        "SELECT q.track_id, q.pending_artist, q.pending_title, q.tidal_id_hint,
                COALESCE(t.source, '')
         FROM queue q
         LEFT JOIN tracks t ON q.track_id = t.id
         ORDER BY q.position ASC, q.id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, Option<i64>>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;

    let mut sources = Vec::new();
    for row in rows {
        let (track_id, pending_artist, pending_title, tidal_id_hint, track_source) = row?;
        if let Some(track_id) = track_id.filter(|id| *id > 0) {
            if include_tidal_only || track_source.as_str() != "tidal_stream" {
                sources.push(PlaylistFromQueueSource::Local(track_id));
            }
            continue;
        }

        if include_tidal_only && let Some(tidal_id) = tidal_id_hint.filter(|id| *id > 0) {
            sources.push(PlaylistFromQueueSource::Tidal(
                tidal_import::ImportTrackMetadata {
                    tidal_id,
                    title: non_empty_or_default(pending_title, "Unknown title"),
                    artist_name: non_empty_or_default(pending_artist, "Unknown artist"),
                    ..Default::default()
                },
            ));
        }
    }

    Ok(sources)
}

pub(super) async fn resolve_playlist_source_ids(
    db: &crate::db::Database,
    sources: Vec<PlaylistFromQueueSource>,
) -> anyhow::Result<Vec<i64>> {
    let mut track_ids = Vec::with_capacity(sources.len());
    for source in sources {
        match source {
            PlaylistFromQueueSource::Local(track_id) => track_ids.push(track_id),
            PlaylistFromQueueSource::Tidal(meta) => {
                let imported = tidal_import::import_track_from_metadata(db, meta).await?;
                track_ids.push(imported.local_id);
            }
        }
    }
    Ok(track_ids)
}

pub(super) async fn create_playlist_from_queue(
    State(state): State<SharedState>,
    Json(payload): Json<PlaylistFromQueueRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let name = payload.name.trim().to_string();
    if name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Playlist name must not be empty" })),
        ));
    }
    let include_tidal_only = payload.include_tidal_only.unwrap_or(true);
    let db = {
        let state = state.read().await;
        state.db.clone()
    };

    let persisted_sources = db
        .with_conn(|conn| load_persisted_queue_playlist_sources(conn, include_tidal_only))
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Failed to read queue: {e}") })),
            )
        })?;
    let mut sources = Vec::new();

    sources.extend(persisted_sources);

    let track_ids = resolve_playlist_source_ids(&db, sources)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Failed to import queued TIDAL tracks: {e}") })),
            )
        })?;
    if track_ids.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Queue has no tracks that can be saved" })),
        ));
    }

    let response = db
        .with_conn(|conn| {
            let playlist = queries::create_playlist(conn, &name, None)?;
            let added = queries::add_tracks_to_playlist(conn, playlist.id, &track_ids)?;
            let playlist = queries::get_playlist(conn, playlist.id)?
                .ok_or_else(|| anyhow::anyhow!("playlist not found after insert"))?;
            Ok(Json(json!({
                "playlist": playlist,
                "added": added,
            })))
        })
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;
    playlist_routes::notify_playlists_changed(&state).await;
    Ok(response)
}
