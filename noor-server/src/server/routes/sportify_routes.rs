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

const SPORTIFY_SEARCH_LIMIT_DEFAULT: u32 = 20;
const SPORTIFY_SEARCH_LIMIT_MAX: u32 = 50;
const SPORTIFY_SEARCH_OFFSET_MAX: u32 = 1_000;

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
    match s.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("album") => SportifySearchKind::Album,
        Some("artist") => SportifySearchKind::Artist,
        Some("playlist") => SportifySearchKind::Playlist,
        // Default to track when missing or unknown: matches Sportify's own
        // most-common usage.
        _ => SportifySearchKind::Track,
    }
}

fn clamp_search_limit(limit: Option<u32>) -> u32 {
    limit
        .unwrap_or(SPORTIFY_SEARCH_LIMIT_DEFAULT)
        .clamp(1, SPORTIFY_SEARCH_LIMIT_MAX)
}

fn clamp_search_offset(offset: Option<u32>) -> u32 {
    offset.unwrap_or(0).min(SPORTIFY_SEARCH_OFFSET_MAX)
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
    let limit = clamp_search_limit(params.limit);
    let offset = clamp_search_offset(params.offset);

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
    let cached = cached.filter(|p| {
        !(matches!(
            kind,
            crate::services::sportify::client::SportifySearchKind::Playlist
        ) && p.playlists.is_empty())
    });

    let payload = match cached {
        Some(p) => p,
        None => {
            let primary = sportify_client.search(q, kind, limit, offset).await;
            let fetched = match primary {
                Ok(fetched) => fetched,
                Err(primary_error)
                    if matches!(
                        kind,
                        crate::services::sportify::client::SportifySearchKind::Playlist
                    ) =>
                {
                    crate::services::spotify::catalog::search_playlists_from_saved_credentials(
                        &db, q, limit, offset,
                    )
                    .await
                    .map_err(|fallback_error| {
                        (
                            StatusCode::BAD_GATEWAY,
                            Json(json!({
                                "error": format!(
                                    "sportify_search: {primary_error}; spotify_fallback: {fallback_error}"
                                )
                            })),
                        )
                    })?
                }
                Err(e) => {
                    return Err((
                        StatusCode::BAD_GATEWAY,
                        Json(json!({ "error": format!("sportify_search: {e}") })),
                    ));
                }
            };
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
    use crate::services::sportify::{cache as sp_cache, normalize, stats};

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
                Ok::<_, anyhow::Error>(())
            })
            .map_err(super::internal)?;
            fetched
        }
    };
    db.with_conn(|conn| {
        stats::write_track_playcount(conn, &track);
        Ok::<_, anyhow::Error>(())
    })
    .map_err(super::internal)?;

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
    use crate::services::sportify::{cache as sp_cache, normalize, stats};

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
                Ok::<_, anyhow::Error>(())
            })
            .map_err(super::internal)?;
            fetched
        }
    };
    db.with_conn(|conn| {
        stats::write_track_playcounts(conn, &album.tracks);
        Ok::<_, anyhow::Error>(())
    })
    .map_err(super::internal)?;

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
    use crate::services::sportify::{cache as sp_cache, normalize, stats};

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
            let fetched = match sportify_client.playlist(id).await {
                Ok(fetched) => fetched,
                Err(primary_error) => {
                    crate::services::spotify::catalog::playlist_from_saved_credentials(&db, id)
                        .await
                        .map_err(|fallback_error| {
                            (
                                StatusCode::BAD_GATEWAY,
                                Json(json!({
                                    "error": format!(
                                        "sportify_playlist_fetch: {primary_error}; spotify_fallback: {fallback_error}"
                                    )
                                })),
                            )
                        })?
                }
            };
            db.with_conn(|conn| {
                sp_cache::put_playlist_meta(conn, id, &fetched)?;
                Ok::<_, anyhow::Error>(())
            })
            .map_err(super::internal)?;
            fetched
        }
    };
    db.with_conn(|conn| {
        stats::write_track_playcounts(conn, &playlist.tracks);
        Ok::<_, anyhow::Error>(())
    })
    .map_err(super::internal)?;

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

