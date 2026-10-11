//! TIDAL catalog endpoints: track/album/artist search, videos, playlists, and artist profiles.

use super::*;

#[derive(Debug, Deserialize)]
pub(super) struct TidalSearchParams {
    pub(super) q: String,
    pub(super) limit: Option<i32>,
    pub(super) offset: Option<i32>,
}

pub(super) const TIDAL_SEARCH_DEFAULT_LIMIT: i32 = 20;

pub(super) const TIDAL_SEARCH_MAX_LIMIT: i32 = 50;

pub(super) fn normalize_tidal_search_query(query: &str) -> Option<&str> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Canonical fetch size for TIDAL search, so callers asking for fewer results
/// share one cache row and one upstream call. Sized to the largest limit the
/// app actually requests (the remote's 12); a bigger request bypasses it and
/// caches on its own size.
pub(super) const TIDAL_SEARCH_CACHE_BUCKET: i32 = 12;

pub(super) fn normalize_tidal_search_limit(limit: Option<i32>) -> i32 {
    limit
        .unwrap_or(TIDAL_SEARCH_DEFAULT_LIMIT)
        .clamp(1, TIDAL_SEARCH_MAX_LIMIT)
}

pub(super) fn tidal_search_flight_key(query: &str, fetch_limit: i32, offset: i32) -> String {
    format!("{}|{}|{}", query.trim().to_lowercase(), fetch_limit, offset)
}

pub(super) fn tidal_search_flights()
-> &'static crate::services::tidal::singleflight::KeyedSingleFlight<String> {
    static FLIGHTS: OnceLock<crate::services::tidal::singleflight::KeyedSingleFlight<String>> =
        OnceLock::new();
    FLIGHTS.get_or_init(Default::default)
}

pub(super) fn empty_tidal_search_response() -> Json<Value> {
    Json(json!({
        "tracks": [],
        "albums": [],
        "artists": [],
        "videos": [],
    }))
}

#[derive(Serialize)]
pub(super) struct TidalSearchTrackResp {
    pub(super) tidal_id: i64,
    pub(super) title: String,
    pub(super) duration_ms: i64,
    pub(super) artist_id: Option<i64>,
    pub(super) artist_name: Option<String>,
    pub(super) album_title: Option<String>,
    pub(super) album_tidal_id: Option<i64>,
    pub(super) artwork_url: Option<String>,
    pub(super) audio_quality: Option<String>,
    pub(super) stream_ready: Option<bool>,
    pub(super) local_id: Option<i64>,
    pub(super) in_library: bool,
}

#[derive(Serialize)]
pub(super) struct TidalSearchAlbumResp {
    pub(super) tidal_id: i64,
    pub(super) title: String,
    pub(super) artist_name: Option<String>,
    pub(super) artwork_url: Option<String>,
    pub(super) local_id: Option<i64>,
    pub(super) in_library: bool,
}

#[derive(Serialize)]
pub(super) struct TidalSearchArtistResp {
    pub(super) tidal_id: i64,
    pub(super) name: String,
    pub(super) artwork_url: Option<String>,
    pub(super) local_id: Option<i64>,
    pub(super) in_library: bool,
}

#[derive(Serialize)]
pub(super) struct TidalSearchVideoResp {
    pub(super) tidal_id: i64,
    pub(super) title: String,
    pub(super) duration_ms: Option<i64>,
    pub(super) artist_id: Option<i64>,
    pub(super) artist_name: Option<String>,
    pub(super) album_tidal_id: Option<i64>,
    pub(super) artwork_url: Option<String>,
    pub(super) quality: Option<String>,
    pub(super) explicit: Option<bool>,
    r#type: String,
}

pub(super) async fn search_tidal_catalog_with_timeout(
    client: &TidalClient,
    query: &str,
    limit: i32,
    offset: i32,
) -> anyhow::Result<TidalSearchCatalog> {
    tokio::time::timeout(
        Duration::from_secs(TIDAL_SEARCH_UPSTREAM_TIMEOUT_SECS),
        client.search_catalog_core(query, limit, offset),
    )
    .await
    .map_err(|_| anyhow::anyhow!("TIDAL search timed out"))?
}

