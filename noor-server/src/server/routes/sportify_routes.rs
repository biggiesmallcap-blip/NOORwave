use crate::SharedState;
use crate::db::queries;
use crate::services::tidal::import as tidal_import;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
pub(super) struct SportifySearchQuery {
    q: String,
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    offset: Option<u32>,
}

fn parse_search_kind(s: Option<&str>) -> crate::services::sportify::client::SportifySearchKind {
    use crate::services::sportify::client::SportifySearchKind;
    match s.map(str::to_ascii_lowercase).as_deref() {
        Some("album") => SportifySearchKind::Album,
        Some("artist") => SportifySearchKind::Artist,
        Some("playlist") => SportifySearchKind::Playlist,
        // Default to track when missing or unknown: matches Sportify's own
        // most-common usage.
        _ => SportifySearchKind::Track,
    }
}

pub(super) async fn sportify_discovery_search(
    State(state): State<SharedState>,
    Query(params): Query<SportifySearchQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use crate::services::sportify::{cache as sp_cache, normalize};

    let q = params.q.trim();
    if q.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "q required" })),
        ));
    }

    let kind = parse_search_kind(params.r#type.as_deref());
    let limit = params.limit.unwrap_or(20).clamp(1, 50);
    let offset = params.offset.unwrap_or(0);

    let (sportify_client, cache_cfg, db) = {
        let s = state.read().await;
        (
            s.sportify_client.clone(),
            s.sportify_cache_config,
            s.db.clone(),
        )
    };
    let Some(sportify_client) = sportify_client else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "sportify_unavailable" })),
        ));
    };

    let cached = db
        .with_conn(|conn| sp_cache::get_search(conn, &cache_cfg, q, kind, limit, offset))
        .map_err(super::internal)?;

    let payload = match cached {
        Some(p) => p,
        None => {
            let fetched = sportify_client
                .search(q, kind, limit, offset)
                .await
                .map_err(|e| {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({ "error": format!("sportify_search: {e}") })),
                    )
                })?;
            db.with_conn(|conn| sp_cache::put_search(conn, q, kind, limit, offset, &fetched))
                .map_err(super::internal)?;
            fetched
        }
    };

    let mut normalized = normalize::search_from_sportify(&payload, "sportify_search");
    db.with_conn(|conn| {
        normalize::enrich_tracks_with_tidal_cache(conn, &cache_cfg, &mut normalized.tracks)?;
        for album in normalized.albums.iter_mut() {
            normalize::enrich_tracks_with_tidal_cache(conn, &cache_cfg, &mut album.tracks)?;
        }
        for playlist in normalized.playlists.iter_mut() {
            normalize::enrich_tracks_with_tidal_cache(conn, &cache_cfg, &mut playlist.tracks)?;
        }
        Ok(())
    })
    .map_err(super::internal)?;

    Ok(Json(serde_json::to_value(normalized).unwrap_or(json!({}))))
}

pub(super) async fn sportify_discovery_track(
    State(state): State<SharedState>,
    Path(spotify_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use crate::services::sportify::{cache as sp_cache, normalize};

    let id = spotify_id.trim();
    if id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "spotify_id required" })),
        ));
    }

    let (sportify_client, cache_cfg, db) = {
        let s = state.read().await;
        (
            s.sportify_client.clone(),
            s.sportify_cache_config,
            s.db.clone(),
        )
    };
    let Some(sportify_client) = sportify_client else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "sportify_unavailable" })),
        ));
    };

    let track = match db
        .with_conn(|conn| sp_cache::get_track_meta(conn, &cache_cfg, id))
        .map_err(super::internal)?
    {
        Some(t) => t,
        None => {
            let fetched = sportify_client.track(id).await.map_err(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": format!("sportify_track_fetch: {e}") })),
                )
            })?;
            db.with_conn(|conn| {
                sp_cache::put_track_meta(conn, id, &fetched)?;
                crate::services::sportify::stats::write_track_playcount(conn, &fetched);
                Ok::<_, anyhow::Error>(())
            })
            .map_err(super::internal)?;
            fetched
        }
    };

    let mut row = normalize::track_from_sportify(&track, "sportify_track");
    db.with_conn(|conn| {
        normalize::enrich_tracks_with_tidal_cache(conn, &cache_cfg, std::slice::from_mut(&mut row))
    })
    .map_err(super::internal)?;

    Ok(Json(serde_json::to_value(row).unwrap_or(json!({}))))
}

