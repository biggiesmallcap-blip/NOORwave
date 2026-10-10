//! Track and album favorite toggles (library and TIDAL).

use super::*;

#[derive(Debug, Deserialize)]
pub struct TrackFavoriteRequest {
    pub(super) track_id: i64,
    pub(super) favorite: bool,
}

#[derive(Debug, Deserialize)]
pub struct AlbumFavoriteRequest {
    pub(super) album_id: i64,
    pub(super) favorite: bool,
}

pub(super) async fn set_track_favorite(
    State(state): State<SharedState>,
    Json(payload): Json<TrackFavoriteRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if payload.track_id <= 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "invalid_track",
                "message": "A valid track id is required.",
            })),
        ));
    }

    let track = {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| queue::get_track_by_id(conn, payload.track_id))
            .map_err(|error| {
                error!(
                    "Failed to load track {} for favorite toggle: {error}",
                    payload.track_id
                );
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "status": "track_lookup_failed",
                        "message": "NOOR couldn't load that track right now.",
                    })),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    Json(json!({
                        "status": "track_not_found",
                        "message": "That track could not be found.",
                    })),
                )
            })?
    };

    let tidal_id = track.tidal_id;
    let was_favorite = track.is_favorite;
    let state_changed = was_favorite != payload.favorite;

    // Update local DB immediately - Tidal sync happens in the background.
    // An intentional re-like moves the song to the top once; delivery retries retain that action time.
    {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| {
                let tx = conn.unchecked_transaction()?;
                crate::db::catalogue_favorites::request(
                    &tx,
                    "track",
                    payload.track_id,
                    payload.favorite,
                )?;
                tx.commit()?;
                Ok(())
            })
            .map_err(|error| {
                error!(
                    "Failed to persist favorite state for track {}: {error}",
                    payload.track_id
                );
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "status": "favorite_persist_failed",
                        "message": "NOOR couldn't refresh the local favorite state.",
                    })),
                )
            })?;

        let _ = state.event_tx.send(AppEvent::LibrarySynced);
    }

    if payload.favorite && state_changed {
        crate::services::scrobbling::enqueue_favorite_love(state.clone(), &track).await;
    }

    crate::services::tidal::favorites::run_if_idle(state.clone()).await;

    Ok(Json(json!({
        "track_id": payload.track_id,
        "tidal_id": tidal_id,
        "favorite": payload.favorite,
        "updated": state_changed
    })))
}

/// Toggle an album's favorite ("liked") state. Mirrors `set_track_favorite`:
/// the local `albums.is_favorite` flag flips immediately (which also counts
/// the album's tracks as library via `favorite_only`), and the TIDAL favorite
/// is synced in the background with a one-shot auth recovery. Unliking never
/// demotes anything beyond the favorite flag itself.
pub(super) async fn set_album_favorite(
    State(state): State<SharedState>,
    Json(payload): Json<AlbumFavoriteRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if payload.album_id <= 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "invalid_album",
                "message": "A valid album id is required.",
            })),
        ));
    }

    let album_row: Option<(Option<i64>, bool)> = {
        let s = state.read().await;
        s.db.with_conn(|conn| {
            let row = conn
                .query_row(
                    "SELECT tidal_id, is_favorite FROM albums WHERE id = ?1",
                    rusqlite::params![payload.album_id],
                    |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1)? != 0)),
                )
                .optional()?;
            Ok::<_, anyhow::Error>(row)
        })
        .map_err(|error| {
            error!(
                "Failed to load album {} for favorite toggle: {error}",
                payload.album_id
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "status": "album_lookup_failed",
                    "message": "NOOR couldn't load that album right now.",
                })),
            )
        })?
    };

    let Some((tidal_id, was_favorite)) = album_row else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({
                "status": "album_not_found",
                "message": "That album could not be found.",
            })),
        ));
    };

    let state_changed = was_favorite != payload.favorite;

    {
        let s = state.read().await;
        s.db.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            crate::db::catalogue_favorites::request(
                &tx,
                "album",
                payload.album_id,
                payload.favorite,
            )?;
            tx.commit()?;
            Ok(())
        })
        .map_err(|error| {
            error!(
                "Failed to persist favorite state for album {}: {error}",
                payload.album_id
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "status": "favorite_persist_failed",
                    "message": "NOOR couldn't refresh the local favorite state.",
                })),
            )
        })?;

        let _ = s.event_tx.send(AppEvent::LibrarySynced);
    }

    crate::services::tidal::favorites::run_if_idle(state.clone()).await;

    Ok(Json(json!({
        "album_id": payload.album_id,
        "tidal_id": tidal_id,
        "favorite": payload.favorite,
        "updated": state_changed
    })))
}