pub(super) async fn tidal_search(
    State(state): State<SharedState>,
    Query(params): Query<TidalSearchParams>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let Some(query) = normalize_tidal_search_query(&params.q) else {
        return Ok(empty_tidal_search_response());
    };

    let tokens = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;
        let s = state.read().await;
        s.tidal.tokens().or(persisted)
    };

    let Some(tokens) = tokens else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let limit = normalize_tidal_search_limit(params.limit);
    let offset = params.offset.unwrap_or(0).max(0);
    // Fetch and cache at one canonical size, then trim to what the caller
    // asked for. The cache key includes the limit, and the app searches the
    // same names at 1 (artwork), 5 (navigate/play), 6 (command palette) and 12
    // (remote), so one artist produced four cache rows and four upstream
    // searches for what is the same query. Requests inside the bucket now
    // share a row; anything larger keeps its own.
    let fetch_limit = limit.max(TIDAL_SEARCH_CACHE_BUCKET);
    // Snapshot what we need from state in one lock acquisition.
    let (db, tidal_session) = {
        let s = state.read().await;
        (s.db.clone(), s.tidal.clone())
    };

    let cache_cfg = crate::services::tidal::cache::TidalSearchCacheConfig::default();

    // Cache check - best-effort. A read failure must NOT block the upstream call.
    let cached = db
        .with_conn(|conn| {
            crate::services::tidal::cache::get_search(conn, &cache_cfg, query, fetch_limit, offset)
        })
        .ok()
        .flatten();

    let client = TidalClient::for_session(tidal_session.clone(), &tokens.country_code)
        .with_metadata_store(state.read().await.db.clone());

    let results = if let Some(hit) = cached {
        hit
    } else {
        tidal_search_flights()
            .get_or_build(
                tidal_search_flight_key(query, fetch_limit, offset),
                || {
                    db.with_conn(|conn| {
                        crate::services::tidal::cache::get_search(
                            conn,
                            &cache_cfg,
                            query,
                            fetch_limit,
                            offset,
                        )
                    })
                    .ok()
                    .flatten()
                },
                || async {
                    let fetched = match search_tidal_catalog_with_timeout(
                        &client,
                        query,
                        fetch_limit,
                        offset,
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            return Err((
                                StatusCode::BAD_GATEWAY,
                                Json(json!({ "error": e.to_string() })),
                            ));
                        }
                    };

                    // Best-effort cache write - log and continue on failure.
                    let to_cache = fetched.clone();
                    let q_owned = query.to_string();
                    if let Err(e) = db.with_conn(move |conn| {
                        crate::services::tidal::cache::put_search(
                            conn,
                            &q_owned,
                            fetch_limit,
                            offset,
                            &to_cache,
                        )
                    }) {
                        tracing::warn!("tidal_search_cache write failed: {}", e);
                    }
                    Ok(fetched)
                },
            )
            .await?
    };

    // Trim the canonical fetch down to what this caller asked for.
    let results = {
        let mut results = results;
        let want = limit.max(0) as usize;
        results.tracks.truncate(want);
        results.albums.truncate(want);
        results.artists.truncate(want);
        results.videos.truncate(want);
        results
    };

    // Batch-lookup which Tidal IDs are in the local library so the frontend can
    // route to local pages and badge entries as in-library.
    let track_tidal_ids: Vec<i64> = results.tracks.iter().map(|t| t.id).collect();
    let album_tidal_ids: Vec<i64> = results.albums.iter().map(|a| a.id).collect();
    let artist_tidal_ids: Vec<i64> = results.artists.iter().map(|a| a.id).collect();
    let (track_map, known_albums, known_artists, artist_photos) = {
        let s = state.read().await;
        s.db.with_conn(|conn| {
            let tracks = queries::get_tidal_track_local_ids(conn, &track_tidal_ids)?;
            let albums = queries::get_known_album_tidal_ids(conn, &album_tidal_ids)?;
            let artists = queries::get_known_artist_tidal_ids(conn, &artist_tidal_ids)?;
            let photos = queries::get_artist_photos_by_tidal_ids(conn, &artist_tidal_ids)?;
            Ok((tracks, albums, artists, photos))
        })
        .unwrap_or_default()
    };

    let tracks: Vec<TidalSearchTrackResp> = results
        .tracks
        .into_iter()
        .map(|t| TidalSearchTrackResp {
            local_id: track_map.get(&t.id).copied(),
            in_library: track_map.contains_key(&t.id),
            tidal_id: t.id,
            title: t.title,
            duration_ms: t.duration * 1000,
            artist_id: t.artist_id,
            artist_name: t.artist_name,
            album_title: t.album_title,
            album_tidal_id: t.album_id,
            artwork_url: t.artwork_url,
            audio_quality: t.audio_quality,
            stream_ready: t.stream_ready,
        })
        .collect();

    let albums: Vec<TidalSearchAlbumResp> = results
        .albums
        .into_iter()
        .map(|a| {
            let local_id = known_albums.get(&a.id).copied();
            TidalSearchAlbumResp {
                tidal_id: a.id,
                title: a.title,
                artist_name: a.artist_name,
                artwork_url: a.artwork_url,
                in_library: local_id.is_some(),
                local_id,
            }
        })
        .collect();

    let artists: Vec<TidalSearchArtistResp> = results
        .artists
        .into_iter()
        .map(|a| {
            let local_id = known_artists.get(&a.id).copied();
            TidalSearchArtistResp {
                tidal_id: a.id,
                name: a.name,
                artwork_url: a.artwork_url.or_else(|| artist_photos.get(&a.id).cloned()),
                in_library: local_id.is_some(),
                local_id,
            }
        })
        .collect();

    let videos: Vec<TidalSearchVideoResp> = results
        .videos
        .into_iter()
        .map(tidal_video_to_resp)
        .collect();

    Ok(Json(
        json!({ "tracks": tracks, "albums": albums, "artists": artists, "videos": videos }),
    ))
}