pub(super) async fn sportify_discovery_album(
    State(state): State<SharedState>,
    Path(spotify_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use crate::services::sportify::{cache as sp_cache, normalize};

    let id = spotify_id.trim();
    if id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "spotify_id required" })),
        ));
    }

    let (sportify_client, cache_cfg, db) = {
        let s = state.read().await;
        (
            s.sportify_client.clone(),
            s.sportify_cache_config,
            s.db.clone(),
        )
    };
    let Some(sportify_client) = sportify_client else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "sportify_unavailable" })),
        ));
    };

    let album = match db
        .with_conn(|conn| sp_cache::get_album_meta(conn, &cache_cfg, id))
        .map_err(super::internal)?
    {
        Some(a) => a,
        None => {
            let fetched = sportify_client.album(id).await.map_err(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": format!("sportify_album_fetch: {e}") })),
                )
            })?;
            db.with_conn(|conn| {
                sp_cache::put_album_meta(conn, id, &fetched)?;
                crate::services::sportify::stats::write_track_playcounts(conn, &fetched.tracks);
                Ok::<_, anyhow::Error>(())
            })
            .map_err(super::internal)?;
            fetched
        }
    };

    let mut row = normalize::album_from_sportify(&album, "sportify_album");
    let pending_ids = super::eager_and_lazy_resolve_for_list(&state, &album.tracks).await;
    db.with_conn(|conn| {
        normalize::enrich_tracks_with_tidal_cache(conn, &cache_cfg, &mut row.tracks)
    })
    .map_err(super::internal)?;

    Ok(Json(json!({
        "album": serde_json::to_value(row).unwrap_or(json!({})),
        "pendingSpotifyIds": pending_ids,
    })))
}

pub(super) async fn sportify_discovery_playlist(
    State(state): State<SharedState>,
    Path(spotify_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use crate::services::sportify::{cache as sp_cache, normalize};

    let id = spotify_id.trim();
    if id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "spotify_id required" })),
        ));
    }

    let (sportify_client, cache_cfg, db) = {
        let s = state.read().await;
        (
            s.sportify_client.clone(),
            s.sportify_cache_config,
            s.db.clone(),
        )
    };
    let Some(sportify_client) = sportify_client else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "sportify_unavailable" })),
        ));
    };

    let playlist = match db
        .with_conn(|conn| sp_cache::get_playlist_meta(conn, &cache_cfg, id))
        .map_err(super::internal)?
    {
        Some(p) => p,
        None => {
            let fetched = sportify_client.playlist(id).await.map_err(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": format!("sportify_playlist_fetch: {e}") })),
                )
            })?;
            db.with_conn(|conn| {
                sp_cache::put_playlist_meta(conn, id, &fetched)?;
                crate::services::sportify::stats::write_track_playcounts(conn, &fetched.tracks);
                Ok::<_, anyhow::Error>(())
            })
            .map_err(super::internal)?;
            fetched
        }
    };

    let mut row = normalize::playlist_from_sportify(&playlist, "sportify_playlist");
    db.with_conn(|conn| {
        normalize::enrich_tracks_with_tidal_cache(conn, &cache_cfg, &mut row.tracks)
    })
    .map_err(super::internal)?;
    let pending_ids = super::spawn_background_resolve_for_list(&state, &playlist.tracks).await;

    Ok(Json(json!({
        "playlist": serde_json::to_value(row).unwrap_or(json!({})),
        "pendingSpotifyIds": pending_ids,
    })))
}

pub(super) async fn sportify_discovery_artist(
    State(state): State<SharedState>,
    Path(spotify_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use crate::services::sportify::{cache as sp_cache, normalize};

    let id = spotify_id.trim();
    if id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "spotify_id required" })),
        ));
    }

    let (sportify_client, cache_cfg, db) = {
        let s = state.read().await;
        (
            s.sportify_client.clone(),
            s.sportify_cache_config,
            s.db.clone(),
        )
    };
    let Some(sportify_client) = sportify_client else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "sportify_unavailable" })),
        ));
    };

    let artist = match db
        .with_conn(|conn| sp_cache::get_artist_meta(conn, &cache_cfg, id))
        .map_err(super::internal)?
    {
        Some(a) => a,
        None => {
            let fetched = sportify_client.artist(id).await.map_err(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": format!("sportify_artist_fetch: {e}") })),
                )
            })?;
            db.with_conn(|conn| {
                sp_cache::put_artist_meta(conn, id, &fetched)?;
                crate::services::sportify::stats::write_artist_monthly_listeners(conn, &fetched);
                Ok::<_, anyhow::Error>(())
            })
            .map_err(super::internal)?;
            fetched
        }
    };

    let row = normalize::artist_from_sportify(&artist);
    Ok(Json(serde_json::to_value(row).unwrap_or(json!({}))))
}

