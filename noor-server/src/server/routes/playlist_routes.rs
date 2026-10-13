use crate::SharedState;
use crate::db::queries;
use crate::services::tidal::mutations as tidal_mutations;
use crate::smart::playlists::{
    PlaylistEvaluationContext, SmartPlaylistDefinition, TrackDspFeatures, evaluate_playlist,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
pub(super) struct AddTracksToPlaylistRequest {
    track_ids: Vec<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CreateSmartPlaylistRequest {
    name: String,
    description: Option<String>,
    /// The root `RuleClause` as a raw JSON value - validated by deserializing into
    /// `SmartPlaylistDefinition` before writing to DB.
    rules: Value,
}

#[derive(Debug, Deserialize)]
pub(super) struct UpdateSmartPlaylistRequest {
    name: String,
    description: Option<String>,
    rules: Value,
}

#[derive(Debug, Deserialize)]
pub(super) struct PreviewSmartPlaylistRequest {
    /// Optional name/description carried over from the editor draft - the
    /// preview doesn't persist anything so these are decorative.
    name: Option<String>,
    description: Option<String>,
    rules: Value,
}

#[derive(Debug, Deserialize)]
pub(super) struct ArtistSearchParams {
    q: Option<String>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CreatePlaylistRequest {
    name: String,
    description: Option<String>,
    #[serde(default)]
    sync_to_tidal: bool,
    #[serde(default)]
    track_ids: Vec<i64>,
    tidal_track_ids: Option<Vec<i64>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct UpdatePlaylistRequest {
    name: String,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RemovePlaylistTracksRequest {
    /// Zero-based positions within the playlist, not track ids: the schema
    /// permits the same track at two positions, so a track id would be
    /// ambiguous about which copy to drop.
    positions: Vec<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MovePlaylistTrackRequest {
    from: i64,
    to: i64,
}

fn require_positive_playlist_id(id: i64) -> Result<(), StatusCode> {
    if id <= 0 {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(())
}

fn require_positive_playlist_id_json(id: i64) -> Result<(), (StatusCode, Json<Value>)> {
    if id <= 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Expected a positive playlist id" })),
        ));
    }
    Ok(())
}

fn require_positive_track_ids(track_ids: &[i64]) -> Result<(), (StatusCode, Json<Value>)> {
    if track_ids.iter().any(|id| *id <= 0) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Track ids must be positive" })),
        ));
    }
    Ok(())
}

/// Tell connected clients a playlist changed so their caches invalidate without
/// a manual refresh. Best-effort: a closed broadcast channel is not an error.
pub(super) async fn notify_playlists_changed(state: &SharedState) {
    let state = state.read().await;
    if let Ok(mut cache) = state.tidal_playlist_tracks_cache.lock() {
        cache.clear();
    }
    let _ = state.event_tx.send(crate::AppEvent::PlaylistsChanged);
}

fn playlist_error_status(error: anyhow::Error) -> StatusCode {
    if error.to_string().contains("playlist not found") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

fn playlist_error_response(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    let message = error.to_string();
    let status =
        if message.contains("playlist not found") || message.contains("smart playlist not found") {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
    (
        status,
        Json(json!({ "message": message, "error": message })),
    )
}

/// Trim and reject a blank playlist name.
fn require_playlist_name(raw: &str) -> Result<String, (StatusCode, Json<Value>)> {
    let name = raw.trim().to_string();
    if name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Playlist name must not be empty" })),
        ));
    }
    Ok(name)
}

/// Map a failed TIDAL write onto a status the UI can act on.
///
/// A conflict is the user's to resolve (refresh, retry), so it must not read as
/// a generic upstream failure.
pub(super) fn tidal_write_error(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    if error
        .downcast_ref::<tidal_mutations::LibraryWritesDisabled>()
        .is_some()
    {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "message": error.to_string() })),
        );
    }
    if error
        .downcast_ref::<tidal_mutations::UnfinishedPlaylist>()
        .is_some()
    {
        tracing::error!(%error, "TIDAL snapshot cleanup failed");
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "message": error.to_string() })),
        );
    }
    if error
        .downcast_ref::<tidal_mutations::PlaylistConflict>()
        .is_some()
    {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "message": "This playlist changed on TIDAL. Refresh it and try again.",
                "error": error.to_string(),
            })),
        );
    }
    tracing::error!("TIDAL playlist write failed: {error}");
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({ "message": "TIDAL rejected the change", "error": error.to_string() })),
    )
}