pub(super) async fn sportify_discovery_playlist_meta(
    State(state): State<SharedState>,
    Path(spotify_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use crate::services::sportify::cache as sp_cache;

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
            let fetched = match sportify_client.playlist(id).await {
                Ok(fetched) => fetched,
                Err(primary_error) => {
                    crate::services::spotify::catalog::playlist_from_saved_credentials(&db, id)
                        .await
                        .map_err(|fallback_error| {
                            (
                                StatusCode::BAD_GATEWAY,
                                Json(json!({
                                    "error": format!(
                                        "sportify_playlist_fetch: {primary_error}; spotify_fallback: {fallback_error}"
                                    )
                                })),
                            )
                        })?
                }
            };
            db.with_conn(|conn| {
                sp_cache::put_playlist_meta(conn, id, &fetched)?;
                Ok::<_, anyhow::Error>(())
            })
            .map_err(super::internal)?;
            fetched
        }
    };

    Ok(Json(sportify_playlist_meta_value(id, &playlist)))
}

fn sportify_playlist_meta_value(
    id: &str,
    playlist: &crate::services::sportify::models::SportifyPlaylist,
) -> Value {
    json!({
        "source": "spotify",
        "spotifyId": playlist.spotify_id().unwrap_or_else(|| id.to_string()),
        "type": "playlist",
        "title": playlist.title(),
        "description": playlist.description.clone(),
        "thumbnail": playlist.best_thumbnail(),
        "owner": playlist
            .owner
            .as_ref()
            .and_then(|owner| owner.display_name())
            .map(str::to_string),
        "followers": playlist.follower_count(),
        "totalTracks": playlist.total_track_count(),
        "snapshotId": playlist.snapshot_id.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        SportifyImportSummary, importable_tidal_id, required_spotify_id,
        sportify_playlist_meta_value, spotify_save_response,
    };
    use crate::services::sportify::models::{
        SportifyAlbum, SportifyAlbumRef, SportifyArtistRef, SportifyImage, SportifyPlaylist,
        SportifyPlaylistOwner, SportifyTrack,
    };
    use axum::http::StatusCode;

    #[test]
    fn search_kind_parser_trims_type_values() {
        assert!(matches!(
            super::parse_search_kind(Some(" playlist ")),
            crate::services::sportify::client::SportifySearchKind::Playlist
        ));
        assert!(matches!(
            super::parse_search_kind(Some(" ALBUM ")),
            crate::services::sportify::client::SportifySearchKind::Album
        ));
        assert!(matches!(
            super::parse_search_kind(Some("unknown")),
            crate::services::sportify::client::SportifySearchKind::Track
        ));
    }

    #[test]
    fn search_limit_and_offset_are_bounded() {
        assert_eq!(super::clamp_search_limit(None), 20);
        assert_eq!(super::clamp_search_limit(Some(0)), 1);
        assert_eq!(super::clamp_search_limit(Some(5_000)), 50);
        assert_eq!(super::clamp_search_offset(None), 0);
        assert_eq!(super::clamp_search_offset(Some(42)), 42);
        assert_eq!(super::clamp_search_offset(Some(5_000)), 1_000);
    }

    #[test]
    fn playlist_meta_value_omits_tracks() {
        let playlist = SportifyPlaylist {
            id: Some("spotify-playlist".to_string()),
            name: Some("Top 50".to_string()),
            description: Some("Daily chart".to_string()),
            thumbnail: Some("https://img.example/small.jpg".to_string()),
            images: vec![SportifyImage {
                url: Some("https://img.example/large.jpg".to_string()),
                width: Some(640),
                height: Some(640),
            }],
            owner: Some(SportifyPlaylistOwner::Name("Spotify".to_string())),
            followers: Some(1234),
            snapshot_id: Some("snapshot-1".to_string()),
            total_tracks: Some(50),
            tracks: vec![SportifyTrack {
                id: Some("track-1".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };

        let value = sportify_playlist_meta_value("fallback-id", &playlist);

        assert!(value.get("tracks").is_none());
        assert_eq!(value["source"], "spotify");
        assert_eq!(value["spotifyId"], "spotify-playlist");
        assert_eq!(value["type"], "playlist");
        assert_eq!(value["title"], "Top 50");
        assert_eq!(value["description"], "Daily chart");
        assert_eq!(value["thumbnail"], "https://img.example/large.jpg");
        assert_eq!(value["owner"], "Spotify");
        assert_eq!(value["followers"], 1234);
        assert_eq!(value["totalTracks"], 50);
        assert_eq!(value["snapshotId"], "snapshot-1");
    }

    #[test]
    fn sportify_track_import_metadata_uses_track_fields_first() {
        let track = SportifyTrack {
            id: Some("spotify-track".to_string()),
            name: Some("Track Title".to_string()),
            artist: Some("Track Artist".to_string()),
            album: Some(SportifyAlbumRef {
                name: Some("Track Album".to_string()),
                images: vec![SportifyImage {
                    url: Some("https://img.example/track.jpg".to_string()),
                    width: Some(320),
                    height: Some(320),
                }],
                ..Default::default()
            }),
            duration_ms: Some(180_000),
            ..Default::default()
        };
        let album = SportifyAlbum {
            name: Some("Parent Album".to_string()),
            artists: vec![SportifyArtistRef {
                name: Some("Parent Artist".to_string()),
                ..Default::default()
            }],
            images: vec![SportifyImage {
                url: Some("https://img.example/parent.jpg".to_string()),
                width: Some(640),
                height: Some(640),
            }],
            ..Default::default()
        };

        let metadata = super::sportify_track_import_metadata(&track, 42, Some(&album));

        assert_eq!(metadata.tidal_id, 42);
        assert_eq!(metadata.title, "Track Title");
        assert_eq!(metadata.artist_name, "Track Artist");
        assert_eq!(metadata.album_title.as_deref(), Some("Track Album"));
        assert_eq!(
            metadata.album_artwork_url.as_deref(),
            Some("https://img.example/track.jpg"),
        );
        assert_eq!(metadata.duration_ms, Some(180_000));
    }

    #[test]
    fn sportify_track_import_metadata_backfills_parent_album_fields() {
        let track = SportifyTrack {
            name: Some("Album Track".to_string()),
            duration_ms: Some(210_000),
            ..Default::default()
        };
        let album = SportifyAlbum {
            name: Some("Parent Album".to_string()),
            artists: vec![SportifyArtistRef {
                name: Some("Parent Artist".to_string()),
                ..Default::default()
            }],
            images: vec![
                SportifyImage {
                    url: Some("https://img.example/small.jpg".to_string()),
                    width: Some(80),
                    height: Some(80),
                },
                SportifyImage {
                    url: Some("https://img.example/large.jpg".to_string()),
                    width: Some(640),
                    height: Some(640),
                },
            ],
            ..Default::default()
        };

        let metadata = super::sportify_track_import_metadata(&track, 99, Some(&album));

        assert_eq!(metadata.title, "Album Track");
        assert_eq!(metadata.artist_name, "Parent Artist");
        assert_eq!(metadata.album_title.as_deref(), Some("Parent Album"));
        assert_eq!(
            metadata.album_artwork_url.as_deref(),
            Some("https://img.example/large.jpg"),
        );
        assert_eq!(metadata.duration_ms, Some(210_000));
    }

    #[test]
    fn required_spotify_id_rejects_blank_ids() {
        let Err((status, body)) = required_spotify_id("  ") else {
            panic!("blank spotify id should fail");
        };

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body.0["error"], "spotify_id required");
    }

    #[test]
    fn required_spotify_id_trims_valid_ids() {
        let id = required_spotify_id("  spotify-id  ").expect("valid spotify id");

        assert_eq!(id, "spotify-id");
    }

    #[test]
    fn importable_tidal_id_rejects_placeholder_ids() {
        assert_eq!(importable_tidal_id(0), None);
        assert_eq!(importable_tidal_id(-42), None);
        assert_eq!(importable_tidal_id(42), Some(42));
    }

    #[test]
    fn spotify_save_response_rejects_unresolved_items() {
        let Err((status, body)) = spotify_save_response(SportifyImportSummary {
            total_tracks: 3,
            unresolved_count: 3,
            ..Default::default()
        }) else {
            panic!("unresolved save should fail");
        };

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body.0["error"], "no_resolved_tracks");
        assert_eq!(body.0["totalTracks"], 3);
        assert_eq!(body.0["unresolvedCount"], 3);
    }

    #[test]
    fn spotify_save_response_returns_import_counts() {
        let Ok(body) = spotify_save_response(SportifyImportSummary {
            total_tracks: 3,
            resolved_count: 2,
            unresolved_count: 1,
            imported: 2,
            import_failures: 0,
            local_ids: vec![11, 12],
        }) else {
            panic!("resolved save should succeed");
        };

        assert_eq!(body.0["imported"], 2);
        assert_eq!(body.0["totalTracks"], 3);
        assert_eq!(body.0["resolvedCount"], 2);
        assert_eq!(body.0["unresolvedCount"], 1);
        assert_eq!(body.0["localIds"][0], 11);
        assert_eq!(body.0["localIds"][1], 12);
    }
}

pub(super) async fn sportify_discovery_artist(
    State(state): State<SharedState>,
    Path(spotify_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use crate::services::sportify::{cache as sp_cache, normalize, stats};

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
                Ok::<_, anyhow::Error>(())
            })
            .map_err(super::internal)?;
            fetched
        }
    };
    db.with_conn(|conn| {
        stats::write_artist_monthly_listeners(conn, &artist);
        Ok::<_, anyhow::Error>(())
    })
    .map_err(super::internal)?;

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