pub(super) async fn sportify_discovery_artist_top_tracks(
    State(state): State<SharedState>,
    Path(spotify_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use crate::services::sportify::{cache as sp_cache, normalize};

    let id = spotify_id.trim();
    if id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "spotify_id required" })),
        ));
    }

    let (sportify_client, cache_cfg, db) = {
        let s = state.read().await;
        (
            s.sportify_client.clone(),
            s.sportify_cache_config,
            s.db.clone(),
        )
    };
    let Some(sportify_client) = sportify_client else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "sportify_unavailable" })),
        ));
    };

    // Top tracks aren't cached as a unit (they're a derived list); rely on
    // the per-track meta cache to absorb repeat hits.
    let tracks = sportify_client.artist_top_tracks(id).await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": format!("sportify_top_tracks_fetch: {e}") })),
        )
    })?;

    db.with_conn(|conn| {
        for t in &tracks {
            if let Some(track_id) = t.id.as_deref() {
                let _ = sp_cache::put_track_meta(conn, track_id, t);
            }
        }
        crate::services::sportify::stats::write_track_playcounts(conn, &tracks);
        Ok::<_, anyhow::Error>(())
    })
    .map_err(super::internal)?;

    let mut rows: Vec<_> = tracks
        .iter()
        .map(|t| normalize::track_from_sportify(t, "sportify_artist_top_tracks"))
        .collect();
    let pending_ids = super::eager_and_lazy_resolve_for_list(&state, &tracks).await;
    db.with_conn(|conn| normalize::enrich_tracks_with_tidal_cache(conn, &cache_cfg, &mut rows))
        .map_err(super::internal)?;

    Ok(Json(json!({
        "spotifyId": id,
        "tracks": rows,
        "pendingSpotifyIds": pending_ids,
    })))
}

pub(super) async fn sportify_discovery_artist_related(
    State(state): State<SharedState>,
    Path(spotify_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use crate::services::sportify::{normalize, recommend};

    let id = spotify_id.trim();
    if id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "spotify_id required" })),
        ));
    }

    let (sportify_client, cache_cfg, db) = {
        let s = state.read().await;
        (
            s.sportify_client.clone(),
            s.sportify_cache_config,
            s.db.clone(),
        )
    };
    let Some(sportify_client) = sportify_client else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "sportify_unavailable" })),
        ));
    };

    let related = recommend::artist_related(&sportify_client, &db, &cache_cfg, id)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("sportify_related: {e}") })),
            )
        })?;

    // Track lists go through eager+lazy resolution; albums/artists carry no
    // tidal state since they're just navigation cards.
    let mut top_rows: Vec<_> = related
        .top_tracks
        .iter()
        .map(|t| normalize::track_from_sportify(t, "sportify_artist_related_top"))
        .collect();
    let mut deep_rows: Vec<_> = related
        .deep_cuts
        .iter()
        .map(|t| normalize::track_from_sportify(t, "sportify_artist_related_deep"))
        .collect();

    let mut pending = Vec::new();
    pending.extend(super::eager_and_lazy_resolve_for_list(&state, &related.top_tracks).await);
    pending.extend(super::eager_and_lazy_resolve_for_list(&state, &related.deep_cuts).await);

    db.with_conn(|conn| {
        normalize::enrich_tracks_with_tidal_cache(conn, &cache_cfg, &mut top_rows)?;
        normalize::enrich_tracks_with_tidal_cache(conn, &cache_cfg, &mut deep_rows)?;
        Ok::<_, anyhow::Error>(())
    })
    .map_err(super::internal)?;

    let recent_releases: Vec<_> = related
        .recent_releases
        .iter()
        .map(|a| normalize::album_from_sportify(a, "sportify_artist_related_recent"))
        .collect();
    let similar_artists: Vec<_> = related
        .similar_artists
        .iter()
        .map(normalize::artist_from_sportify)
        .collect();

    Ok(Json(json!({
        "spotifyId": id,
        "topTracks": top_rows,
        "deepCuts": deep_rows,
        "recentReleases": recent_releases,
        "similarArtists": similar_artists,
        "pendingSpotifyIds": pending,
    })))
}