/// A session-bound TIDAL client for a playlist write, or `None` when TIDAL is
/// not connected. A local-only playlist never needs these.
async fn tidal_write_context(
    state: &SharedState,
) -> Option<crate::services::tidal::client::TidalClient> {
    state.read().await.tidal.client()
}

/// Load a playlist or 404. Returns the row so callers can branch on
/// `tidal_uuid` and `is_smart`.
async fn load_playlist(
    state: &SharedState,
    id: i64,
) -> Result<crate::db::models::Playlist, (StatusCode, Json<Value>)> {
    let guard = state.read().await;
    guard
        .db
        .with_conn(|conn| {
            queries::get_playlist(conn, id)?.ok_or_else(|| anyhow::anyhow!("playlist not found"))
        })
        .map_err(playlist_error_response)
}

/// Where each local row of a TIDAL-mirrored playlist sits in the live TIDAL
/// playlist: `map[local_position] = tidal_index`.
///
/// The local copy can differ from TIDAL's item list: sync reads tracks only, so
/// videos are missing locally, and the playlist may have been edited in another
/// TIDAL app since the last sync. `remote` is TIDAL's list with each track
/// resolved to its local id (`None` for videos and unknown tracks). Each local
/// row matches the next remote item with the same id, in order. A local row
/// with no match means TIDAL no longer has it, so the edit is refused (`None`)
/// rather than applied to whatever now sits at that index.
pub(super) fn align_to_tidal(local: &[i64], remote: &[Option<i64>]) -> Option<Vec<usize>> {
    let mut map = Vec::with_capacity(local.len());
    let mut next = 0;
    for id in local {
        let offset = remote[next..].iter().position(|item| *item == Some(*id))?;
        map.push(next + offset);
        next += offset + 1;
    }
    Some(map)
}

/// TIDAL's `toIndex` for moving local row `from` to local index `to`, both
/// measured after lifting the row out (the local and TIDAL move convention).
/// The item lands just before the local row that will follow it, or just after
/// the last local row, so videos and unknown items keep their neighbours.
pub(super) fn tidal_move_target(map: &[usize], from: usize, to: usize) -> usize {
    let tidal_from = map[from];
    let lifted = |index: usize| if index > tidal_from { index - 1 } else { index };
    let others: Vec<usize> = map
        .iter()
        .enumerate()
        .filter(|(position, _)| *position != from)
        .map(|(_, index)| *index)
        .collect();
    match others.get(to) {
        Some(&next) => lifted(next),
        None => others.last().map_or(0, |&last| lifted(last) + 1),
    }
}