#[derive(Debug, Deserialize)]
pub(super) struct SaveSpotifyItemBody {
    spotify_id: String,
}

#[derive(Debug, Default)]
struct SportifyImportSummary {
    total_tracks: usize,
    resolved_count: usize,
    unresolved_count: usize,
    imported: usize,
    import_failures: usize,
    local_ids: Vec<i64>,
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
                    if let Some(tidal_id) = importable_tidal_id(hit.tidal_track_id) {
                        out.push((t.clone(), tidal_id));
                    }
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

pub(super) async fn save_spotify_track(
    State(state): State<SharedState>,
    Json(body): Json<SaveSpotifyItemBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use crate::services::sportify::recommend;

    let id = required_spotify_id(&body.spotify_id)?;

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

    let track = recommend::cached_track(&sportify_client, &db, &cache_cfg, id)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("sportify_track_fetch: {e}") })),
            )
        })?;

    let summary = import_cached_sportify_tracks(
        &db,
        &cache_cfg,
        std::slice::from_ref(&track),
        None,
        "save_spotify_track",
    )
    .await
    .map_err(super::internal)?;

    spotify_save_response(summary)
}

pub(super) async fn save_spotify_album(
    State(state): State<SharedState>,
    Json(body): Json<SaveSpotifyItemBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use crate::services::sportify::recommend;

    let id = required_spotify_id(&body.spotify_id)?;

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

    let album = recommend::cached_album(&sportify_client, &db, &cache_cfg, id)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("sportify_album_fetch: {e}") })),
            )
        })?;

    let summary = import_cached_sportify_tracks(
        &db,
        &cache_cfg,
        &album.tracks,
        Some(&album),
        "save_spotify_album",
    )
    .await
    .map_err(super::internal)?;

    spotify_save_response(summary)
}