pub(super) fn tidal_video_to_resp(video: TidalSearchVideo) -> TidalSearchVideoResp {
    TidalSearchVideoResp {
        tidal_id: video.id,
        title: video.title,
        duration_ms: video.duration.map(|duration| duration * 1000),
        artist_id: video.artist_id,
        artist_name: video.artist_name,
        album_tidal_id: video.album_id,
        artwork_url: video.artwork_url,
        quality: video.quality,
        explicit: video.explicit,
        r#type: video.r#type,
    }
}

pub(super) const TIDAL_VIDEO_MIX_ID_MAX_LEN: usize = 96;

pub(super) fn normalize_tidal_video_mix_id(id: &str) -> Result<&str, StatusCode> {
    let trimmed = id.trim();
    if trimmed.is_empty()
        || trimmed.len() > TIDAL_VIDEO_MIX_ID_MAX_LEN
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(trimmed)
}

pub(super) async fn tidal_request_tokens(
    state: &SharedState,
) -> Result<Option<tidal_auth::TidalTokens>, (StatusCode, Json<Value>)> {
    let persisted = load_persisted_tidal_tokens(state).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
    })?;
    let s = state.read().await;
    Ok(s.tidal.tokens().or(persisted))
}

/// Every video list the listener opens teaches the discovery crawler.
pub(super) async fn harvest_seen_videos(
    state: &SharedState,
    videos: &[crate::services::tidal::client::TidalSearchVideo],
    list_key: Option<String>,
) {
    use crate::services::video_discovery::harvest::{self, HarvestContext};
    let db = state.read().await.db.clone();
    let candidates: Vec<crate::services::video_sets::VideoCandidate> = videos
        .iter()
        .map(crate::services::video_sets::VideoCandidate::from)
        .collect();
    let ctx = match list_key.as_deref() {
        Some(key) => HarvestContext::List { key },
        None => HarvestContext::Search,
    };
    if let Err(error) = db.with_conn(|conn| harvest::ingest(conn, &candidates, ctx)) {
        tracing::debug!(target: "noor.video_discovery", %error, "could not harvest seen videos");
    }
}