/// Read the live TIDAL playlist and map this playlist's local rows onto it.
async fn locate_on_tidal(
    state: &SharedState,
    client: &crate::services::tidal::client::TidalClient,
    uuid: &str,
    playlist_id: i64,
) -> Result<(Vec<usize>, tidal_mutations::PlaylistItems), (StatusCode, Json<Value>)> {
    let remote = tidal_mutations::get_playlist_items(client, uuid)
        .await
        .map_err(tidal_write_error)?;
    let map = state
        .read()
        .await
        .db
        .with_conn(|conn| {
            let local = queries::playlist_track_order(conn, playlist_id)?;
            let resolved = remote
                .items
                .iter()
                .map(|item| match item {
                    Some(tidal_id) => crate::db::catalogue::track_id(conn, *tidal_id),
                    None => Ok(None),
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            Ok(align_to_tidal(&local, &resolved))
        })
        .map_err(playlist_error_response)?
        .ok_or_else(|| tidal_write_error(tidal_mutations::PlaylistConflict.into()))?;
    Ok((map, remote))
}

/// Existing callers create local playlists. Snapshot saves opt into TIDAL.
pub(super) async fn create_playlist_route(
    State(state): State<SharedState>,
    Json(payload): Json<CreatePlaylistRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let name = require_playlist_name(&payload.name)?;
    let description = payload
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty());

    if payload.sync_to_tidal {
        return save_tidal_playlist(
            &state,
            &name,
            description,
            &payload.track_ids,
            payload.tidal_track_ids.as_deref(),
        )
        .await;
    }

    let response = {
        let guard = state.read().await;
        guard
            .db
            .with_conn(|conn| {
                let playlist = queries::create_playlist(conn, &name, description)?;
                Ok(Json(json!({ "playlist": playlist })))
            })
            .map_err(playlist_error_response)?
    };
    notify_playlists_changed(&state).await;
    Ok(response)
}

async fn save_tidal_playlist(
    state: &SharedState,
    name: &str,
    description: Option<&str>,
    track_ids: &[i64],
    original_tidal_ids: Option<&[i64]>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_positive_track_ids(track_ids)?;
    if track_ids.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Save at least one track" })),
        ));
    }
    if let Some(originals) = original_tidal_ids {
        require_positive_track_ids(originals)?;
        if originals.len() != track_ids.len() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": "Each snapshot track needs its original TIDAL id" })),
            ));
        }
    }
    let tidal_ids = state
        .read()
        .await
        .db
        .with_conn(|conn| {
            if let Some(originals) = original_tidal_ids {
                // A provider alias can resolve to a local recording without a
                // selected TIDAL id. Verify identity using the import lookup.
                for (local_id, tidal_id) in track_ids.iter().zip(originals) {
                    anyhow::ensure!(
                        crate::db::catalogue::track_id(conn, *tidal_id)? == Some(*local_id),
                        "Each original TIDAL id must resolve to its snapshot track"
                    );
                }
                return Ok(originals.to_vec());
            }
            track_ids
                .iter()
                .map(|id| {
                    conn.query_row("SELECT tidal_id FROM tracks WHERE id = ?1", [id], |row| {
                        row.get::<_, Option<i64>>(0)
                    })?
                    .filter(|id| *id > 0)
                    .ok_or_else(|| anyhow::anyhow!("Every saved track must have a TIDAL id"))
                })
                .collect::<anyhow::Result<Vec<_>>>()
        })
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": error.to_string() })),
            )
        })?;
    let client = tidal_write_context(state).await.ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "message": "Connect TIDAL to save this playlist" })),
        )
    })?;
    // A large snapshot takes many sequential TIDAL calls. Run it detached so
    // a client timeout or disconnect cannot drop the handler halfway through
    // and leave a half-filled playlist on TIDAL with no local record.
    let task_state = state.clone();
    let (name, description) = (name.to_string(), description.map(str::to_string));
    let track_ids = track_ids.to_vec();
    let playlist = tokio::spawn(async move {
        let state = &task_state;
        let (name, description) = (name.as_str(), description.as_deref());
        let uuid = tidal_mutations::create_playlist_snapshot(&client, name, description, &tidal_ids)
            .await
            .map_err(tidal_write_error)?;
        let persisted = state
            .read()
            .await
            .db
            .with_conn(|conn| persist_tidal_snapshot(conn, name, description, &uuid, &track_ids));
        let playlist = match persisted {
            Ok(playlist) => playlist,
            Err(error) => {
                if let Err(cleanup) = tidal_mutations::delete_playlist(&client, &uuid).await {
                    tracing::error!(%uuid, %cleanup, "Failed to clean up snapshot after local save failed");
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(
                            json!({ "message": format!("Local save failed: {error}. The TIDAL playlist {uuid} still exists; sync your library before retrying") }),
                        ),
                    ));
                }
                return Err(playlist_error_response(error));
            }
        };
        Ok::<_, (StatusCode, Json<Value>)>(playlist)
    })
    .await
    .map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "message": format!("Snapshot save task failed: {error}") })),
        )
    })??;
    notify_playlists_changed(state).await;
    Ok(Json(json!({ "playlist": playlist })))
}

pub(super) fn persist_tidal_snapshot(
    conn: &rusqlite::Connection,
    name: &str,
    description: Option<&str>,
    uuid: &str,
    track_ids: &[i64],
) -> anyhow::Result<crate::db::models::Playlist> {
    let tx = conn.unchecked_transaction()?;
    let playlist = queries::create_playlist(&tx, name, description)?;
    tx.execute(
        "UPDATE playlists SET tidal_uuid = ?2, is_synced = 1, track_count = ?3 WHERE id = ?1",
        rusqlite::params![playlist.id, uuid, track_ids.len() as i64],
    )?;
    // A snapshot keeps repeated occurrences instead of using the deduping add helper.
    for (position, id) in track_ids.iter().enumerate() {
        tx.execute(
            "INSERT INTO playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
            rusqlite::params![playlist.id, id, position as i64],
        )?;
    }
    let saved = queries::get_playlist(&tx, playlist.id)?
        .ok_or_else(|| anyhow::anyhow!("Saved playlist missing"))?;
    tx.commit()?;
    Ok(saved)
}