pub(super) async fn sportify_discovery_album_related(
    State(state): State<SharedState>,
    Path(spotify_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use crate::services::sportify::{normalize, recommend};

    let id = spotify_id.trim();
    if id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "spotify_id required" })),
        ));
    }

    let (sportify_client, cache_cfg, db) = {
        let s = state.read().await;
        (
            s.sportify_client.clone(),
            s.sportify_cache_config,
            s.db.clone(),
        )
    };
    let Some(sportify_client) = sportify_client else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "sportify_unavailable" })),
        ));
    };

    let related = recommend::album_related(&sportify_client, &db, &cache_cfg, id)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("sportify_related: {e}") })),
            )
        })?;

    let mut more_from_artist: Vec<_> = related
        .more_from_artist
        .iter()
        .map(|t| normalize::track_from_sportify(t, "sportify_album_related"))
        .collect();
    let pending = super::eager_and_lazy_resolve_for_list(&state, &related.more_from_artist).await;

    db.with_conn(|conn| {
        normalize::enrich_tracks_with_tidal_cache(conn, &cache_cfg, &mut more_from_artist)
    })
    .map_err(super::internal)?;

    let more_albums_by_artist: Vec<_> = related
        .more_albums_by_artist
        .iter()
        .map(|a| normalize::album_from_sportify(a, "sportify_album_related_albums"))
        .collect();

    Ok(Json(json!({
        "spotifyId": id,
        "moreFromArtist": more_from_artist,
        "moreAlbumsByArtist": more_albums_by_artist,
        "pendingSpotifyIds": pending,
    })))
}

pub(super) async fn sportify_discovery_track_related(
    State(state): State<SharedState>,
    Path(spotify_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use crate::services::sportify::{normalize, recommend};

    let id = spotify_id.trim();
    if id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "spotify_id required" })),
        ));
    }

    let (sportify_client, cache_cfg, db) = {
        let s = state.read().await;
        (
            s.sportify_client.clone(),
            s.sportify_cache_config,
            s.db.clone(),
        )
    };
    let Some(sportify_client) = sportify_client else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "sportify_unavailable" })),
        ));
    };

    let related = recommend::track_related(&sportify_client, &db, &cache_cfg, id)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("sportify_related: {e}") })),
            )
        })?;

    let mut more_from_album: Vec<_> = related
        .more_from_album
        .iter()
        .map(|t| normalize::track_from_sportify(t, "sportify_track_related_album"))
        .collect();
    let mut more_from_artist: Vec<_> = related
        .more_from_artist
        .iter()
        .map(|t| normalize::track_from_sportify(t, "sportify_track_related_artist"))
        .collect();

    let mut pending = Vec::new();
    pending.extend(super::eager_and_lazy_resolve_for_list(&state, &related.more_from_album).await);
    pending.extend(super::eager_and_lazy_resolve_for_list(&state, &related.more_from_artist).await);

    db.with_conn(|conn| {
        normalize::enrich_tracks_with_tidal_cache(conn, &cache_cfg, &mut more_from_album)?;
        normalize::enrich_tracks_with_tidal_cache(conn, &cache_cfg, &mut more_from_artist)?;
        Ok::<_, anyhow::Error>(())
    })
    .map_err(super::internal)?;

    Ok(Json(json!({
        "spotifyId": id,
        "moreFromAlbum": more_from_album,
        "moreFromArtist": more_from_artist,
        "pendingSpotifyIds": pending,
    })))
}

#[derive(Debug, Deserialize)]
pub(super) struct SaveSpotifyPlaylistBody {
    spotify_id: String,
    /// Override for the noor playlist name. Defaults to the Sportify
    /// playlist title.
    #[serde(default)]
    name: Option<String>,
}