pub(super) async fn tidal_video_search(
    State(state): State<SharedState>,
    Query(params): Query<TidalSearchParams>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let Some(query) = normalize_tidal_video_search_query(&params.q) else {
        return Ok(Json(json!({ "videos": [] })));
    };

    let Some(tokens) = tidal_request_tokens(&state).await? else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let limit = normalize_tidal_video_search_limit(params.limit);
    let offset = params.offset.unwrap_or(0).max(0);
    let tidal_session = state.read().await.tidal.clone();
    let client = TidalClient::for_session(tidal_session.clone(), &tokens.country_code)
        .with_metadata_store(state.read().await.db.clone());
    let videos = match client.search_videos(query, limit, offset).await {
        Ok(videos) => videos,
        Err(e) => {
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": e.to_string() })),
            ));
        }
    };

    harvest_seen_videos(&state, &videos, None).await;

    Ok(Json(json!({
        "videos": videos.into_iter().map(tidal_video_to_resp).collect::<Vec<_>>()
    })))
}

pub(super) const TIDAL_VIDEO_SEARCH_DEFAULT_LIMIT: i32 = 20;

pub(super) const TIDAL_VIDEO_SEARCH_MAX_LIMIT: i32 = 50;

pub(super) fn normalize_tidal_video_search_query(query: &str) -> Option<&str> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

pub(super) fn normalize_tidal_video_search_limit(limit: Option<i32>) -> i32 {
    limit
        .unwrap_or(TIDAL_VIDEO_SEARCH_DEFAULT_LIMIT)
        .clamp(1, TIDAL_VIDEO_SEARCH_MAX_LIMIT)
}

pub(super) fn tidal_track_artwork_url(t: &TidalTrack, size: i32) -> Option<String> {
    t.album
        .as_ref()
        .and_then(|al| al.cover.as_ref())
        .and_then(|c| TidalClient::get_artwork_url(&Some(c.clone()), size))
}

pub(in crate::server) fn tidal_track_playable_json(
    t: TidalTrack,
    library_state: Option<queries::TidalTrackLibraryState>,
    artwork_size: i32,
) -> Value {
    let artwork = tidal_track_artwork_url(&t, artwork_size);
    json!({
        "tidal_id": t.id,
        "title": t.title,
        "duration_ms": t.duration * 1000,
        "track_number": t.track_number,
        "disc_number": t.volume_number,
        "artist_name": t.artist.name,
        "artist_tidal_id": t.artist.id,
        "album_title": t.album.as_ref().map(|al| al.title.clone()),
        "album_tidal_id": t.album.as_ref().map(|al| al.id),
        "artwork_url": artwork,
        "track_id": library_state.map(|s| s.local_id).unwrap_or(0),
        "is_in_library": library_state.is_some(),
        "is_favorite": library_state.map(|s| s.is_favorite).unwrap_or(false),
    })
}

#[derive(Debug, Deserialize)]
pub(super) struct TidalVideoPlaybackParams {
    pub(super) quality: Option<String>,
}

pub(super) fn tidal_video_stream_error_response(
    video_id: i64,
    err: tidal_stream::StreamResolveError,
    fallback_message: &str,
) -> (StatusCode, Json<Value>) {
    match err {
        tidal_stream::StreamResolveError::SessionExpired { message } => (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "status": "session_expired",
                "message": "TIDAL session expired while starting video playback.",
                "details": message,
                "video_id": video_id,
            })),
        ),
        tidal_stream::StreamResolveError::SessionRefreshFailed { message } => (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "status": "session_refresh_failed",
                "message": "TIDAL session could not be refreshed before video playback.",
                "details": message,
                "video_id": video_id,
            })),
        ),
        tidal_stream::StreamResolveError::ResponseParseFailed { message } => (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "status": "response_parse_failed",
                "message": fallback_message,
                "details": message,
                "video_id": video_id,
            })),
        ),
        tidal_stream::StreamResolveError::ManifestDecodeFailed { message } => (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "status": "manifest_decode_failed",
                "message": "TIDAL video manifest could not be decoded.",
                "details": message,
                "video_id": video_id,
            })),
        ),
        tidal_stream::StreamResolveError::ManifestParseFailed { message } => (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "status": "manifest_parse_failed",
                "message": "TIDAL video manifest could not be parsed.",
                "details": message,
                "video_id": video_id,
            })),
        ),
        tidal_stream::StreamResolveError::MissingStreamUrl => (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "status": "missing_stream_url",
                "message": "TIDAL video manifest did not contain an HLS stream URL.",
                "video_id": video_id,
            })),
        ),
        tidal_stream::StreamResolveError::MissingManifest => (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "status": "missing_manifest",
                "message": "TIDAL video playback response did not contain a manifest.",
                "video_id": video_id,
            })),
        ),
        tidal_stream::StreamResolveError::StreamRejected { message } => (
            StatusCode::FORBIDDEN,
            Json(json!({
                "status": "stream_rejected",
                "message": "TIDAL rejected the video playback request.",
                "details": message,
                "video_id": video_id,
            })),
        ),
        tidal_stream::StreamResolveError::RequestFailed { message } => (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "status": "stream_request_failed",
                "message": fallback_message,
                "details": message,
                "video_id": video_id,
            })),
        ),
        tidal_stream::StreamResolveError::UpstreamHttp { status, body } => (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "status": "stream_upstream_http",
                "message": format!("TIDAL returned {} while starting video playback.", status),
                "details": body,
                "video_id": video_id,
            })),
        ),
    }
}