fn spotify_save_response(
    summary: SportifyImportSummary,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if summary.resolved_count == 0 {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "error": "no_resolved_tracks",
                "totalTracks": summary.total_tracks,
                "resolvedCount": 0,
                "unresolvedCount": summary.unresolved_count,
            })),
        ));
    }

    if summary.imported == 0 {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": "all_imports_failed",
                "totalTracks": summary.total_tracks,
                "resolvedCount": summary.resolved_count,
                "importFailures": summary.import_failures,
            })),
        ));
    }

    Ok(Json(json!({
        "imported": summary.imported,
        "totalTracks": summary.total_tracks,
        "resolvedCount": summary.resolved_count,
        "unresolvedCount": summary.unresolved_count,
        "importFailures": summary.import_failures,
        "localIds": summary.local_ids,
    })))
}

fn required_spotify_id(raw: &str) -> Result<&str, (StatusCode, Json<Value>)> {
    let id = raw.trim();
    if id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "spotify_id required" })),
        ));
    }
    Ok(id)
}

fn importable_tidal_id(tidal_id: i64) -> Option<i64> {
    (tidal_id > 0).then_some(tidal_id)
}

async fn import_cached_sportify_tracks(
    db: &crate::db::Database,
    cache_cfg: &crate::services::sportify::cache::SportifyCacheConfig,
    tracks: &[crate::services::sportify::models::SportifyTrack],
    album_fallback: Option<&crate::services::sportify::models::SportifyAlbum>,
    log_context: &str,
) -> anyhow::Result<SportifyImportSummary> {
    use crate::services::sportify::cache as sp_cache;

    let resolutions: Vec<(crate::services::sportify::models::SportifyTrack, i64)> =
        db.with_conn(|conn| {
            let mut out = Vec::new();
            for t in tracks {
                let Some(spotify_track_id) = t.id.as_deref() else {
                    continue;
                };
                if let Some(hit) =
                    sp_cache::get_tidal_resolution(conn, cache_cfg, spotify_track_id)?
                {
                    if let Some(tidal_id) = importable_tidal_id(hit.tidal_track_id) {
                        out.push((t.clone(), tidal_id));
                    }
                }
            }
            Ok::<_, anyhow::Error>(out)
        })?;

    let total_tracks = tracks.len();
    let resolved_count = resolutions.len();
    let unresolved_count = total_tracks.saturating_sub(resolved_count);
    let mut local_ids: Vec<i64> = Vec::with_capacity(resolved_count);
    let mut import_failures: usize = 0;

    for (sp_track, tidal_id) in &resolutions {
        let metadata = sportify_track_import_metadata(sp_track, *tidal_id, album_fallback);
        match tidal_import::import_track_from_metadata(db, metadata).await {
            Ok(imported) => local_ids.push(imported.local_id),
            Err(e) => {
                tracing::warn!(
                    "{}: import failed for tidal_id {}: {}",
                    log_context,
                    tidal_id,
                    e
                );
                import_failures += 1;
            }
        }
    }

    Ok(SportifyImportSummary {
        total_tracks,
        resolved_count,
        unresolved_count,
        imported: local_ids.len(),
        import_failures,
        local_ids,
    })
}

