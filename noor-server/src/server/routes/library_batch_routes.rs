//! Batch library mutations: `/api/library/batch/*`.
//!
//! Each handler resolves a deduped set of positive library ids, performs the
//! corresponding TIDAL-side mutation, mirrors the change into the local DB, and
//! emits `LibrarySynced` (plus queue/playback events when a delete touches the
//! active queue). Extracted from `routes.rs` verbatim - no behavior change.

use crate::db::queries;
use crate::playback::player;
use crate::services::tidal::mutations as tidal_mutations;
use crate::{AppEvent, SharedState};
use axum::{extract::State, http::StatusCode, response::Json};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::warn;

#[derive(Debug, Deserialize)]
pub(super) struct BatchPlaylistRequest {
    playlist_id: i64,
    track_ids: Vec<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct BatchDeleteRequest {
    track_ids: Option<Vec<i64>>,
    album_ids: Option<Vec<i64>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct BatchGenreRequest {
    genre_id: i64,
    track_ids: Vec<i64>,
}

/// Filter to positive ids, sort, and dedup. Non-positive ids (ephemeral /
/// discovery tracks) are dropped with a warning - they have no library row to
/// mutate.
fn dedupe_positive_ids(ids: &[i64]) -> Vec<i64> {
    let (filtered, dropped): (Vec<i64>, Vec<i64>) = ids.iter().copied().partition(|id| *id > 0);
    if !dropped.is_empty() {
        warn!(
            "dedupe_positive_ids: dropped {} non-positive IDs (ephemeral/discovery tracks): {:?}",
            dropped.len(),
            &dropped[..dropped.len().min(5)]
        );
    }
    let mut ids: Vec<i64> = filtered;
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn require_positive_batch_id(id: i64) -> Result<(), StatusCode> {
    if id <= 0 {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(())
}

pub(super) async fn batch_add_to_playlist(
    State(state): State<SharedState>,
    Json(payload): Json<BatchPlaylistRequest>,
) -> Result<Json<Value>, StatusCode> {
    require_positive_batch_id(payload.playlist_id)?;

    let track_ids = dedupe_positive_ids(&payload.track_ids);
    if track_ids.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let (http, tokens) = {
        let state = state.read().await;
        let tokens = state.tidal_tokens.clone().ok_or(StatusCode::UNAUTHORIZED)?;
        (state.http_client.clone(), tokens)
    };

    let (playlist, track_pairs) = {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| {
                let playlist = queries::get_playlist(conn, payload.playlist_id)?
                    .ok_or_else(|| anyhow::anyhow!("playlist not found"))?;
                let track_pairs = queries::get_track_tidal_ids(conn, &track_ids)?;
                Ok((playlist, track_pairs))
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    let playlist_uuid = playlist.tidal_uuid.ok_or(StatusCode::BAD_REQUEST)?;
    let tidal_track_ids: Vec<i64> = track_pairs.iter().map(|(_, tidal_id)| *tidal_id).collect();
    if tidal_track_ids.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    tidal_mutations::add_to_playlist(
        &http,
        &tokens.access_token,
        &playlist_uuid,
        &tidal_track_ids,
        &tokens.country_code,
    )
    .await
    .map_err(|error| {
        tracing::error!("Batch add to playlist failed: {error}");
        StatusCode::BAD_GATEWAY
    })?;

    let added = {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| {
                let mut position: i64 = conn.query_row(
                    "SELECT COALESCE(MAX(position) + 1, 0) FROM playlist_tracks WHERE playlist_id = ?1",
                    rusqlite::params![payload.playlist_id],
                    |row| row.get(0),
                )?;
                let mut added = 0;
                for (track_id, _) in &track_pairs {
                    added += conn.execute(
                        "INSERT OR IGNORE INTO playlist_tracks (playlist_id, track_id, position)
                         VALUES (?1, ?2, ?3)",
                        rusqlite::params![payload.playlist_id, track_id, position],
                    )?;
                    position += 1;
                }
                conn.execute(
                    "UPDATE playlists
                     SET track_count = (SELECT COUNT(*) FROM playlist_tracks WHERE playlist_id = ?1),
                         updated_at = datetime('now')
                     WHERE id = ?1",
                    rusqlite::params![payload.playlist_id],
                )?;
                Ok(added)
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    {
        let state = state.read().await;
        let _ = state.event_tx.send(AppEvent::LibrarySynced);
    }

    Ok(Json(json!({
        "playlist_id": payload.playlist_id,
        "requested_tracks": track_ids.len(),
        "resolved_tracks": track_pairs.len(),
        "added": added
    })))
}

pub(super) async fn batch_delete_items(
    State(state): State<SharedState>,
    Json(payload): Json<BatchDeleteRequest>,
) -> Result<Json<Value>, StatusCode> {
    let track_ids = dedupe_positive_ids(payload.track_ids.as_deref().unwrap_or(&[]));
    let album_ids = dedupe_positive_ids(payload.album_ids.as_deref().unwrap_or(&[]));
    if track_ids.is_empty() && album_ids.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let (track_pairs, album_pairs) = {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| {
                Ok((
                    queries::get_track_tidal_ids(conn, &track_ids)?,
                    queries::get_album_tidal_ids(conn, &album_ids)?,
                ))
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    let remote_track_ids: Vec<i64> = track_pairs.iter().map(|(_, tidal_id)| *tidal_id).collect();
    let remote_album_ids: Vec<i64> = album_pairs.iter().map(|(_, tidal_id)| *tidal_id).collect();

    let (removed_tracks, removed_albums) =
        if remote_track_ids.is_empty() && remote_album_ids.is_empty() {
            (0, 0)
        } else {
            let (http, tokens) = {
                let state = state.read().await;
                let tokens = state.tidal_tokens.clone().ok_or(StatusCode::UNAUTHORIZED)?;
                (state.http_client.clone(), tokens)
            };

            let removed_tracks = if remote_track_ids.is_empty() {
                0
            } else {
                tidal_mutations::remove_favorite_tracks(
                    &http,
                    &tokens.access_token,
                    &tokens.user_id,
                    &remote_track_ids,
                    &tokens.country_code,
                )
                .await
                .map_err(|error| {
                    tracing::error!("Batch delete tracks failed: {error}");
                    StatusCode::BAD_GATEWAY
                })?
            };

            let removed_albums = if remote_album_ids.is_empty() {
                0
            } else {
                tidal_mutations::remove_favorite_albums(
                    &http,
                    &tokens.access_token,
                    &tokens.user_id,
                    &remote_album_ids,
                    &tokens.country_code,
                )
                .await
                .map_err(|error| {
                    tracing::error!("Batch delete albums failed: {error}");
                    StatusCode::BAD_GATEWAY
                })?
            };

            (removed_tracks, removed_albums)
        };

    // Also delete from local DB so removed items disappear immediately.
    let db = {
        let s = state.read().await;
        s.db.clone()
    };
    let deleted_track_ids = track_ids.clone();
    let outcome = match db.with_conn(|conn| {
        for local_id in &track_ids {
            conn.execute(
                "DELETE FROM tracks WHERE id = ?1",
                rusqlite::params![local_id],
            )?;
        }
        for local_id in &album_ids {
            conn.execute(
                "DELETE FROM albums WHERE id = ?1",
                rusqlite::params![local_id],
            )?;
        }
        let outcome = player::reconcile_after_track_delete(conn, &deleted_track_ids)?;
        Ok::<player::ReconcileOutcome, anyhow::Error>(outcome)
    }) {
        Ok(o) => o,
        Err(e) => {
            warn!("Batch delete: local DB cleanup failed: {e}");
            player::ReconcileOutcome::default()
        }
    };

    {
        let state = state.read().await;
        let _ = state.event_tx.send(AppEvent::LibrarySynced);
        if outcome.queue_changed {
            let _ = state.event_tx.send(AppEvent::QueueUpdated);
        }
        if outcome.current_changed {
            let _ = state.event_tx.send(AppEvent::PlaybackStateChanged);
        }
    }

    Ok(Json(json!({
        "requested_tracks": track_ids.len(),
        "requested_albums": album_ids.len(),
        "removed_tracks": removed_tracks,
        "removed_albums": removed_albums,
        "resolved_tracks": track_pairs.len(),
        "resolved_albums": album_pairs.len()
    })))
}

pub(super) async fn batch_set_genre(
    State(state): State<SharedState>,
    Json(payload): Json<BatchGenreRequest>,
) -> Result<Json<Value>, StatusCode> {
    require_positive_batch_id(payload.genre_id)?;

    let track_ids = dedupe_positive_ids(&payload.track_ids);
    if track_ids.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let affected = {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| {
                let genre_exists: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM genres WHERE id = ?1)",
                    rusqlite::params![payload.genre_id],
                    |row| row.get(0),
                )?;
                if !genre_exists {
                    return Err(anyhow::anyhow!("genre not found"));
                }
                queries::assign_genre_to_tracks(conn, payload.genre_id, &track_ids, "manual")
            })
            .map_err(|error| {
                if error.to_string().contains("genre not found") {
                    StatusCode::NOT_FOUND
                } else {
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            })?
    };

    {
        let state = state.read().await;
        let _ = state.event_tx.send(AppEvent::LibrarySynced);
    }

    Ok(Json(json!({
        "genre_id": payload.genre_id,
        "requested_tracks": track_ids.len(),
        "affected": affected
    })))
}