pub(super) async fn tidal_video_playback(
    State(state): State<SharedState>,
    Path(video_id): Path<i64>,
    Query(params): Query<TidalVideoPlaybackParams>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if video_id <= 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Expected a positive TIDAL video id" })),
        ));
    }

    crate::services::video_discovery::governor::note_video_activity();

    let Some(tokens) = tidal_request_tokens(&state).await? else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let quality = params.quality.unwrap_or_else(|| "HIGH".to_string());
    let http_client = state.read().await.http_client.clone();
    tracing::info!(target: "tidal::video", video_id, quality = %quality, "Resolving TIDAL video stream");
    let stream_info = match tidal_stream::resolve_video_stream(
        &http_client,
        &tokens.access_token,
        video_id,
        &quality,
    )
    .await
    {
        Ok(info) => info,
        Err(e) if e.is_session_expired() => {
            let session = state.read().await.tidal.clone();
            let refreshed = session
                .refresh_stale(&tokens.access_token)
                .await
                .map_err(|re| {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({ "error": format!("TIDAL session refresh failed: {}", re) })),
                    )
                })?;
            tidal_stream::resolve_video_stream(
                &http_client,
                &refreshed.access_token,
                video_id,
                &quality,
            )
            .await
            .map_err(|e2| {
                tidal_video_stream_error_response(
                    video_id,
                    e2,
                    "TIDAL video playback URL could not be resolved.",
                )
            })?
        }
        Err(e) => {
            return Err(tidal_video_stream_error_response(
                video_id,
                e,
                "TIDAL video playback URL could not be resolved.",
            ));
        }
    };

    Ok(Json(json!({
        "hls_url": stream_info.hls_manifest_url,
        "expires_at": stream_info.expires_at,
        "quality": stream_info.video_quality,
    })))
}

pub(super) async fn tidal_video_mix_items(
    State(state): State<SharedState>,
    Path(mix_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mix_id = normalize_tidal_video_mix_id(&mix_id).map_err(|status| {
        (
            status,
            Json(json!({ "error": "invalid TIDAL video mix id" })),
        )
    })?;

    let Some(tokens) = tidal_request_tokens(&state).await? else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let tidal_session = state.read().await.tidal.clone();
    let client = TidalClient::for_session(tidal_session.clone(), &tokens.country_code)
        .with_metadata_store(state.read().await.db.clone());
    let items = match client.get_video_mix_items(mix_id).await {
        Ok(items) => items,
        Err(e) => {
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": e.to_string() })),
            ));
        }
    };

    harvest_seen_videos(&state, &items, Some(format!("mix:{mix_id}"))).await;

    Ok(Json(json!({
        "items": items.into_iter().map(tidal_video_to_resp).collect::<Vec<_>>()
    })))
}