fn sportify_track_import_metadata(
    sp_track: &crate::services::sportify::models::SportifyTrack,
    tidal_id: i64,
    album_fallback: Option<&crate::services::sportify::models::SportifyAlbum>,
) -> tidal_import::ImportTrackMetadata {
    let fallback_artist = album_fallback
        .and_then(|album| album.artists.first())
        .and_then(|artist| artist.name.clone());
    let fallback_artwork = album_fallback.and_then(sportify_album_best_thumbnail);

    tidal_import::ImportTrackMetadata {
        tidal_id,
        title: sp_track
            .name
            .clone()
            .unwrap_or_else(|| "Spotify track".to_string()),
        artist_name: sp_track
            .primary_artist()
            .map(str::to_string)
            .or(fallback_artist)
            .unwrap_or_else(|| "Unknown artist".to_string()),
        artist_tidal_id: None,
        artist_picture: None,
        album_title: sp_track
            .album
            .as_ref()
            .and_then(|album| album.name.clone())
            .or_else(|| album_fallback.and_then(|album| album.name.clone())),
        album_tidal_id: None,
        album_artwork_url: sp_track.best_thumbnail().or(fallback_artwork),
        duration_ms: sp_track.duration_ms,
    }
}

fn sportify_album_best_thumbnail(
    album: &crate::services::sportify::models::SportifyAlbum,
) -> Option<String> {
    album
        .images
        .iter()
        .max_by_key(|image| image.width.unwrap_or(0))
        .and_then(|image| image.url.clone())
}
