use crate::SharedState;
use crate::db::queries;
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
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub(super) async fn toggle_playlist_favorite_route(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let playlist = queries::toggle_playlist_favorite(conn, id)?;
            Ok(Json(json!({ "playlist": playlist })))
        })
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })
}

pub(super) async fn add_tracks_to_playlist_route(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(payload): Json<AddTracksToPlaylistRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let added = queries::add_tracks_to_playlist(conn, id, &payload.track_ids)?;
            Ok(Json(json!({ "added": added })))
        })
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })
}

pub(super) async fn evaluate_smart_playlist(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
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
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Up to four distinct album-artwork URLs for the cover mosaic on /playlists.
/// Regular playlists return without scanning every track. Smart playlists
/// still need to evaluate their rules, but the response payload is four
/// short strings instead of the entire track list.
pub(super) async fn get_playlist_cover_sample(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
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
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
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
    let query = params.q.unwrap_or_default();
    let limit = params.limit.unwrap_or(20).clamp(1, 50);
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

    let state = state.read().await;
    state
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
        })
}

pub(super) async fn update_smart_playlist_route(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(payload): Json<UpdateSmartPlaylistRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
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

    let state = state.read().await;
    state
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
        .map_err(|e| {
            let status = if e.to_string().contains("not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, Json(json!({ "message": e.to_string() })))
        })
}

pub(super) async fn delete_smart_playlist_route(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            queries::delete_smart_playlist(conn, id)?;
            Ok(Json(json!({ "deleted": true })))
        })
        .map_err(|e| {
            let status = if e.to_string().contains("not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, Json(json!({ "message": e.to_string() })))
        })
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
    let acrcloud_ids = queries::get_track_ids_with_acrcloud_match(conn)?;
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
    for track_id in acrcloud_ids {
        context = context.with_sample_source(track_id, "acrcloud");
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