/// Rename a playlist and/or replace its description.
///
/// For a TIDAL-mirrored playlist the remote write happens first: if it fails,
/// nothing changes locally, so the two never silently diverge. Doing it the
/// other way round would let the next sync quietly revert the user's edit.
pub(super) async fn update_playlist_route(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(payload): Json<UpdatePlaylistRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_positive_playlist_id_json(id)?;
    let name = require_playlist_name(&payload.name)?;
    let description = payload
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty());

    let playlist = load_playlist(&state, id).await?;
    if let Some(uuid) = playlist.tidal_uuid.as_deref() {
        let client = tidal_write_context(&state).await.ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "message": "Connect TIDAL to edit a synced playlist" })),
            )
        })?;
        tidal_mutations::rename_playlist(&client, uuid, &name, description)
            .await
            .map_err(tidal_write_error)?;
    }

    let response = {
        let guard = state.read().await;
        guard
            .db
            .with_conn(|conn| {
                let playlist = queries::rename_playlist(conn, id, &name, description)?;
                Ok(Json(json!({ "playlist": playlist })))
            })
            .map_err(playlist_error_response)?
    };
    notify_playlists_changed(&state).await;
    Ok(response)
}

/// Delete any playlist: regular, smart, or TIDAL-mirrored.
///
/// A TIDAL-mirrored playlist is deleted on TIDAL first. Deleting it only
/// locally would look like it worked and then have the next sync bring it back.
pub(super) async fn delete_playlist_route(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_positive_playlist_id_json(id)?;

    let playlist = load_playlist(&state, id).await?;
    if let Some(uuid) = playlist.tidal_uuid.as_deref() {
        let client = tidal_write_context(&state).await.ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "message": "Connect TIDAL to delete a synced playlist" })),
            )
        })?;
        tidal_mutations::delete_playlist(&client, uuid)
            .await
            .map_err(tidal_write_error)?;
    }

    let response = {
        let guard = state.read().await;
        guard
            .db
            .with_conn(|conn| {
                queries::delete_playlist(conn, id)?;
                Ok(Json(json!({ "deleted": true })))
            })
            .map_err(playlist_error_response)?
    };
    notify_playlists_changed(&state).await;
    Ok(response)
}

/// Remove tracks from a playlist by position, closing the resulting gaps.
pub(super) async fn remove_playlist_tracks_route(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(payload): Json<RemovePlaylistTracksRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_positive_playlist_id_json(id)?;
    if payload.positions.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "No positions given" })),
        ));
    }
    if payload.positions.iter().any(|position| *position < 0) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Positions must not be negative" })),
        ));
    }

    let playlist = load_playlist(&state, id).await?;
    if playlist.is_smart {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "message": "A smart playlist's contents come from its rules. Edit the rules instead."
            })),
        ));
    }
    if let Some(uuid) = playlist.tidal_uuid.as_deref() {
        let client = tidal_write_context(&state).await.ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "message": "Connect TIDAL to edit a synced playlist" })),
            )
        })?;
        let (map, remote) = locate_on_tidal(&state, &client, uuid, id).await?;
        // Positions past the end are ignored, as the local removal ignores them.
        let indices: Vec<usize> = payload
            .positions
            .iter()
            .filter_map(|position| map.get(*position as usize).copied())
            .collect();
        tidal_mutations::remove_playlist_items(&client, uuid, &indices, &remote.etag)
            .await
            .map_err(tidal_write_error)?;
    }

    let response = {
        let guard = state.read().await;
        guard
            .db
            .with_conn(|conn| {
                let removed = queries::remove_playlist_positions(conn, id, &payload.positions)?;
                Ok(Json(json!({ "removed": removed })))
            })
            .map_err(playlist_error_response)?
    };
    notify_playlists_changed(&state).await;
    Ok(response)
}