pub(super) async fn tidal_video_playlist_items(
    State(state): State<SharedState>,
    Path(uuid): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let uuid = normalize_tidal_playlist_uuid(&uuid).map_err(|status| {
        (
            status,
            Json(json!({ "error": "invalid TIDAL playlist id" })),
        )
    })?;

    let Some(tokens) = tidal_request_tokens(&state).await? else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let tidal_session = state.read().await.tidal.clone();
    let client = TidalClient::for_session(tidal_session.clone(), &tokens.country_code)
        .with_metadata_store(state.read().await.db.clone());
    let items = match client.get_playlist_video_items(uuid).await {
        Ok(items) => items,
        Err(e) => {
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": e.to_string() })),
            ));
        }
    };

    harvest_seen_videos(&state, &items, Some(format!("playlist:{uuid}"))).await;

    Ok(Json(json!({
        "items": items.into_iter().map(tidal_video_to_resp).collect::<Vec<_>>()
    })))
}

#[derive(Debug, Deserialize)]
pub(super) struct TidalPlaylistSearchParams {
    pub(super) q: String,
    #[serde(default)]
    pub(super) limit: Option<i32>,
    #[serde(default)]
    pub(super) offset: Option<i32>,
}

pub(super) const TIDAL_PLAYLIST_SEARCH_DEFAULT_LIMIT: i32 = 20;

pub(super) const TIDAL_PLAYLIST_SEARCH_MAX_LIMIT: i32 = 50;

pub(super) const TIDAL_PLAYLIST_UUID_MAX_LEN: usize = 96;

pub(super) fn normalize_tidal_playlist_search_query(query: &str) -> Option<&str> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

pub(super) fn normalize_tidal_playlist_search_limit(limit: Option<i32>) -> i32 {
    limit
        .unwrap_or(TIDAL_PLAYLIST_SEARCH_DEFAULT_LIMIT)
        .clamp(1, TIDAL_PLAYLIST_SEARCH_MAX_LIMIT)
}

pub(super) fn normalize_tidal_playlist_uuid(uuid: &str) -> Result<&str, StatusCode> {
    let trimmed = uuid.trim();
    if trimmed.is_empty()
        || trimmed.len() > TIDAL_PLAYLIST_UUID_MAX_LEN
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(trimmed)
}

pub(super) async fn tidal_playlist_search(
    State(state): State<SharedState>,
    Query(params): Query<TidalPlaylistSearchParams>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let Some(query) = normalize_tidal_playlist_search_query(&params.q) else {
        return Ok(Json(json!({ "playlists": [] })));
    };

    let tokens = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;
        let s = state.read().await;
        s.tidal.tokens().or(persisted)
    };
    let Some(tokens) = tokens else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let limit = normalize_tidal_playlist_search_limit(params.limit);
    let offset = params.offset.unwrap_or(0).max(0);
    let tidal_session = state.read().await.tidal.clone();
    let client = TidalClient::for_session(tidal_session.clone(), &tokens.country_code)
        .with_metadata_store(state.read().await.db.clone());
    let playlists = match client.search_playlists(query, limit, offset).await {
        Ok(r) => r,
        Err(e) => {
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": e.to_string() })),
            ));
        }
    };

    let items: Vec<Value> = playlists
        .into_iter()
        .map(|p| {
            json!({
                "uuid": p.uuid,
                "title": p.title,
                "description": p.description,
                "number_of_tracks": p.number_of_tracks,
                "artwork_url": TidalClient::get_artwork_url(&p.square_image, 640),
            })
        })
        .collect();
    Ok(Json(json!({ "playlists": items })))
}

pub(super) async fn tidal_playlist_tracks(
    State(state): State<SharedState>,
    Path(uuid): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let uuid = normalize_tidal_playlist_uuid(&uuid).map_err(|status| {
        (
            status,
            Json(json!({ "error": "invalid TIDAL playlist uuid" })),
        )
    })?;

    let tokens = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;
        let s = state.read().await;
        s.tidal.tokens().or(persisted)
    };
    let Some(tokens) = tokens else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let (tidal_session, playlist_tracks_cache) = {
        let s = state.read().await;
        (s.tidal.clone(), s.tidal_playlist_tracks_cache.clone())
    };
    let limit = 100;
    let offset = 0;
    let cache_key = tidal_playlist_tracks_cache_key(&tokens.country_code, uuid, limit, offset);
    let client = TidalClient::for_session(tidal_session.clone(), &tokens.country_code)
        .with_metadata_store(state.read().await.db.clone());
    let tracks = match get_cached_tidal_playlist_tracks(&playlist_tracks_cache, &cache_key) {
        Some(cached) => cached,
        None => {
            let resp = match client.get_playlist_tracks(uuid, limit, offset).await {
                Ok(r) => r,
                Err(e) => {
                    return Err((
                        StatusCode::BAD_GATEWAY,
                        Json(json!({ "error": e.to_string() })),
                    ));
                }
            };
            put_cached_tidal_playlist_tracks(&playlist_tracks_cache, cache_key, resp.items.clone());
            resp.items
        }
    };

    let tidal_ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    let library_states = {
        let s = state.read().await;
        s.db.with_conn(|conn| queries::get_tidal_track_library_states(conn, &tidal_ids))
            .unwrap_or_default()
    };
    let playable: Vec<serde_json::Value> = tracks
        .into_iter()
        .map(|t| {
            let library_state = library_states.get(&t.id).copied();
            tidal_track_playable_json(t, library_state, 640)
        })
        .collect();

    Ok(Json(json!({ "tracks": playable })))
}

