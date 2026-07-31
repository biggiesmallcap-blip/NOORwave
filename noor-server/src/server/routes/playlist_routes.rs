use crate::SharedState;
use crate::db::queries;
use crate::services::tidal::auth::TidalTokens;
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
fn tidal_write_error(error: anyhow::Error) -> (StatusCode, Json<Value>) {
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

/// The TIDAL credentials needed for a playlist write, or `None` when TIDAL is
/// not connected. A local-only playlist never needs these.
async fn tidal_write_context(state: &SharedState) -> Option<(reqwest::Client, TidalTokens)> {
    let state = state.read().await;
    let tokens = state.tidal_tokens.clone()?;
    Some((state.http_client.clone(), tokens))
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

/// Create a regular, local playlist.
///
/// Local-only on purpose: this does not create a counterpart on TIDAL, so the
/// list stays a NOORwave artifact. Editing an existing TIDAL-mirrored playlist
/// does write through - see `update_playlist_route` below.
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
        let (http, tokens) = tidal_write_context(&state).await.ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "message": "Connect TIDAL to edit a synced playlist" })),
            )
        })?;
        tidal_mutations::rename_playlist(
            &http,
            &tokens.access_token,
            uuid,
            &name,
            description,
            &tokens.country_code,
        )
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
        let (http, tokens) = tidal_write_context(&state).await.ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "message": "Connect TIDAL to delete a synced playlist" })),
            )
        })?;
        tidal_mutations::delete_playlist(&http, &tokens.access_token, uuid, &tokens.country_code)
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
        let (http, tokens) = tidal_write_context(&state).await.ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "message": "Connect TIDAL to edit a synced playlist" })),
            )
        })?;
        tidal_mutations::remove_playlist_items(
            &http,
            &tokens.access_token,
            uuid,
            &payload.positions,
            &tokens.country_code,
        )
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
        let (http, tokens) = tidal_write_context(&state).await.ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "message": "Connect TIDAL to edit a synced playlist" })),
            )
        })?;
        tidal_mutations::move_playlist_item(
            &http,
            &tokens.access_token,
            uuid,
            payload.from,
            payload.to,
            &tokens.country_code,
        )
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
    let (http, tokens) = tidal_write_context(&state).await.ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "message": "Connect TIDAL to refresh a synced playlist" })),
        )
    })?;

    let client = crate::services::tidal::client::TidalClient::with_http(
        http,
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
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

    let response = {
        let guard = state.read().await;
        guard
            .db
            .with_conn(|conn| {
                queries::get_playlist(conn, id)?
                    .ok_or_else(|| anyhow::anyhow!("playlist not found"))?;
                let added = queries::add_tracks_to_playlist(conn, id, &payload.track_ids)?;
                Ok(Json(json!({ "added": added })))
            })
            .map_err(playlist_error_response)?
    };
    notify_playlists_changed(&state).await;
    Ok(response)
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