/// Move a track within a playlist. `to` is the destination index measured after
/// the moved row has been lifted out, matching the queue's move endpoint.
pub(super) async fn move_playlist_track_route(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(payload): Json<MovePlaylistTrackRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_positive_playlist_id_json(id)?;
    if payload.from < 0 || payload.to < 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Positions must not be negative" })),
        ));
    }

    let playlist = load_playlist(&state, id).await?;
    if playlist.is_smart {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "A smart playlist's order comes from its rules." })),
        ));
    }
    if let Some(uuid) = playlist.tidal_uuid.as_deref() {
        let client = tidal_write_context(&state).await.ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "message": "Connect TIDAL to edit a synced playlist" })),
            )
        })?;
        let (map, remote) = locate_on_tidal(&state, &client, uuid, id).await?;
        let from = payload.from as usize;
        if from >= map.len() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": "That position is not in the playlist" })),
            ));
        }
        let to = tidal_move_target(&map, from, payload.to as usize);
        tidal_mutations::move_playlist_item(&client, uuid, map[from], to, &remote.etag)
            .await
            .map_err(tidal_write_error)?;
    }

    let response = {
        let guard = state.read().await;
        guard
            .db
            .with_conn(|conn| {
                queries::move_playlist_track(conn, id, payload.from, payload.to)?;
                let tracks = queries::get_playlist_tracks(conn, id)?;
                Ok(Json(json!({ "tracks": tracks })))
            })
            .map_err(playlist_error_response)?
    };
    notify_playlists_changed(&state).await;
    Ok(response)
}

/// Re-pull one playlist's tracks from TIDAL right now.
///
/// The escape hatch for when the sync's change-detection heuristic
/// (`playlist_needs_pull`) guesses wrong, and the fastest way to see an edit
/// made in the TIDAL app without waiting for a sync.
pub(super) async fn refresh_playlist_route(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_positive_playlist_id_json(id)?;

    let playlist = load_playlist(&state, id).await?;
    let Some(uuid) = playlist.tidal_uuid.clone() else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "This playlist is not synced from TIDAL" })),
        ));
    };
    let client = tidal_write_context(&state).await.ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "message": "Connect TIDAL to refresh a synced playlist" })),
        )
    })?;
    let client = client.with_metadata_store(state.read().await.db.clone());
    let never_cancelled = || -> anyhow::Result<()> { Ok(()) };
    let tracks =
        super::tidal_sync_routes::fetch_tidal_playlist_tracks(&client, &uuid, &never_cancelled)
            .await
            .map_err(tidal_write_error)?;

    let response = {
        let guard = state.read().await;
        guard
            .db
            .with_conn(|conn| {
                let count = super::tidal_sync_routes::replace_playlist_tracks(conn, id, &tracks)?;
                Ok(Json(json!({ "tracks": count })))
            })
            .map_err(playlist_error_response)?
    };
    notify_playlists_changed(&state).await;
    Ok(response)
}