pub(crate) fn tidal_playlist_tracks_cache_key(
    country_code: &str,
    uuid: &str,
    limit: i32,
    offset: i32,
) -> String {
    format!("{country_code}:{uuid}:{limit}:{offset}")
}

pub(crate) fn get_cached_tidal_playlist_tracks(
    cache: &TidalPlaylistTracksCache,
    key: &str,
) -> Option<Vec<TidalTrack>> {
    let mut guard = cache.lock().unwrap();
    if let Some((stored_at, cached)) = guard.get(key)
        && stored_at.elapsed() < TIDAL_PLAYLIST_TRACKS_CACHE_TTL
    {
        return Some(cached.clone());
    }
    guard.remove(key);
    None
}

pub(crate) fn put_cached_tidal_playlist_tracks(
    cache: &TidalPlaylistTracksCache,
    key: String,
    tracks: Vec<TidalTrack>,
) {
    let mut guard = cache.lock().unwrap();
    // Sweep expired entries on insert so distinct (playlist, page) keys don't
    // accumulate dead entries for the process lifetime. Inserts only happen on
    // a cache miss (after a network fetch), so the O(n) scan is cheap and rare.
    guard.retain(|_, (stored_at, _)| stored_at.elapsed() < TIDAL_PLAYLIST_TRACKS_CACHE_TTL);
    guard.insert(key, (Instant::now(), tracks));
}

pub(super) async fn tidal_artist_profile(
    State(state): State<SharedState>,
    Path(tidal_artist_id): Path<i64>,
    Query(query): Query<catalog_routes::ArtistProfileQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if tidal_artist_id <= 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Expected a positive TIDAL artist id" })),
        ));
    }

    let (tokens, tidal_session) = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;
        let s = state.read().await;
        (s.tidal.tokens().or(persisted), s.tidal.clone())
    };

    let Some(tokens) = tokens else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let client = TidalClient::for_session(tidal_session.clone(), &tokens.country_code)
        .with_metadata_store(state.read().await.db.clone());

    // Same rich payload the library `/api/artists/{id}/discography` route
    // builds, keyed straight off the TIDAL id (no local artist row). This is
    // what lets a non-library artist page render identically to a library one:
    // bio, similar artists, videos, and categorized releases instead of a bare
    // top-tracks-and-albums stub.
    let payload = if query.preview {
        catalog_routes::build_tidal_artist_preview_payload(&state, &client, tidal_artist_id).await
    } else {
        catalog_routes::build_tidal_artist_payload(&state, &client, tidal_artist_id).await
    };
    Ok(Json(payload))
}

pub(super) async fn tidal_artist_core(
    State(state): State<SharedState>,
    Path(tidal_artist_id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if tidal_artist_id <= 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Expected a positive TIDAL artist id" })),
        ));
    }

    let (tokens, tidal_session) = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;
        let s = state.read().await;
        (s.tidal.tokens().or(persisted), s.tidal.clone())
    };

    let Some(tokens) = tokens else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let client = TidalClient::for_session(tidal_session.clone(), &tokens.country_code)
        .with_metadata_store(state.read().await.db.clone());
    let payload =
        catalog_routes::build_tidal_artist_core_payload(&state, &client, tidal_artist_id).await;
    Ok(Json(payload))
}