/// Save an ephemeral Spotify-sourced playlist into the user's library.
///
/// Pre-condition: the playlist's tracks have been bulk-resolved against
/// TIDAL (the ephemeral view does this on open). Tracks without a cached
/// resolution are skipped. We never invent a placeholder TIDAL id.
pub(super) async fn save_spotify_playlist(
    State(state): State<SharedState>,
    Json(body): Json<SaveSpotifyPlaylistBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use crate::services::sportify::{cache as sp_cache, recommend};

    let id = body.spotify_id.trim();
    if id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "spotify_id required" })),
        ));
    }

    let (sportify_client, cache_cfg, db) = {
        let s = state.read().await;
        (
            s.sportify_client.clone(),
            s.sportify_cache_config,
            s.db.clone(),
        )
    };
    let Some(sportify_client) = sportify_client else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "sportify_unavailable" })),
        ));
    };

    let playlist = recommend::cached_playlist(&sportify_client, &db, &cache_cfg, id)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("sportify_playlist_fetch: {e}") })),
            )
        })?;

    let playlist_name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| playlist.name.clone())
        .unwrap_or_else(|| "Spotify playlist".to_string());

    // Pull cached TIDAL resolutions for the playlist's tracks. Tracks without
    // a cached hit are skipped here. The frontend should bulk-resolve before
    // calling Save.
    let resolutions: Vec<(crate::services::sportify::models::SportifyTrack, i64)> = db
        .with_conn(|conn| {
            let mut out = Vec::new();
            for t in &playlist.tracks {
                let Some(spotify_track_id) = t.id.as_deref() else {
                    continue;
                };
                if let Some(hit) =
                    sp_cache::get_tidal_resolution(conn, &cache_cfg, spotify_track_id)?
                {
                    out.push((t.clone(), hit.tidal_track_id));
                }
            }
            Ok::<_, anyhow::Error>(out)
        })
        .map_err(super::internal)?;

    let total_tracks = playlist.tracks.len();
    let resolved_count = resolutions.len();
    let unresolved_count = total_tracks.saturating_sub(resolved_count);

    if resolutions.is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "error": "no_resolved_tracks",
                "totalTracks": total_tracks,
                "resolvedCount": 0,
                "unresolvedCount": unresolved_count,
            })),
        ));
    }

    // Import each resolved TIDAL track into the local library so it has a
    // local_id for the playlist join. Failures are skipped, not fatal.
    let mut local_ids: Vec<i64> = Vec::with_capacity(resolutions.len());
    let mut import_failures: usize = 0;
    for (sp_track, tidal_id) in &resolutions {
        let metadata = tidal_import::ImportTrackMetadata {
            tidal_id: *tidal_id,
            title: sp_track.name.clone().unwrap_or_default(),
            artist_name: sp_track
                .primary_artist()
                .map(str::to_string)
                .unwrap_or_default(),
            artist_tidal_id: None,
            artist_picture: None,
            album_title: sp_track.album.as_ref().and_then(|a| a.name.clone()),
            album_tidal_id: None,
            album_artwork_url: sp_track.best_thumbnail(),
            duration_ms: sp_track.duration_ms,
        };
        match tidal_import::import_track_from_metadata(&db, metadata).await {
            Ok(imported) => local_ids.push(imported.local_id),
            Err(e) => {
                tracing::warn!(
                    "save_spotify_playlist: import failed for tidal_id {}: {}",
                    tidal_id,
                    e
                );
                import_failures += 1;
            }
        }
    }

    if local_ids.is_empty() {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": "all_imports_failed",
                "totalTracks": total_tracks,
                "resolvedCount": resolved_count,
                "importFailures": import_failures,
            })),
        ));
    }

    // Create the playlist row and bulk-add tracks in a single transaction.
    let result = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO playlists (name, description, is_smart, is_synced, track_count)
                 VALUES (?1, ?2, 0, 0, 0)",
                params![playlist_name, playlist.description],
            )?;
            let playlist_id = conn.last_insert_rowid();
            let added = queries::add_tracks_to_playlist(conn, playlist_id, &local_ids)?;
            let row = queries::get_playlist(conn, playlist_id)?
                .ok_or_else(|| anyhow::anyhow!("playlist not found after insert"))?;
            Ok::<_, anyhow::Error>((row, added))
        })
        .map_err(super::internal)?;

    Ok(Json(json!({
        "playlist": result.0,
        "added": result.1,
        "totalTracks": total_tracks,
        "resolvedCount": resolved_count,
        "unresolvedCount": unresolved_count,
        "importFailures": import_failures,
    })))
}