pub(super) async fn get_playlists(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let mut playlists = queries::get_playlists(conn)?;

            // Count smart playlists - if none, skip expensive loading.
            let smart_count = playlists.iter().filter(|p| p.is_smart).count();
            if smart_count > 0 {
                // Load all data once, build a shared context.
                let tracks = queries::get_all_tracks(conn)?;
                let context = build_smart_playlist_context(conn)?;

                for playlist in &mut playlists {
                    if playlist.is_smart {
                        let tracks = resolve_smart_playlist_tracks_with_context(
                            playlist, &tracks, &context,
                        )?;
                        playlist.track_count = tracks.len() as i32;
                    }
                }
            }

            Ok(Json(json!({ "playlists": playlists })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub(super) async fn get_playlist_tracks(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    require_positive_playlist_id(id)?;

    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let playlist = queries::get_playlist(conn, id)?
                .ok_or_else(|| anyhow::anyhow!("playlist not found"))?;
            let tracks = if playlist.is_smart {
                resolve_smart_playlist_tracks(conn, &playlist)?
            } else {
                queries::get_playlist_tracks(conn, id)?
            };
            Ok(Json(json!({ "tracks": tracks })))
        })
        .map_err(playlist_error_status)
}

pub(super) async fn toggle_playlist_favorite_route(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_positive_playlist_id_json(id)?;

    let response = {
        let guard = state.read().await;
        guard
            .db
            .with_conn(|conn| {
                let playlist = queries::toggle_playlist_favorite(conn, id)?;
                Ok(Json(json!({ "playlist": playlist })))
            })
            .map_err(playlist_error_response)?
    };
    notify_playlists_changed(&state).await;
    Ok(response)
}

pub(super) async fn add_tracks_to_playlist_route(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(payload): Json<AddTracksToPlaylistRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_positive_playlist_id_json(id)?;
    require_positive_track_ids(&payload.track_ids)?;
    let outcome = add_tracks_mirrored(&state, id, &payload.track_ids).await?;
    Ok(Json(json!({
        "added": outcome.added,
        "skipped_not_on_tidal": outcome.skipped_not_on_tidal,
    })))
}

pub(super) struct PlaylistAddOutcome {
    pub(super) added: usize,
    pub(super) resolved: usize,
    pub(super) skipped_not_on_tidal: usize,
}

/// The single add path behind every endpoint. A TIDAL-linked playlist is
/// written on TIDAL first and keeps only the tracks TIDAL can hold; a local
/// playlist takes everything.
pub(super) async fn add_tracks_mirrored(
    state: &SharedState,
    playlist_id: i64,
    track_ids: &[i64],
) -> Result<PlaylistAddOutcome, (StatusCode, Json<Value>)> {
    let playlist = load_playlist(state, playlist_id).await?;
    if playlist.is_smart {
        // Contents come from its rules; a direct add would vanish on the next
        // evaluation.
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "A smart playlist's contents come from its rules" })),
        ));
    }
    let local_ids: Vec<i64> = match playlist.tidal_uuid.as_deref() {
        Some(uuid) => {
            // Only the tracks the local add will insert. A track already here can
            // map to a different TIDAL release than the copy TIDAL holds, and
            // TIDAL's own duplicate check would then add it remotely only.
            let (new_count, pairs): (usize, Vec<(i64, i64)>) = state
                .read()
                .await
                .db
                .with_conn(|conn| {
                    let new_ids = queries::new_playlist_track_ids(conn, playlist_id, track_ids)?;
                    let pairs = queries::get_track_tidal_ids(conn, &new_ids)?
                        .into_iter()
                        .filter(|(_, tidal_id)| *tidal_id > 0)
                        .collect();
                    Ok((new_ids.len(), pairs))
                })
                .map_err(playlist_error_response)?;
            if new_count == 0 {
                // Everything is already here: nothing to send TIDAL.
                return Ok(PlaylistAddOutcome {
                    added: 0,
                    resolved: 0,
                    skipped_not_on_tidal: 0,
                });
            }
            if pairs.is_empty() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "message": "None of these tracks are available on TIDAL" })),
                ));
            }
            let client = tidal_write_context(state).await.ok_or_else(|| {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({ "message": "Connect TIDAL to add to this playlist" })),
                )
            })?;
            let tidal_ids: Vec<i64> = pairs.iter().map(|(_, tidal_id)| *tidal_id).collect();
            tidal_mutations::add_to_playlist(&client, uuid, &tidal_ids)
                .await
                .map_err(tidal_write_error)?;
            pairs.iter().map(|(track_id, _)| *track_id).collect()
        }
        None => track_ids.to_vec(),
    };
    let added = state
        .read()
        .await
        .db
        .with_conn(|conn| queries::add_tracks_to_playlist(conn, playlist_id, &local_ids))
        .map_err(playlist_error_response)?;
    notify_playlists_changed(state).await;
    Ok(PlaylistAddOutcome {
        added,
        resolved: local_ids.len(),
        skipped_not_on_tidal: track_ids.len() - local_ids.len(),
    })
}

pub(super) async fn evaluate_smart_playlist(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    require_positive_playlist_id(id)?;

    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let playlist = queries::get_playlist(conn, id)?
                .ok_or_else(|| anyhow::anyhow!("playlist not found"))?;
            let tracks = if playlist.is_smart {
                resolve_smart_playlist_tracks(conn, &playlist)?
            } else {
                queries::get_playlist_tracks(conn, id)?
            };
            Ok(Json(json!({
                "playlist": playlist,
                "tracks": tracks,
                "resolved_count": tracks.len()
            })))
        })
        .map_err(playlist_error_status)
}

/// Up to four distinct album-artwork URLs for the cover mosaic on /playlists.
/// Regular playlists return without scanning every track. Smart playlists
/// still need to evaluate their rules, but the response payload is four
/// short strings instead of the entire track list.
pub(super) async fn get_playlist_cover_sample(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    require_positive_playlist_id(id)?;

    const COVER_SAMPLE_LIMIT: i64 = 4;
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let playlist = queries::get_playlist(conn, id)?
                .ok_or_else(|| anyhow::anyhow!("playlist not found"))?;
            let urls = if playlist.is_smart {
                let tracks = resolve_smart_playlist_tracks(conn, &playlist)?;
                unique_artwork_urls(&tracks, COVER_SAMPLE_LIMIT as usize)
            } else {
                queries::sample_playlist_artwork(conn, id, COVER_SAMPLE_LIMIT)?
            };
            Ok(Json(json!({ "urls": urls })))
        })
        .map_err(playlist_error_status)
}

fn unique_artwork_urls(tracks: &[crate::db::models::Track], limit: usize) -> Vec<String> {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::with_capacity(limit);
    for track in tracks {
        let Some(url) = track.artwork_url.as_deref() else {
            continue;
        };
        if seen.insert(url) {
            out.push(url.to_string());
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}

/// Live "matches N tracks" preview for the smart-playlist editor. Accepts a
/// rules body identical to create/update but never touches the database -
/// just evaluates and counts.
pub(super) async fn preview_smart_playlist(
    State(state): State<SharedState>,
    Json(payload): Json<PreviewSmartPlaylistRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let definition = SmartPlaylistDefinition {
        name: payload.name.unwrap_or_default(),
        description: payload.description,
        root: serde_json::from_value(payload.rules).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": format!("Invalid rules: {e}") })),
            )
        })?,
    };
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let tracks = queries::get_all_tracks(conn)?;
            let context = build_smart_playlist_context(conn)?;
            let count =
                crate::smart::playlists::evaluate_playlist(&definition, &tracks, &context).len();
            Ok(Json(json!({ "count": count })))
        })
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": e.to_string() })),
            )
        })
}

/// Lightweight artist autocomplete - returns `{ id, name }` pairs only.
/// Powers the smart-playlist editor's artist tag input.
pub(super) async fn search_artists_route(
    State(state): State<SharedState>,
    axum::extract::Query(params): axum::extract::Query<ArtistSearchParams>,
) -> Result<Json<Value>, StatusCode> {
    let query = params.q.unwrap_or_default().trim().to_string();
    if query.is_empty() {
        return Ok(empty_artist_search_response());
    }
    let limit = clamp_artist_search_limit(params.limit);
    let state = state.read().await;
    let results: Vec<(i64, String)> = state
        .db
        .with_conn(|conn| queries::search_library_artist_names(conn, &query, limit))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let artists: Vec<Value> = results
        .into_iter()
        .map(|(id, name)| json!({ "id": id, "name": name }))
        .collect();
    Ok(Json(json!({ "artists": artists })))
}

fn empty_artist_search_response() -> Json<Value> {
    Json(json!({ "artists": [] }))
}

fn clamp_artist_search_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(20).clamp(1, 50)
}

pub(super) async fn create_smart_playlist_route(
    State(state): State<SharedState>,
    Json(payload): Json<CreateSmartPlaylistRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let name = payload.name.trim().to_string();
    if name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Playlist name must not be empty" })),
        ));
    }

    // Validate the rules JSON by deserialising into SmartPlaylistDefinition.
    let definition = SmartPlaylistDefinition {
        name: name.clone(),
        description: payload.description.clone(),
        root: serde_json::from_value(payload.rules.clone()).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": format!("Invalid rules: {e}") })),
            )
        })?,
    };
    let rules_json = serde_json::to_string(&definition).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "message": "Failed to serialise rules" })),
        )
    })?;

    let response = {
        let guard = state.read().await;
        guard
            .db
            .with_conn(|conn| {
                let playlist = queries::create_smart_playlist(
                    conn,
                    &name,
                    payload.description.as_deref(),
                    &rules_json,
                )?;
                Ok(Json(json!({ "playlist": playlist })))
            })
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "message": e.to_string() })),
                )
            })?
    };
    notify_playlists_changed(&state).await;
    Ok(response)
}

pub(super) async fn update_smart_playlist_route(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(payload): Json<UpdateSmartPlaylistRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_positive_playlist_id_json(id)?;

    let name = payload.name.trim().to_string();
    if name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Playlist name must not be empty" })),
        ));
    }

    let definition = SmartPlaylistDefinition {
        name: name.clone(),
        description: payload.description.clone(),
        root: serde_json::from_value(payload.rules.clone()).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": format!("Invalid rules: {e}") })),
            )
        })?,
    };
    let rules_json = serde_json::to_string(&definition).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "message": "Failed to serialise rules" })),
        )
    })?;

    let response = {
        let guard = state.read().await;
        guard
            .db
            .with_conn(|conn| {
                let playlist = queries::update_smart_playlist(
                    conn,
                    id,
                    &name,
                    payload.description.as_deref(),
                    &rules_json,
                )?;
                Ok(Json(json!({ "playlist": playlist })))
            })
            .map_err(playlist_error_response)?
    };
    notify_playlists_changed(&state).await;
    Ok(response)
}

pub(super) async fn delete_smart_playlist_route(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_positive_playlist_id_json(id)?;

    let response = {
        let guard = state.read().await;
        guard
            .db
            .with_conn(|conn| {
                queries::delete_smart_playlist(conn, id)?;
                Ok(Json(json!({ "deleted": true })))
            })
            .map_err(playlist_error_response)?
    };
    notify_playlists_changed(&state).await;
    Ok(response)
}

fn resolve_smart_playlist_tracks_with_context(
    playlist: &crate::db::models::Playlist,
    tracks: &[crate::db::models::Track],
    context: &PlaylistEvaluationContext,
) -> anyhow::Result<Vec<crate::db::models::Track>> {
    let Some(raw_rules) = playlist.smart_rules.as_deref() else {
        return Ok(Vec::new());
    };

    let definition: SmartPlaylistDefinition = serde_json::from_str(raw_rules)?;
    let resolved = evaluate_playlist(&definition, tracks, context)
        .into_iter()
        .cloned()
        .collect();
    Ok(resolved)
}

/// Build a fully-populated evaluation context (genres, playlist memberships, DSP features,
/// sample-match sources). All smart-playlist rule types can evaluate against this.
fn build_smart_playlist_context(
    conn: &rusqlite::Connection,
) -> anyhow::Result<PlaylistEvaluationContext> {
    // Smart-playlist genre rules are inclusion-only - the RuleClause enum has
    // no genre-negation variant (only NotInPlaylist). So fallback genres can
    // safely apply to every rule. If a Genre-negation primitive is added
    // later, this is the place to split into literal vs. fallback contexts.
    let genre_map = queries::get_track_genre_paths_with_fallback(conn)?;
    let playlist_memberships = queries::get_playlist_memberships(conn)?;
    let dsp_rows = queries::get_all_audio_dsp_features(conn)?;
    let fingerprint_ids = queries::get_track_ids_with_fingerprint(conn)?;

    let mut context = PlaylistEvaluationContext::new();
    for (track_id, rows) in genre_map {
        context = context.with_track_genres(track_id, queries::ResolvedGenre::paths_only(&rows));
    }
    for (playlist_id, track_ids) in playlist_memberships {
        context = context.with_playlist_tracks(playlist_id, track_ids);
    }
    for (track_id, bpm, key_signature, camelot_key, energy, danceability, is_instrumental) in
        dsp_rows
    {
        context = context.with_track_dsp(
            track_id,
            TrackDspFeatures {
                bpm,
                key_signature,
                camelot_key,
                energy,
                danceability,
                is_instrumental,
            },
        );
    }
    for track_id in fingerprint_ids {
        context = context.with_sample_source(track_id, "fingerprint");
    }
    Ok(context)
}

fn resolve_smart_playlist_tracks(
    conn: &rusqlite::Connection,
    playlist: &crate::db::models::Playlist,
) -> anyhow::Result<Vec<crate::db::models::Track>> {
    let tracks = queries::get_all_tracks(conn)?;
    let context = build_smart_playlist_context(conn)?;
    resolve_smart_playlist_tracks_with_context(playlist, &tracks, &context)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_artist_search_response_keeps_route_payload_shape() {
        let Json(body) = empty_artist_search_response();
        assert_eq!(body, json!({ "artists": [] }));
    }

    #[test]
    fn artist_search_limit_is_bounded() {
        assert_eq!(clamp_artist_search_limit(None), 20);
        assert_eq!(clamp_artist_search_limit(Some(-5)), 1);
        assert_eq!(clamp_artist_search_limit(Some(0)), 1);
        assert_eq!(clamp_artist_search_limit(Some(5_000)), 50);
    }
}
