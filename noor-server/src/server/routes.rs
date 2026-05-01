use crate::db::queries;
use crate::library::duplicates as dup;
use crate::metadata::discogs::DiscogsClient;
use crate::metadata::lastfm::LastFmClient;
use crate::playback::{player, queue, runtime as playback_runtime};
use crate::services::discovery::{
    DiscoveryCandidateSeed, DiscoveryProvider, TidalDiscoveryProvider,
};
use crate::services::learning as discovery_learning;
use crate::services::tidal::{
    auth as tidal_auth, client::TidalClient, import as tidal_import,
    mutations as tidal_mutations, stream as tidal_stream,
};
use crate::services::spotify;
use crate::smart::discovery as discovery_engine;
use crate::smart::external_discovery as external_discovery_engine;
use crate::smart::playlists::{
    PlaylistEvaluationContext, SmartPlaylistDefinition, TrackDspFeatures, evaluate_playlist,
};
use crate::{AppEvent, PlaybackRuntimeInfo, PlaybackRuntimeState, SharedState};
use anyhow::Context;
use axum::{
    Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
    routing::{get, patch, post, put},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use rusqlite::{params, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

#[derive(Debug, Deserialize)]
pub struct ListParams {
    sort_by: Option<String>,
    sort_dir: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
    favorite_only: Option<bool>,
    // DSP filter params
    bpm_min: Option<f64>,
    bpm_max: Option<f64>,
    energy_min: Option<f64>,
    energy_max: Option<f64>,
    key_signature: Option<String>,
    instrumental_only: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    q: String,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct GenreTrackParams {
    include_descendants: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct GenreHeatParams {
    days: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ListenHistoryParams {
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct AnalyticsDashboardParams {
    recent_limit: Option<i64>,
    top_limit: Option<i64>,
    days: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct DiscoveryPreviewRequest {
    prompt: String,
    mode: Option<String>,
    services: Option<Vec<String>>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct DiscoveryPresetRequest {
    name: String,
    prompt: String,
    mode: Option<String>,
    services: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct DiscoveryExternalRequest {
    prompt: String,
    mode: Option<String>,
    services: Option<Vec<String>>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct DiscoveryExternalResultRequest {
    provider: String,
    provider_track_id: String,
    title: String,
    artist_name: Option<String>,
    album_title: Option<String>,
    artwork_url: Option<String>,
    duration_ms: Option<i64>,
    audio_quality: Option<String>,
    normalized_genres: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct DiscoveryConnectionsRequest {
    prompt: String,
    mode: Option<String>,
    services: Option<Vec<String>>,
    limit: Option<i64>,
    seed: DiscoveryExternalResultRequest,
}

#[derive(Debug, Deserialize)]
pub struct PlaybackTrackRequest {
    track_id: i64,
}

#[derive(Debug, Deserialize)]
pub struct QueueReplaceRequest {
    track_ids: Vec<i64>,
    /// Optional per-row provenance strings, aligned by index with
    /// `track_ids`. `None` (or omission) means "no reason recorded".
    /// When the client sends a shorter list than `track_ids`, missing
    /// indices are treated as `None`. Excess entries are ignored.
    #[serde(default)]
    reasons: Option<Vec<Option<String>>>,
    /// Phase 2c-ii-a: last.fm candidates that have no library track_id yet.
    /// These are appended after the library tracks as pending queue rows.
    #[serde(default)]
    pending_candidates: Option<Vec<PendingCandidateRequest>>,
}

#[derive(Debug, Deserialize)]
pub struct PendingCandidateRequest {
    pub artist: String,
    pub title: String,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct QueueRemoveRequest {
    queue_item_id: i64,
}

#[derive(Debug, Deserialize)]
pub struct QueueMoveRequest {
    item_id: i64,
    new_pos: i32,
}

#[derive(Debug, Deserialize)]
pub struct PlaylistFromQueueRequest {
    name: String,
    #[serde(default)]
    include_tidal_only: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct BatchPlaylistRequest {
    playlist_id: i64,
    track_ids: Vec<i64>,
}

#[derive(Debug, Deserialize)]
pub struct BatchDeleteRequest {
    track_ids: Option<Vec<i64>>,
    album_ids: Option<Vec<i64>>,
}

#[derive(Debug, Deserialize)]
pub struct BatchGenreRequest {
    genre_id: i64,
    track_ids: Vec<i64>,
}

#[derive(Debug, Deserialize)]
pub struct TrackFavoriteRequest {
    track_id: i64,
    favorite: bool,
}

#[derive(Debug, Deserialize)]
pub struct PositionRequest {
    position_ms: i64,
}

#[derive(Debug, Deserialize)]
pub struct VolumeRequest {
    volume: f64,
}

#[derive(Debug, Deserialize)]
pub struct ShuffleModeRequest {
    mode: String,
}

#[derive(Debug, Deserialize)]
pub struct RepeatModeRequest {
    mode: String,
}

#[derive(Debug, Deserialize)]
pub struct AutomixRequest {
    enabled: bool,
    crossfade_ms: Option<i32>,
    discover_new: Option<bool>,
    use_learning: Option<bool>,
    allow_external: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct DiscoveryTrainRequest {
    mode: Option<String>,
    rebuild_audio: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct DiscoveryFeedbackRequest {
    seed_track_id: i64,
    candidate_track_id: i64,
    action: String,
    surface: String,
    context: Option<Value>,
    #[serde(default)]
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ResolveGroupRequest {
    preferred_track_id: i64,
}

#[derive(Debug, Deserialize)]
struct AddTracksToPlaylistRequest {
    track_ids: Vec<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateSmartPlaylistRequest {
    name: String,
    description: Option<String>,
    /// The root `RuleClause` as a raw JSON value — validated by deserializing into
    /// `SmartPlaylistDefinition` before writing to DB.
    rules: Value,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSmartPlaylistRequest {
    name: String,
    description: Option<String>,
    rules: Value,
}

pub fn api_routes(state: SharedState) -> Router {
    Router::new()
        // Library endpoints
        .route("/api/tracks", get(get_tracks))
        .route("/api/tracks/count", get(get_track_count))
        .route("/api/albums", get(get_albums))
        .route("/api/albums/{id}/tracks", get(get_album_tracks))
        .route("/api/artists", get(get_artists))
        .route("/api/artists/{id}/tracks", get(get_artist_tracks))
        .route("/api/artists/{id}/discography", get(get_artist_discography))
        .route("/api/tidal/albums/{id}/tracks", get(get_tidal_album_tracks))
        .route("/api/tidal/albums/{id}/import", post(import_tidal_album))
        .route("/api/tidal/tracks/import", post(import_tidal_track_for_radio))
        .route("/api/genres", get(get_genres))
        .route("/api/genres/heat", get(get_genre_heat))
        .route("/api/genres/co-occurrence", get(get_genre_co_occurrence))
        .route("/api/genres/cohorts", get(get_genre_cohorts))
        .route("/api/genres/evolution", get(get_genre_evolution))
        .route("/api/genres/{id}/tracks", get(get_genre_tracks))
        .route("/api/playlists", get(get_playlists))
        .route("/api/playlists/{id}/tracks", get(get_playlist_tracks).post(add_tracks_to_playlist_route))
        .route("/api/playlists/{id}/favorite", patch(toggle_playlist_favorite_route))
        .route("/api/smart/playlists", post(create_smart_playlist_route))
        .route(
            "/api/smart/playlists/{id}",
            put(update_smart_playlist_route).delete(delete_smart_playlist_route),
        )
        .route(
            "/api/smart/playlists/{id}/evaluate",
            get(evaluate_smart_playlist),
        )
        .route("/api/analytics/overview", get(get_analytics_overview))
        .route("/api/analytics/dashboard", get(get_analytics_dashboard))
        .route("/api/analytics/listens/recent", get(get_recent_listens))
        .route("/api/discovery/preview", post(preview_discovery))
        .route("/api/discovery/new", post(discover_new_music))
        .route("/api/discovery/save", post(save_discovery_track))
        .route("/api/discovery/play", post(play_discovery_track))
        .route("/api/discovery/connections", post(discover_connected_music))
        .route("/api/discovery/status", get(get_discovery_status))
        .route("/api/discovery/train", post(start_discovery_training))
        .route("/api/discovery/train/status", get(get_discovery_training_status))
        .route("/api/discovery/train/stop", post(stop_discovery_training))
        .route("/api/discovery/feedback", post(record_discovery_feedback))
        .route(
            "/api/discovery/presets",
            get(get_discovery_presets).post(create_discovery_preset),
        )
        // Similar Radio
        .route("/api/discovery/radio", post(get_radio_tracks))
        .route("/api/discovery/radio/compute", post(compute_radio_similarity))
        // Discovery Sound Space
        .route("/api/discovery/space", post(get_discovery_space))
        .route("/api/radio/song", post(radio_song))
        .route("/api/radio/album", post(radio_album))
        .route("/api/radio/artist", post(radio_artist))
        .route("/api/radio/start", post(radio_start))
        .route("/api/discovery/space/meta", get(get_discovery_space_meta))
        .route("/api/discovery/artists", get(get_discovery_artists))
        .route(
            "/api/library/batch/add-to-playlist",
            post(batch_add_to_playlist),
        )
        .route("/api/library/batch/delete", post(batch_delete_items))
        .route("/api/library/batch/set-genre", post(batch_set_genre))
        .route(
            "/api/library/enrich/musicbrainz",
            post(start_musicbrainz_enrichment),
        )
        .route(
            "/api/library/enrich/musicbrainz/status",
            get(get_musicbrainz_status),
        )
        .route(
            "/api/library/enrich/musicbrainz/portable",
            get(get_musicbrainz_portable_snapshot),
        )
        .route(
            "/api/library/enrich/musicbrainz/portable/export",
            post(export_musicbrainz_portable_snapshot),
        )
        .route(
            "/api/library/enrich/musicbrainz/portable/import",
            post(import_musicbrainz_portable_snapshot),
        )
        .route("/api/library/tracks/favorite", post(set_track_favorite))
        // Duplicates
        .route("/api/library/duplicates/scan", post(scan_duplicates))
        .route("/api/library/duplicates", get(get_duplicates))
        .route(
            "/api/library/duplicates/{group_id}/resolve",
            post(resolve_duplicate_group),
        )
        .route(
            "/api/library/duplicates/{group_id}/dismiss",
            post(dismiss_duplicate_group),
        )
        // Playback
        .route("/api/playback/state", get(get_playback_state))
        .route("/api/playback/runtime", get(get_playback_runtime))
        .route("/api/playback/play", post(play_track))
        .route("/api/playback/pause", post(pause_playback))
        .route("/api/playback/resume", post(resume_playback))
        .route("/api/playback/previous", post(previous_track))
        .route("/api/playback/next", post(next_track))
        .route("/api/playback/position", post(set_playback_position))
        .route("/api/playback/volume", post(set_playback_volume))
        .route("/api/playback/shuffle", post(set_playback_shuffle))
        .route("/api/playback/repeat", post(set_playback_repeat))
        .route("/api/playback/automix", post(set_playback_automix))
        .route(
            "/api/playback/queue",
            get(get_playback_queue).post(replace_playback_queue),
        )
        .route("/api/playback/queue/add", post(add_queue_track))
        .route("/api/playback/queue/remove", post(remove_queue_track))
        .route("/api/playback/queue/move", post(move_queue_track))
        .route("/api/playback/queue/clear", post(clear_queue_route))
        .route("/api/playlists/from-queue", post(create_playlist_from_queue))
        // Audio output settings + device enumeration
        .route("/api/audio/devices", get(get_audio_devices))
        .route(
            "/api/audio/settings",
            get(get_audio_settings).put(put_audio_settings),
        )
        // Search
        .route("/api/search", get(search))
        .route("/api/search/audio", post(search_audio))
        .route("/api/search/vibe", get(search_vibe))
        .route("/api/search/underrated", get(search_underrated))
        // TIDAL
        .route("/api/tidal/login", post(tidal_login))
        .route("/api/tidal/login/poll", post(tidal_poll))
        .route("/api/tidal/sync", post(tidal_sync_library))
        .route("/api/tidal/status", get(tidal_status))
        .route("/api/tidal/backoff", axum::routing::get(get_tidal_backoff_status))
        .route("/api/tidal/search", get(tidal_search))
        .route("/api/tidal/playlists/search", get(tidal_playlist_search))
        .route("/api/tidal/playlists/{uuid}/tracks", get(tidal_playlist_tracks))
        .route("/api/tidal/play", post(play_tidal_ephemeral))
        .route("/api/tidal/artists/{tidal_id}", get(tidal_artist_profile))
        .route("/api/tidal/logout", post(tidal_logout))
        // Spotify
        .route("/api/spotify/config", post(spotify_save_config))
        .route("/api/spotify/config", axum::routing::delete(spotify_clear_config))
        .route("/api/spotify/status", get(spotify_status))
        .route("/api/library/enrich/spotify", post(start_spotify_enrichment))
        .route("/api/library/enrich/spotify/status", get(get_spotify_enrichment_status))
        .route("/api/library/enrich/spotify/reset", post(reset_spotify_enrichment))
        // Last.fm
        .route("/api/lastfm/config", post(lastfm_save_config))
        .route("/api/lastfm/config", axum::routing::delete(lastfm_clear_config))
        .route("/api/lastfm/status", get(lastfm_status))
        .route("/api/library/enrich/lastfm", post(start_lastfm_enrichment))
        .route("/api/library/enrich/lastfm/stop", post(stop_lastfm_enrichment))
        .route("/api/library/enrich/lastfm/status", get(get_lastfm_enrichment_status))
        .route("/api/library/enrich/lastfm/reset", post(reset_lastfm_enrichment))
        // Audio analysis
        .route("/api/library/analyze/audio-features", post(start_audio_analysis))
        .route("/api/library/analyze/stop", post(stop_audio_analysis))
        .route("/api/library/analyze/status", get(get_audio_analysis_status))
        .route("/api/tracks/{id}/audio-features", get(get_track_audio_features))
        .route("/api/library/audio-features/stats", get(get_audio_features_stats))
        .route("/api/library/audio-features/quality", get(get_audio_features_quality))
        .route("/api/library/analytics", get(get_library_analytics))
        .route("/api/library/analyze/reanalyze-stale", get(reanalyze_stale_tracks))
        .route("/api/library/analyze/reset", post(reset_audio_analysis))
        .route("/api/sync/info", get(get_sync_info))
        .route("/api/sync/auto", post(set_auto_sync))
        // Status
        .route("/api/status", get(status))
        // Home page discovery endpoints
        .route("/api/home/releases", get(get_home_releases))
        .route("/api/home/picks", get(get_home_picks))
        .route("/api/home/articles", get(get_home_articles))
        .route("/api/home/news", get(get_home_news))
        // Trending / charts (Phase 5)
        .route("/api/charts", get(get_charts))
        // Server auth management
        .route("/api/server/token", get(get_server_token_handler))
        .route("/api/server/token/regenerate", post(regenerate_server_token_handler))
        // Server configuration
        .route("/api/server/info", get(get_server_info))
        .route("/api/server/host_mode", put(put_server_host_mode))
        .with_state(state)
}

async fn get_server_token_handler(
    State(state): State<SharedState>,
) -> Json<Value> {
    let s = state.read().await;
    Json(json!({ "token": s.server_token }))
}

async fn regenerate_server_token_handler(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let new_token = {
        let s = state.read().await;
        s.db.with_conn(|conn| crate::db::queries::regenerate_server_token(conn))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    {
        let mut s = state.write().await;
        s.server_token = new_token.clone();
    }
    Ok(Json(json!({ "token": new_token })))
}

async fn get_server_info(State(state): State<SharedState>) -> Json<Value> {
    let host_mode = state
        .read()
        .await
        .db
        .with_conn(|conn| {
            let v: Option<String> = conn
                .query_row(
                    "SELECT value FROM server_config WHERE key = 'server.host_mode'",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(v.map(|s| s == "true").unwrap_or(false))
        })
        .unwrap_or(false);

    let bind_address = if host_mode { "0.0.0.0:3334" } else { "127.0.0.1:3334" };
    Json(json!({
        "host_mode": host_mode,
        "bind_address": bind_address,
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn put_server_host_mode(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<Value>, StatusCode> {
    let host_mode = body
        .get("host_mode")
        .and_then(|v| v.as_bool())
        .ok_or(StatusCode::BAD_REQUEST)?;

    state
        .read()
        .await
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO server_config (key, value) VALUES ('server.host_mode', ?1)",
                rusqlite::params![if host_mode { "true" } else { "false" }],
            )?;
            Ok(())
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let bind_address = if host_mode { "0.0.0.0:3334" } else { "127.0.0.1:3334" };
    Ok(Json(json!({ "host_mode": host_mode, "bind_address": bind_address })))
}

async fn get_tracks(
    State(state): State<SharedState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    let sort_by = params.sort_by.as_deref().unwrap_or("date_added");
    let sort_dir = params.sort_dir.as_deref().unwrap_or("desc");
    let limit = params.limit.unwrap_or(50);
    let offset = params.offset.unwrap_or(0);
    let favorite_only = params.favorite_only.unwrap_or(false);

    let dsp = queries::DspFilters {
        bpm_min: params.bpm_min,
        bpm_max: params.bpm_max,
        energy_min: params.energy_min,
        energy_max: params.energy_max,
        key_signature: params.key_signature.clone(),
        instrumental_only: params.instrumental_only.unwrap_or(false),
    };

    state
        .db
        .with_conn(|conn| {
            let tracks =
                queries::get_tracks_with_dsp(conn, sort_by, sort_dir, limit, offset, favorite_only, &dsp)?;
            let total = queries::get_track_count(conn, favorite_only)?;
            Ok(Json(json!({ "tracks": tracks, "total": total })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_track_count(
    State(state): State<SharedState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Value>, StatusCode> {
    let favorite_only = params.favorite_only.unwrap_or(false);
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let count = queries::get_track_count(conn, favorite_only)?;
            Ok(Json(json!({ "count": count })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_albums(
    State(state): State<SharedState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    let sort_by = params.sort_by.as_deref().unwrap_or("title");
    let sort_dir = params.sort_dir.as_deref().unwrap_or("asc");
    let limit = params.limit.unwrap_or(100);
    let offset = params.offset.unwrap_or(0);
    let favorite_only = params.favorite_only.unwrap_or(false);

    state
        .db
        .with_conn(|conn| {
            let albums =
                queries::get_albums(conn, sort_by, sort_dir, limit, offset, favorite_only)?;
            let total = queries::get_album_count(conn, favorite_only)?;
            Ok(Json(json!({ "albums": albums, "total": total })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_artists(
    State(state): State<SharedState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    let sort_by = params.sort_by.as_deref().unwrap_or("name");
    let sort_dir = params.sort_dir.as_deref().unwrap_or("asc");
    let limit = params.limit.unwrap_or(50);
    let offset = params.offset.unwrap_or(0);

    state
        .db
        .with_conn(|conn| {
            let artists = queries::get_artists(conn, sort_by, sort_dir, limit, offset)?;
            Ok(Json(json!({ "artists": artists })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_artist_tracks(
    State(state): State<SharedState>,
    axum::extract::Path(artist_id): axum::extract::Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let tracks = queries::get_artist_tracks(conn, artist_id)?;
            Ok(Json(json!({ "tracks": tracks })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_album_tracks(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let tracks = queries::get_album_tracks(conn, id)?;
            Ok(Json(json!({ "tracks": tracks })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_artist_discography(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let tidal_artist_id = {
        let s = state.read().await;
        s.db
            .with_conn(|conn| queries::get_artist_tidal_id(conn, id))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() }))))?
    };

    let Some(tidal_artist_id) = tidal_artist_id else {
        return Ok(Json(json!({
            "albums": [],
            "top_tracks": [],
            "available": false,
            "reason": "artist_not_on_tidal"
        })));
    };

    let tokens = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
        })?;
        let s = state.read().await;
        s.tidal_tokens.clone().or(persisted)
    };

    let Some(tokens) = tokens else {
        return Ok(Json(json!({
            "albums": [],
            "top_tracks": [],
            "available": false,
            "reason": "tidal_not_connected"
        })));
    };

    let client = TidalClient::new(tokens.access_token.clone(), tokens.country_code.clone());

    let albums_fut = client.get_artist_albums(tidal_artist_id, 50, 0, Some("ALBUMS"));
    let eps_fut = client.get_artist_albums(tidal_artist_id, 50, 0, Some("EPSANDSINGLES"));
    let top_fut = client.get_artist_top_tracks(tidal_artist_id, 10, 0);

    let (albums_res, eps_res, top_res) = tokio::join!(albums_fut, eps_fut, top_fut);

    let mut all_albums: Vec<crate::services::tidal::client::TidalAlbum> = Vec::new();
    if let Ok(r) = albums_res { all_albums.extend(r.items); }
    if let Ok(r) = eps_res { all_albums.extend(r.items); }

    let tidal_album_ids: Vec<i64> = all_albums.iter().map(|a| a.id).collect();
    let known_map = {
        let s = state.read().await;
        s.db
            .with_conn(|conn| queries::get_known_album_tidal_ids(conn, &tidal_album_ids))
            .unwrap_or_default()
    };

    let albums_payload: Vec<Value> = all_albums
        .into_iter()
        .map(|a| {
            let artwork = crate::services::tidal::client::TidalClient::get_artwork_url(&a.cover, 320);
            let local_id = known_map.get(&a.id).copied();
            json!({
                "tidal_id": a.id,
                "local_id": local_id,
                "title": a.title,
                "artwork_url": artwork,
                "release_date": a.release_date,
                "release_type": a.release_type,
                "number_of_tracks": a.number_of_tracks,
                "artist_name": a.artist.name,
                "in_library": local_id.is_some()
            })
        })
        .collect();

    let top_tracks_payload: Vec<Value> = top_res
        .map(|r| {
            r.items
                .into_iter()
                .map(|t| {
                    let artwork = t
                        .album
                        .as_ref()
                        .and_then(|al| al.cover.as_ref())
                        .map(|c| crate::services::tidal::client::TidalClient::get_artwork_url(&Some(c.clone()), 160))
                        .flatten();
                    json!({
                        "tidal_id": t.id,
                        "title": t.title,
                        "duration_ms": t.duration * 1000,
                        "artwork_url": artwork,
                        "album_title": t.album.as_ref().map(|al| al.title.clone()),
                        "album_tidal_id": t.album.as_ref().map(|al| al.id),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Json(json!({
        "albums": albums_payload,
        "top_tracks": top_tracks_payload,
        "available": true
    })))
}

async fn get_tidal_album_tracks(
    State(state): State<SharedState>,
    Path(tidal_album_id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let tokens = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
        })?;
        let s = state.read().await;
        s.tidal_tokens.clone().or(persisted)
    };

    let Some(tokens) = tokens else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let client = TidalClient::new(tokens.access_token.clone(), tokens.country_code.clone());
    let result = client.get_album_tracks(tidal_album_id).await.map_err(|e| {
        (StatusCode::BAD_GATEWAY, Json(json!({ "error": e.to_string() })))
    })?;

    let tracks: Vec<Value> = result
        .items
        .into_iter()
        .map(|t| {
            let artwork = t
                .album
                .as_ref()
                .and_then(|al| al.cover.as_ref())
                .map(|c| crate::services::tidal::client::TidalClient::get_artwork_url(&Some(c.clone()), 160))
                .flatten();
            json!({
                "tidal_id": t.id,
                "title": t.title,
                "duration_ms": t.duration * 1000,
                "track_number": t.track_number,
                "disc_number": t.volume_number,
                "artist_name": t.artist.name,
                "artist_tidal_id": t.artist.id,
                "album_title": t.album.as_ref().map(|al| al.title.clone()),
                "artwork_url": artwork,
            })
        })
        .collect();

    Ok(Json(json!({ "tracks": tracks })))
}

async fn import_tidal_album(
    State(state): State<SharedState>,
    Path(tidal_album_id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (tokens, db) = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
        })?;
        let s = state.read().await;
        (s.tidal_tokens.clone().or(persisted), s.db.clone())
    };

    let Some(tokens) = tokens else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let client = TidalClient::new(tokens.access_token.clone(), tokens.country_code.clone());
    let imported = tidal_import::import_album(&db, &client, tidal_album_id)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, Json(json!({ "error": e.to_string() }))))?;

    let tracks: Vec<Value> = imported
        .tracks
        .iter()
        .map(|t| json!({ "tidal_id": t.tidal_id, "local_id": t.local_id }))
        .collect();

    Ok(Json(json!({
        "album_id": imported.album_id,
        "tracks": tracks,
    })))
}

#[derive(Debug, Deserialize)]
struct ImportTidalTrackBody {
    tidal_id: i64,
    title: String,
    artist_name: String,
    artist_tidal_id: Option<i64>,
    album_title: Option<String>,
    duration_ms: Option<i64>,
}

async fn import_tidal_track_for_radio(
    State(state): State<SharedState>,
    Json(body): Json<ImportTidalTrackBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let db = {
        let s = state.read().await;
        s.db.clone()
    };
    let imported = tidal_import::import_track_from_metadata(
        &db,
        body.tidal_id,
        body.title,
        body.artist_name,
        body.artist_tidal_id,
        body.album_title,
        body.duration_ms,
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
    })?;
    Ok(Json(json!({
        "tidal_id": imported.tidal_id,
        "local_id": imported.local_id,
    })))
}

async fn get_genres(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let genres = queries::get_genre_tree(conn)?;
            Ok(Json(json!({ "genres": genres })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_genre_heat(
    State(state): State<SharedState>,
    Query(params): Query<GenreHeatParams>,
) -> Result<Json<Value>, StatusCode> {
    let days = params.days.unwrap_or(90).max(1);
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let heat = queries::get_genre_heat(conn, days)?;
            Ok(Json(json!({ "heat": heat })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[derive(Debug, Deserialize)]
struct GenreCoOccurrenceParams {
    days: Option<i64>,
    window_minutes: Option<i64>,
    min_count: Option<i64>,
}

async fn get_genre_co_occurrence(
    State(state): State<SharedState>,
    Query(params): Query<GenreCoOccurrenceParams>,
) -> Result<Json<Value>, StatusCode> {
    let days = params.days.unwrap_or(90).max(1);
    let window = params.window_minutes.unwrap_or(30).max(5);
    let min = params.min_count.unwrap_or(3).max(1);
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let pairs = queries::get_genre_co_occurrence(conn, days, window, min)
                .map_err(|e| {
                    tracing::error!("co-occurrence query failed: {e:#}");
                    anyhow::anyhow!("co-occurrence query failed: {e:#}")
                })?;
            Ok(Json(json!({ "pairs": pairs })))
        })
        .map_err(|e| {
            tracing::error!("co-occurrence handler error: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

#[derive(Debug, Deserialize)]
struct GenreCohortParams {
    days: Option<i64>,
}

async fn get_genre_cohorts(
    State(state): State<SharedState>,
    Query(params): Query<GenreCohortParams>,
) -> Result<Json<Value>, StatusCode> {
    let days = params.days.unwrap_or(90).max(1);
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let cohorts = queries::get_genre_cohorts(conn, days)?;
            Ok(Json(json!({ "cohorts": cohorts })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[derive(Debug, Deserialize)]
struct GenreEvolutionParams {
    days: Option<i64>,
}

async fn get_genre_evolution(
    State(state): State<SharedState>,
    Query(params): Query<GenreEvolutionParams>,
) -> Result<Json<Value>, StatusCode> {
    let days = params.days.unwrap_or(90).max(7);
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let evolution = queries::get_genre_evolution(conn, days)?;
            Ok(Json(json!({ "evolution": evolution })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_genre_tracks(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Query(params): Query<GenreTrackParams>,
) -> Result<Json<Value>, StatusCode> {
    let include_descendants = params.include_descendants.unwrap_or(true);
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let tracks = queries::get_tracks_by_genre(conn, id, include_descendants)?;
            Ok(Json(json!({ "tracks": tracks })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_playlists(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let mut playlists = queries::get_playlists(conn)?;

            // Count smart playlists — if none, skip expensive loading.
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

async fn get_playlist_tracks(
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

async fn toggle_playlist_favorite_route(
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

async fn add_tracks_to_playlist_route(
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

async fn evaluate_smart_playlist(
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

async fn create_smart_playlist_route(
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

async fn update_smart_playlist_route(
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

async fn delete_smart_playlist_route(
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

async fn get_analytics_overview(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let overview = queries::get_analytics_overview(conn)?;
            Ok(Json(json!({ "overview": overview })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_recent_listens(
    State(state): State<SharedState>,
    Query(params): Query<ListenHistoryParams>,
) -> Result<Json<Value>, StatusCode> {
    let limit = params.limit.unwrap_or(25).clamp(1, 200);
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let listens = queries::get_recent_listens(conn, limit)?;
            Ok(Json(json!({ "listens": listens })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_analytics_dashboard(
    State(state): State<SharedState>,
    Query(params): Query<AnalyticsDashboardParams>,
) -> Result<Json<Value>, StatusCode> {
    let recent_limit = params.recent_limit.unwrap_or(12).clamp(1, 50);
    let top_limit = params.top_limit.unwrap_or(8).clamp(1, 20);
    let days = params.days.unwrap_or(14).clamp(1, 90);

    let state = state.read().await;
    let dashboard = state
        .db
        .with_conn(|conn| {
            Ok(crate::db::models::AnalyticsDashboard {
                overview: queries::get_analytics_overview(conn)?,
                recent_listens: queries::get_recent_listens(conn, recent_limit)?,
                top_tracks: queries::get_top_tracks_by_history(conn, top_limit)?,
                top_artists: queries::get_top_artists_by_history(conn, top_limit)?,
                top_genres: queries::get_top_genres_by_history(conn, top_limit)?,
                activity: queries::get_listen_activity(conn, days)?,
                behavior: queries::get_behavior_metrics(conn)?,
            })
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({ "dashboard": dashboard })))
}

async fn preview_discovery(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryPreviewRequest>,
) -> Result<Json<Value>, StatusCode> {
    let prompt = payload.prompt.trim();
    if prompt.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let services = payload
        .services
        .clone()
        .unwrap_or_else(|| vec!["tidal".to_string()]);
    let mode = normalize_discovery_mode(payload.mode.as_deref());
    let candidate_limit = payload.limit.unwrap_or(18).clamp(1, 40);
    let result_limit = payload.limit.unwrap_or(8).clamp(1, 20) as usize;

    let state = state.read().await;
    let recent_similar = state
        .db
        .with_conn(|conn| queries::get_similar_tracks(conn, 1, 5, &[]))
        .unwrap_or_default();
    if let Ok(Some(preview)) = discovery_learning::build_prompt_preview(
        &state.db,
        prompt,
        &mode,
        &services,
        result_limit,
        &recent_similar,
    ) {
        return Ok(Json(json!({ "preview": preview })));
    }

    let preview = state
        .db
        .with_conn(|conn| {
            let request = discovery_engine::DiscoveryPreviewRequest {
                prompt: prompt.to_string(),
                mode,
                services,
                limit: result_limit,
            };
            let context = discovery_engine::DiscoveryContext {
                overview: queries::get_analytics_overview(conn)?,
                behavior: queries::get_behavior_metrics(conn)?,
                recent_listens: queries::get_recent_listens(conn, 12)?,
                top_artists: queries::get_top_artists_by_history(conn, 6)?,
                top_genres: queries::get_top_genres_by_history(conn, 6)?,
                track_genres: queries::get_track_genre_paths(conn)?,
            };
            let candidates = queries::get_discovery_candidate_tracks(conn, candidate_limit)?;
            let preview = discovery_engine::build_preview(&request, &context, &candidates);
            queries::cache_discovery_results(conn, None, &preview.results)?;
            Ok(preview)
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({ "preview": preview })))
}

async fn discover_new_music(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryExternalRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let prompt = payload.prompt.trim();
    if prompt.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "prompt_required",
                "message": "Add a few words first so NOOR can search outward.",
            })),
        ));
    }

    let mode = normalize_discovery_mode(payload.mode.as_deref());
    let services = normalize_discovery_services(payload.services);
    if !services.iter().any(|service| service == "tidal") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "tidal_required",
                "message": "TIDAL must stay selected for real new-music discovery right now.",
            })),
        ));
    }

    let request = external_discovery_engine::ExternalDiscoveryRequest {
        prompt: prompt.to_string(),
        mode,
        services,
        limit: payload.limit.unwrap_or(10).clamp(1, 20) as usize,
    };
    let context = load_external_discovery_context(&state)
        .await
        .map_err(internal_discovery_error)?;
    let queries = external_discovery_engine::build_search_queries(&request, &context);
    let queries = augment_search_queries_with_lastfm(&state, &request, &context, queries).await;
    let provider = tidal_discovery_provider(&state).await?;
    let candidates = provider
        .search_tracks(&queries, 10)
        .await
        .map_err(discovery_upstream_error)?;
    let candidates = enrich_candidates_with_metadata(&state, candidates).await;
    let embedding_scores = discovery_learning::compute_external_embedding_scores(
        &{
            let guard = state.read().await;
            guard.db.clone()
        },
        prompt,
        &candidates,
    )
    .unwrap_or_default();
    let library_tidal_ids = existing_candidate_tidal_ids(&state, &candidates)
        .await
        .map_err(internal_discovery_error)?;
    let mut feed = external_discovery_engine::build_external_feed(
        &request,
        &context,
        &candidates,
        &library_tidal_ids,
        discovery_provider_capabilities(),
        None,
    );
    for result in &mut feed.results {
        result.embedding_score = embedding_scores.get(&result.provider_track_id).copied();
        if let Some(score) = result.embedding_score {
            result.score = (((result.score as f64) * 0.8) + (score.max(0.0) * 20.0)).round() as i32;
            if score > 0.2 {
                result.tags.push("embedding boost".to_string());
            }
        }
    }
    feed.results.sort_by(|left, right| right.score.cmp(&left.score));

    Ok(Json(json!({ "feed": feed })))
}

async fn save_discovery_track(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryExternalResultRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let provider = normalize_external_provider(&payload.provider).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "unsupported_provider",
                "message": "That discovery provider is not supported yet.",
            })),
        )
    })?;

    if provider != "tidal" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "unsupported_provider",
                "message": "Only TIDAL discovery saves are wired up right now.",
            })),
        ));
    }

    let provider = tidal_discovery_provider(&state).await?;
    provider
        .save_track(&payload.provider_track_id)
        .await
        .map_err(discovery_upstream_error)?;

    Ok(Json(json!({
        "saved": true,
        "provider": "tidal",
        "provider_track_id": payload.provider_track_id,
        "message": format!("Saved “{}” to TIDAL favorites. Run sync to pull it fully into NOOR.", payload.title),
    })))
}

async fn play_discovery_track(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryExternalResultRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let previous_track_id = current_playback_track_id(&state).await;
    let provider = normalize_external_provider(&payload.provider).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "unsupported_provider",
                "message": "That discovery provider is not supported yet.",
            })),
        )
    })?;

    if provider != "tidal" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "unsupported_provider",
                "message": "Inline playback is only wired up for TIDAL discovery right now.",
            })),
        ));
    }

    let track = discovery_result_to_track(&payload)?;
    let stream_request = tidal_stream::StreamRequest::new(
        parse_provider_track_id(&payload.provider_track_id)?,
        payload
            .audio_quality
            .clone()
            .unwrap_or_else(|| tidal_stream::DEFAULT_AUDIO_QUALITY.to_string()),
    );
    let stream_info = resolve_tidal_playback_stream(&state, &track, &stream_request)
        .await
        .map_err(|error| {
            tidal_playback_error_response(
                track.id,
                error,
                "TIDAL stream could not be resolved before discovery playback.",
            )
        })?;
    let runtime_handle = ensure_playback_runtime_for_track(&state, &track).await?;
    let crossfade_ms = current_crossfade_ms(&state).await;
    let user_quality = current_user_audio_quality(&state).await;
    let job = player::build_playback_preparation(
        &track,
        Some(&stream_info),
        crossfade_ms,
        user_quality,
    );
    runtime_handle.play(job).map_err(|error| {
        let message = format!("Failed to start host audio playback: {error}");
        report_playback_failure(&state, &message);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "status": "playback_runtime_failed",
                "message": message,
                "track_id": track.id,
            })),
        )
    })?;
    {
        let mut state_guard = state.write().await;
        state_guard.current_stream_display = Some(crate::StreamDisplayInfo {
            audio_quality: stream_info.audio_quality.clone(),
            sample_rate: stream_info.sample_rate,
            bit_depth: stream_info.bit_depth,
        });
        state_guard.pending_stream_display = None;
    }

    let snapshot = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE playback_state
                     SET current_track_id = NULL, position_ms = 0, is_playing = 1
                     WHERE id = 1",
                    [],
                )?;
                player::load_snapshot(conn)
            })
            .map_err(|error| {
                tracing::error!(
                    target: "noor.discovery.playback",
                    event = "external_playback_state_update_failed",
                    error = %error,
                    track_id = track.id,
                    "failed to persist playback state for external discovery track"
                );
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "status": "playback_state_update_failed",
                        "message": "Failed to persist playback state for the discovered track.",
                        "track_id": track.id,
                    })),
                )
            })?
    };

    sync_session_after_snapshot(
        &state,
        &snapshot,
        Some(player::ListenSessionEndReason::Replaced),
    )
    .await;
    set_external_playback_track(&state, Some(track.clone())).await;
    record_transition_if_changed(&state, previous_track_id, &snapshot, "discovery", false).await;

    let state_guard = state.read().await;
    let _ = state_guard
        .event_tx
        .send(AppEvent::TrackChanged { track_id: track.id });
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    drop(state_guard);

    let snapshot = overlay_snapshot_with_external_track(&state, snapshot).await;
    Ok(Json(json!({
        "state": snapshot.state,
        "queue": snapshot.queue
    })))
}

async fn discover_connected_music(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryConnectionsRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let prompt = payload.prompt.trim();
    if prompt.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "prompt_required",
                "message": "Keep a prompt in play so NOOR can connect the next songs.",
            })),
        ));
    }

    let provider_name = normalize_external_provider(&payload.seed.provider).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "unsupported_provider",
                "message": "That discovery provider is not supported yet.",
            })),
        )
    })?;
    if provider_name != "tidal" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "unsupported_provider",
                "message": "Only TIDAL connection trails are wired up right now.",
            })),
        ));
    }

    let request = external_discovery_engine::ExternalDiscoveryRequest {
        prompt: prompt.to_string(),
        mode: normalize_discovery_mode(payload.mode.as_deref()),
        services: normalize_discovery_services(payload.services),
        limit: payload.limit.unwrap_or(10).clamp(1, 20) as usize,
    };
    let context = load_external_discovery_context(&state)
        .await
        .map_err(internal_discovery_error)?;
    let seed = DiscoveryCandidateSeed {
        provider_track_id: payload.seed.provider_track_id.clone(),
        title: payload.seed.title.clone(),
        artist_name: payload.seed.artist_name.clone(),
        album_title: payload.seed.album_title.clone(),
        normalized_genres: payload.seed.normalized_genres.clone().unwrap_or_default(),
    };
    let queries = external_discovery_engine::build_connection_queries(&request, &context, &seed);
    let queries = augment_connection_queries_with_lastfm(&state, &seed, queries).await;
    let provider = tidal_discovery_provider(&state).await?;
    let candidates = provider
        .connected_tracks(&seed, &queries, 8)
        .await
        .map_err(discovery_upstream_error)?
        .into_iter()
        .filter(|candidate| candidate.provider_track_id != seed.provider_track_id)
        .collect::<Vec<_>>();
    let candidates = enrich_candidates_with_metadata(&state, candidates).await;
    let embedding_scores = discovery_learning::compute_external_embedding_scores(
        &{
            let guard = state.read().await;
            guard.db.clone()
        },
        prompt,
        &candidates,
    )
    .unwrap_or_default();
    let library_tidal_ids = existing_candidate_tidal_ids(&state, &candidates)
        .await
        .map_err(internal_discovery_error)?;
    let trail_item = Some(discovery_request_to_trail_item(&payload.seed));
    let mut feed = external_discovery_engine::build_external_feed(
        &request,
        &context,
        &candidates,
        &library_tidal_ids,
        discovery_provider_capabilities(),
        trail_item,
    );
    for result in &mut feed.results {
        result.embedding_score = embedding_scores.get(&result.provider_track_id).copied();
        if let Some(score) = result.embedding_score {
            result.score = (((result.score as f64) * 0.8) + (score.max(0.0) * 20.0)).round() as i32;
            if score > 0.2 {
                result.tags.push("embedding boost".to_string());
            }
        }
    }
    feed.results.sort_by(|left, right| right.score.cmp(&left.score));

    Ok(Json(json!({ "feed": feed })))
}

async fn get_discovery_presets(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    let presets = state
        .db
        .with_conn(queries::list_discovery_presets)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({ "presets": presets })))
}

async fn create_discovery_preset(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryPresetRequest>,
) -> Result<Json<Value>, StatusCode> {
    let name = payload.name.trim();
    let prompt = payload.prompt.trim();
    if name.is_empty() || prompt.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mode = normalize_discovery_mode(payload.mode.as_deref());

    let services_json = serde_json::to_string(
        &payload
            .services
            .unwrap_or_else(|| vec!["tidal".to_string()]),
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let state = state.read().await;
    let preset = state
        .db
        .with_conn(|conn| {
            queries::create_discovery_preset(conn, name, prompt, &mode, &services_json)
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({ "preset": preset })))
}

async fn get_discovery_status(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    let status = state
        .db
        .with_conn(queries::get_discovery_status)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "status": status })))
}

async fn get_discovery_training_status(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    let run = state
        .db
        .with_conn(queries::get_latest_training_run)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Build a synthetic per-stage breakdown so the frontend can render a
    // pipeline view without needing multi-row stage history in the schema.
    const STAGE_ORDER: &[&str] = &[
        "corpus", "behavioral", "audio", "fusion", "neighbors", "evaluate",
    ];
    const STAGE_THRESHOLDS: &[f64] = &[0.05, 0.2, 0.55, 0.72, 0.88, 0.96];

    let stages: Vec<Value> = if let Some(ref r) = run {
        let current_stage_idx = STAGE_ORDER
            .iter()
            .position(|&s| s == r.stage)
            .unwrap_or(0);
        STAGE_ORDER
            .iter()
            .enumerate()
            .map(|(i, &name)| {
                let stage_status = if r.status == "failed" && i == current_stage_idx {
                    "failed"
                } else if i < current_stage_idx {
                    "done"
                } else if i == current_stage_idx {
                    r.status.as_str()
                } else {
                    "pending"
                };
                let progress = if i < current_stage_idx {
                    1.0_f64
                } else if i == current_stage_idx {
                    let lo = if i == 0 { 0.0 } else { STAGE_THRESHOLDS[i - 1] };
                    let hi = STAGE_THRESHOLDS[i];
                    ((r.progress - lo) / (hi - lo)).clamp(0.0, 1.0)
                } else {
                    0.0_f64
                };
                json!({ "stage": name, "status": stage_status, "progress": progress })
            })
            .collect()
    } else {
        Vec::new()
    };

    Ok(Json(json!({ "run": run, "stages": stages })))
}

async fn start_discovery_training(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryTrainRequest>,
) -> Result<Json<Value>, StatusCode> {
    use std::sync::atomic::Ordering;

    let mode = payload.mode.as_deref().unwrap_or("incremental");
    let full_mode = mode == "full";
    let rebuild_audio = payload.rebuild_audio.unwrap_or(false);
    let (db, cancel) = {
        let guard = state.read().await;
        (guard.db.clone(), guard.discovery_train_cancel.clone())
    };

    // Guard: reject if a run is already in progress
    let already_running = db
        .with_conn(|conn| queries::get_latest_training_run(conn))
        .ok()
        .flatten()
        .map(|run| run.status == "running")
        .unwrap_or(false);

    if already_running {
        return Ok(Json(json!({
            "status": "already_running",
            "mode": mode
        })));
    }

    // Reset cancel flag synchronously before spawning so that a Stop request
    // arriving immediately after this call reaches the spawned task.
    cancel.store(false, Ordering::SeqCst);

    tokio::spawn(async move {
        let event_tx = {
            let guard = state.read().await;
            guard.event_tx.clone()
        };
        if let Err(error) =
            discovery_learning::start_training(db, event_tx, full_mode, rebuild_audio, cancel).await
        {
            tracing::error!(
                target: "noor.discovery.training",
                error = %error,
                "discovery learning pipeline failed"
            );
        }
    });
    Ok(Json(json!({
        "status": "training_started",
        "mode": if full_mode { "full" } else { "incremental" }
    })))
}

async fn stop_discovery_training(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use std::sync::atomic::Ordering;
    let s = state.read().await;
    s.discovery_train_cancel.store(true, Ordering::Relaxed);
    Ok(Json(json!({ "status": "stopping" })))
}

async fn record_discovery_feedback(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryFeedbackRequest>,
) -> Result<Json<Value>, StatusCode> {
    let context_json = payload.context.as_ref().map(Value::to_string);
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            queries::record_discovery_feedback(
                conn,
                payload.seed_track_id,
                payload.candidate_track_id,
                &payload.action,
                &payload.surface,
                context_json.as_deref(),
                payload.session_id.as_deref(),
            )
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({ "recorded": true })))
}

// ─── Similar Radio ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct RadioRequest {
    seed_track_id: Option<i64>,
    seed_tidal_id: Option<i64>,  // resolve to local library track when seed_track_id <= 0
    creativity: Option<f64>,    // 0.0 (tight) to 1.0 (adventurous), default 0.3
    context_window: Option<i64>, // number of recent tracks to influence, default 5
    limit: Option<i64>,          // results to return, default 20
    exclude_ids: Option<Vec<i64>>, // already-played track IDs
}

/// Get similar tracks for the "Similar Radio" feature.
/// Combines pre-computed similarity scores with creativity/context adjustments.
async fn get_radio_tracks(
    State(state): State<SharedState>,
    Json(payload): Json<RadioRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let creativity = payload.creativity.unwrap_or(0.3).clamp(0.0, 1.0);
    let context_window = payload.context_window.unwrap_or(5).max(0) as usize;
    let limit = payload.limit.unwrap_or(20).max(1).min(50);
    let exclude_ids = payload.exclude_ids.unwrap_or_default();

    let state = state.read().await;

    let seed_track_id: i64 = if let Some(id) = payload.seed_track_id.filter(|&id| id > 0) {
        id
    } else if let Some(tidal_id) = payload.seed_tidal_id {
        state
            .db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT id FROM tracks WHERE tidal_id = ?1 LIMIT 1",
                    params![tidal_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?)
            })
            .map_err(|e| {
                tracing::error!("DB error resolving tidal_id {}: {}", tidal_id, e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": "Database error"})),
                )
            })?
            .ok_or_else(|| (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "No local track matches that Tidal ID"})),
            ))?
    } else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "seed_track_id or seed_tidal_id required"})),
        ));
    };

    if let Some(mut rows) = discovery_learning::radio_from_neighbors(
        &state.db,
        seed_track_id,
        &exclude_ids,
        limit,
        creativity,
    )
    .map_err(|e| {
        tracing::error!("Failed to load embedding neighbors: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "Failed to query learned neighbors"})),
        )
    })?
    {
        // DSP harmonic post-scoring — apply the shared harmonic multiplier to
        // every row that has audio features on both sides. Rows without
        // features are left untouched (never penalised for being unanalyzed).
        let seed_features = state
            .db
            .with_conn(|conn| queries::get_audio_dsp_features(conn, seed_track_id))
            .ok()
            .flatten();

        if let Some(seed) = seed_features.as_ref() {
            for row in rows.iter_mut() {
                let cand = state
                    .db
                    .with_conn(|conn| queries::get_audio_dsp_features(conn, row.track_id))
                    .ok()
                    .flatten();
                if let Some(cand) = cand {
                    let mult = crate::services::audio_analysis::compute_harmonic_multiplier(
                        seed.camelot_key.as_deref(),
                        cand.camelot_key.as_deref(),
                        seed.bpm,
                        cand.bpm,
                    );
                    row.adjusted_score *= mult;
                    if mult > 1.5 && !row.reason_tags.iter().any(|t| t == "harmonic match") {
                        row.reason_tags.push("harmonic match".to_string());
                    }
                }
            }
            // Re-sort by adjusted_score descending after the multiplier pass.
            rows.sort_by(|a, b| {
                b.adjusted_score
                    .partial_cmp(&a.adjusted_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        let model = state
            .db
            .with_conn(queries::get_active_embedding_model)
            .ok()
            .flatten();
        return Ok(Json(json!({
            "tracks": rows,
            "seed_track_id": seed_track_id,
            "creativity": creativity,
            "context_window": context_window,
            "computed_at": model.as_ref().and_then(|m| m.trained_at.clone()),
            "model_family": model.as_ref().map(|m| m.family.clone()),
            "model_key": model.as_ref().map(|m| m.model_key.clone()),
            "reasons": ["learned neighbors", "session feedback", "taste graph", "harmonic post-scoring"],
        })));
    }

    // Get similar tracks from pre-computed similarity table
    let similar = state
        .db
        .with_conn(|conn| {
            queries::get_similar_tracks(conn, seed_track_id, limit * 3, &exclude_ids)
        })
        .map_err(|e| {
            tracing::error!("Failed to get similar tracks: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "Failed to query similar tracks"})),
            )
        })?;

    if similar.is_empty() {
        // Fallback: return random tracks from the same artist/genre
        return Ok(Json(json!({
            "tracks": [],
            "message": "No similar tracks found. Try syncing your library or running similarity computation.",
            "computed_at": null,
        })));
    }

    // Apply creativity filter: higher creativity = pick from further down the list
    // We use a temperature-based sampling: sort by adjusted score with noise
    let temperature = creativity * 0.5; // 0.0 = deterministic, 0.5 = max noise

    use rand::Rng;
    let mut rng = rand::thread_rng();

    let mut scored: Vec<_> = similar
        .into_iter()
        .map(|track| {
            // Add noise proportional to creativity
            let noise = rng.gen_range(0.0..=temperature);
            let adjusted_score = track.similarity_score * (1.0 - temperature) + noise;
            (track, adjusted_score)
        })
        .collect();

    // Sort by adjusted score
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Take top `limit` results
    let results: Vec<_> = scored
        .into_iter()
        .take(limit as usize)
        .map(|(track, adjusted_score)| {
            json!({
                "track_id": track.track_id,
                "title": track.title,
                "artist_name": track.artist_name,
                "album_title": track.album_title,
                "artwork_url": track.artwork_url,
                "duration_ms": track.duration_ms,
                "best_quality": track.best_quality,
                "similarity_score": track.similarity_score,
                "adjusted_score": adjusted_score,
                "co_listen_score": track.co_listen_score,
                "co_album_score": track.co_album_score,
                "co_artist_score": track.co_artist_score,
                "genre_proximity": track.genre_proximity,
                "reason_tags": Vec::<String>::new(),
                "model_key": Value::Null,
                "source_mode": "legacy",
            })
        })
        .collect();

    // Get computation timestamp
    let computed_at = state
        .db
        .with_conn(|conn| queries::get_similarity_computed_at(conn))
        .ok()
        .flatten();

    Ok(Json(json!({
        "tracks": results,
        "seed_track_id": seed_track_id,
        "creativity": creativity,
        "context_window": context_window,
        "computed_at": computed_at,
        "model_family": Value::Null,
        "model_key": Value::Null,
        "reasons": ["legacy similarity fallback"],
    })))
}

/// Trigger background similarity computation.
async fn compute_radio_similarity(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    // Clone the DB handle before spawning so we don't hold the RwLock during computation
    let db = {
        let s = state.read().await;
        s.db.clone()
    };
    tokio::spawn(async move {
        tracing::info!(target: "noor.radio", "Starting track similarity computation...");
        match db.with_conn(|conn| queries::compute_track_similarity(conn)) {
            Ok(count) => {
                tracing::info!(
                    target: "noor.radio",
                    count = count,
                    "Track similarity computation complete"
                );
            }
            Err(e) => {
                tracing::error!(target: "noor.radio", "Similarity computation failed: {}", e);
            }
        }
    });

    Ok(Json(json!({
        "status": "computation_started",
        "message": "Similarity computation running in background. This may take a few minutes for large libraries."
    })))
}

// ─── Discovery Sound Space ───────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct DiscoverySpaceRequest {
    mode: Option<String>,
    seed_track_id: Option<i64>,
    prompt: Option<String>,
    creativity: Option<f64>,
    limit: Option<i64>,
    include_artists: Option<bool>,
}

async fn get_discovery_space(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoverySpaceRequest>,
) -> Result<Json<Value>, StatusCode> {
    let mode = payload.mode.unwrap_or_else(|| "radio".to_string());
    let limit = payload.limit.unwrap_or(60).max(1).min(200);
    let seed_id = payload.seed_track_id.unwrap_or(0);
    let prompt = payload.prompt.as_deref().unwrap_or("").trim().to_string();

    let mut state_guard = state.read().await;

    #[derive(Debug)]
    struct SpaceTrack {
        track_id: i64,
        title: String,
        artist_name: String,
        album_title: Option<String>,
        artwork_url: Option<String>,
        duration_ms: Option<i64>,
        similarity_score: f64,
        source: String,
        energy: Option<f64>,
        danceability: Option<f64>,
        bpm: Option<f64>,
        key_signature: Option<String>,
        camelot_key: Option<String>,
        is_instrumental: Option<bool>,
        loudness_lufs: Option<f64>,
        skip_rate: Option<f64>,
        completion_avg: Option<f64>,
        cohort_id: Option<String>,
        cohort_label: Option<String>,
        top_genre: Option<String>,
        top_genre_source: Option<String>,
        top_genre_confidence: Option<f64>,
        last_played_at: Option<String>,
        play_count: i64,
        is_in_library: bool,
        radio_source: Option<String>,  // "library" | "lastfm" | "engine"
        radio_reason: Option<String>,
    }

    // ── 1. Decide track set based on inputs ──────────────────────────────────
    //
    //   prompt set   → rank_candidates (text/genre/affinity scoring)
    //   seed_id set  → radio_from_neighbors (embedding graph)
    //   neither      → most-played fallback

    let mut space_tracks: Vec<SpaceTrack> = if !prompt.is_empty() {
        // Prompt path: run the full discovery scoring engine against the library
        let p = prompt.clone();
        let lim = limit;
        state_guard.db.with_conn(move |conn| {
            let request = discovery_engine::DiscoveryPreviewRequest {
                prompt: p.clone(),
                mode: "mood".to_string(),
                services: vec!["tidal".to_string()],
                limit: lim as usize,
            };
            let context = discovery_engine::DiscoveryContext {
                overview: queries::get_analytics_overview(conn)?,
                behavior: queries::get_behavior_metrics(conn)?,
                recent_listens: queries::get_recent_listens(conn, 12)?,
                top_artists: queries::get_top_artists_by_history(conn, 6)?,
                top_genres: queries::get_top_genres_by_history(conn, 6)?,
                track_genres: queries::get_track_genre_paths(conn)?,
            };
            let candidates = queries::get_discovery_candidate_tracks(conn, lim * 4)?;
            let preview = discovery_engine::build_preview(&request, &context, &candidates);
            Ok(preview.results.into_iter().map(|r| SpaceTrack {
                track_id: r.track_id,
                title: r.title,
                artist_name: r.artist_name.as_deref().unwrap_or("").to_string(),
                album_title: r.album_title,
                artwork_url: r.artwork_url,
                duration_ms: r.duration_ms,
                similarity_score: (r.score as f64 / 99.0).clamp(0.0, 1.0),
                source: r.service,
                energy: None,
                danceability: None,
                bpm: None,
                key_signature: None,
                camelot_key: None,
                is_instrumental: None,
                loudness_lufs: None,
                skip_rate: None,
                completion_avg: None,
                cohort_id: None,
                cohort_label: None,
                top_genre: None,
                top_genre_source: None,
                top_genre_confidence: None,
                last_played_at: None,
                play_count: 0,
                is_in_library: true,
                radio_source: None,
                radio_reason: None,
            }).collect::<Vec<_>>())
        }).unwrap_or_default()
    } else if seed_id > 0 {
        let db = state_guard.db.clone();
        let lastfm = crate::metadata::lastfm::LastFmClient::load(state_guard.http_client.clone(), &state_guard.db);
        drop(state_guard);

        let queue = crate::services::radio::orchestrate_song(
            &db,
            lastfm.as_ref(),
            seed_id,
            crate::services::radio::RadioBlend::Mixed,
            limit as usize,
            &[],
        )
        .await
        .ok();

        state_guard = state.read().await;

        if let Some(queue) = queue {
            queue
                .tracks
                .into_iter()
                .map(|c| SpaceTrack {
                    track_id: if c.is_in_library { c.track_id } else { c.tidal_track_id.unwrap_or(c.track_id) },
                    title: c.title,
                    artist_name: c.artist_name,
                    album_title: c.album_title,
                    artwork_url: c.artwork_url,
                    duration_ms: c.duration_ms,
                    similarity_score: c.similarity_score,
                    source: match c.source {
                        crate::services::radio::RadioSource::Library => "tidal".to_string(),
                        _ => "external".to_string(),
                    },
                    energy: None,
                    danceability: None,
                    bpm: None,
                    key_signature: None,
                    camelot_key: None,
                    is_instrumental: None,
                    loudness_lufs: None,
                    skip_rate: None,
                    completion_avg: None,
                    cohort_id: None,
                    cohort_label: None,
                    top_genre: None,
                    top_genre_source: None,
                    top_genre_confidence: None,
                    last_played_at: None,
                    play_count: 0,
                    is_in_library: c.is_in_library,
                    radio_source: Some(match c.source {
                        crate::services::radio::RadioSource::Library => "library".to_string(),
                        crate::services::radio::RadioSource::Lastfm => "lastfm".to_string(),
                        crate::services::radio::RadioSource::Engine => "engine".to_string(),
                    }),
                    radio_reason: Some(c.reason),
                })
                .collect()
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    // ── 1b. Prepend the seed track itself when in seed mode (so canvas has center) ──
    if seed_id > 0 && prompt.is_empty() {
        // Avoid duplicating if it somehow ended up in the candidate list.
        let already_present = space_tracks.iter().any(|t| t.track_id == seed_id);
        if !already_present {
            let seed_track_opt: Option<(i64, String, Option<String>, Option<String>, Option<String>, Option<i64>, Option<String>)> =
                state_guard.db.with_conn(|conn| {
                    Ok(conn.query_row(
                        "SELECT t.id, t.title, ar.name, al.title, al.artwork_url, t.duration_ms, t.source
                         FROM tracks t
                         LEFT JOIN artists ar ON t.artist_id = ar.id
                         LEFT JOIN albums al ON t.album_id = al.id
                         WHERE t.id = ?1",
                        rusqlite::params![seed_id],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, Option<String>>(2)?,
                                row.get::<_, Option<String>>(3)?,
                                row.get::<_, Option<String>>(4)?,
                                row.get::<_, Option<i64>>(5)?,
                                row.get::<_, Option<String>>(6)?,
                            ))
                        },
                    ).ok())
                }).unwrap_or(None);

            if let Some((id, title, artist, album, artwork, dur, src)) = seed_track_opt {
                space_tracks.insert(0, SpaceTrack {
                    track_id: id,
                    title,
                    artist_name: artist.unwrap_or_default(),
                    album_title: album,
                    artwork_url: artwork,
                    duration_ms: dur,
                    similarity_score: 1.0,
                    source: src.unwrap_or_else(|| "tidal".to_string()),
                    energy: None,
                    danceability: None,
                    bpm: None,
                    key_signature: None,
                    camelot_key: None,
                    is_instrumental: None,
                    loudness_lufs: None,
                    skip_rate: None,
                    completion_avg: None,
                    cohort_id: None,
                    cohort_label: None,
                    top_genre: None,
                    top_genre_source: None,
                    top_genre_confidence: None,
                    last_played_at: None,
                    play_count: 0,
                    is_in_library: true,
                    radio_source: None,
                    radio_reason: None,
                });
            }
        }
    }

    // ── 2. Fill remainder from most-played library tracks ────────────────────
    let seeded_ids: HashSet<i64> = space_tracks.iter().map(|t| t.track_id).collect();
    let remaining = limit - space_tracks.len() as i64;
    if remaining > 0 {
        let fallback = state_guard.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT t.id, t.title, a.name, al.title, al.artwork_url, t.duration_ms, t.source
                 FROM tracks t
                 LEFT JOIN artists a ON t.artist_id = a.id
                 LEFT JOIN albums al ON t.album_id = al.id
                 ORDER BY t.play_count DESC, t.date_added DESC
                 LIMIT ?1",
            )?;
            let rows = stmt.query_map([limit], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })?;
            let mut result = Vec::new();
            for r in rows { result.push(r?); }
            Ok(result)
        }).unwrap_or_default();

        for (id, title, artist, album, artwork, dur, src) in fallback {
            if !seeded_ids.contains(&id) {
                space_tracks.push(SpaceTrack {
                    track_id: id,
                    title,
                    artist_name: artist.unwrap_or_default(),
                    album_title: album,
                    artwork_url: artwork,
                    duration_ms: dur,
                    similarity_score: 0.5,
                    source: src.unwrap_or_else(|| "tidal".to_string()),
                    energy: None,
                    danceability: None,
                    bpm: None,
                    key_signature: None,
                    camelot_key: None,
                    is_instrumental: None,
                    loudness_lufs: None,
                    skip_rate: None,
                    completion_avg: None,
                    cohort_id: None,
                    cohort_label: None,
                    top_genre: None,
                    top_genre_source: None,
                    top_genre_confidence: None,
                    last_played_at: None,
                    play_count: 0,
                    is_in_library: true,
                    radio_source: None,
                    radio_reason: None,
                });
            }
        }
        space_tracks.truncate(limit as usize);
    }

    // ── 3. Fetch DSP features for all collected track IDs ────────────────────
    if !space_tracks.is_empty() {
        let ids_csv: String = space_tracks
            .iter()
            .filter(|t| t.is_in_library)
            .map(|t| t.track_id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        if ids_csv.is_empty() {
            // No library tracks present (pure external response) — nothing to enrich.
        } else {
            type DspRow = (
                Option<f64>, // energy
                Option<f64>, // danceability
                Option<f64>, // bpm
                Option<String>, // key_signature
                Option<String>, // camelot_key
                Option<i64>, // is_instrumental (0/1)
                Option<f64>, // loudness_lufs
            );
            let dsp_map: std::collections::HashMap<i64, DspRow> = state_guard.db.with_conn(|conn| {
                let sql = format!(
                    "SELECT track_id, energy, danceability, bpm, key_signature, camelot_key,
                            is_instrumental, loudness_lufs
                     FROM audio_dsp_features WHERE track_id IN ({ids_csv})"
                );
                let mut stmt = conn.prepare(&sql)?;
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<f64>>(1)?,
                        row.get::<_, Option<f64>>(2)?,
                        row.get::<_, Option<f64>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, Option<f64>>(7)?,
                    ))
                })?;
                let mut map = std::collections::HashMap::new();
                for r in rows {
                    let (id, energy, dance, bpm, key, camelot, instr, lufs) = r?;
                    map.insert(id, (energy, dance, bpm, key, camelot, instr, lufs));
                }
                Ok(map)
            }).unwrap_or_default();

            for t in &mut space_tracks {
                if let Some((energy, dance, bpm, key, camelot, instr, lufs)) = dsp_map.get(&t.track_id) {
                    t.energy = *energy;
                    t.danceability = *dance;
                    t.bpm = *bpm;
                    t.key_signature = key.clone();
                    t.camelot_key = camelot.clone();
                    t.is_instrumental = instr.map(|v| v != 0);
                    t.loudness_lufs = *lufs;
                }
            }
        }
    }

    // ── 3b. Aggregate skip-rate + completion-avg from listen_history ─────────
    if !space_tracks.is_empty() {
        let ids_csv: String = space_tracks
            .iter()
            .filter(|t| t.is_in_library)
            .map(|t| t.track_id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        if ids_csv.is_empty() {
            // No library tracks present (pure external response) — nothing to enrich.
        } else {
            let listen_map: std::collections::HashMap<i64, (Option<f64>, Option<f64>)> = state_guard.db.with_conn(|conn| {
                let sql = format!(
                    "SELECT lh.track_id,
                            AVG(CASE WHEN lh.completed = 1 THEN 0.0 ELSE 1.0 END) AS skip_rate,
                            AVG(
                                CASE
                                    WHEN t.duration_ms IS NULL OR t.duration_ms = 0 THEN NULL
                                    ELSE MIN(1.0, CAST(lh.duration_listened_ms AS REAL) / CAST(t.duration_ms AS REAL))
                                END
                            ) AS completion_avg
                     FROM listen_history lh
                     JOIN tracks t ON t.id = lh.track_id
                     WHERE lh.track_id IN ({ids_csv})
                     GROUP BY lh.track_id"
                );
                let mut stmt = conn.prepare(&sql)?;
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<f64>>(1)?,
                        row.get::<_, Option<f64>>(2)?,
                    ))
                })?;
                let mut map = std::collections::HashMap::new();
                for r in rows {
                    let (id, skip, comp) = r?;
                    map.insert(id, (skip, comp));
                }
                Ok(map)
            }).unwrap_or_default();

            for t in &mut space_tracks {
                if let Some((skip, comp)) = listen_map.get(&t.track_id) {
                    // Preserve Option semantics — None means "no listen data" (distinct from 0.0).
                    t.skip_rate = *skip;
                    t.completion_avg = *comp;
                }
            }
        }
    }

    // ── 3c. Backfill last_played_at + play_count from tracks table ───────────
    if !space_tracks.is_empty() {
        let ids_csv: String = space_tracks
            .iter()
            .filter(|t| t.is_in_library)
            .map(|t| t.track_id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        if ids_csv.is_empty() {
            // No library tracks present (pure external response) — nothing to enrich.
        } else {
            let track_meta: std::collections::HashMap<i64, (Option<String>, i64)> = state_guard.db.with_conn(|conn| {
                let sql = format!(
                    "SELECT id, last_played_at, play_count FROM tracks WHERE id IN ({ids_csv})"
                );
                let mut stmt = conn.prepare(&sql)?;
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    ))
                })?;
                let mut map = std::collections::HashMap::new();
                for r in rows {
                    let (id, last, plays) = r?;
                    map.insert(id, (last, plays));
                }
                Ok(map)
            }).unwrap_or_default();

            for t in &mut space_tracks {
                if let Some((last, plays)) = track_meta.get(&t.track_id) {
                    t.last_played_at = last.clone();
                    t.play_count = *plays;
                }
            }
        }
    }

    // ── 3d. Top-genre with source + confidence (highest confidence per track) ─
    if !space_tracks.is_empty() {
        let ids_csv: String = space_tracks
            .iter()
            .filter(|t| t.is_in_library)
            .map(|t| t.track_id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        if ids_csv.is_empty() {
            // No library tracks present (pure external response) — nothing to enrich.
        } else {
            type GenreRow = (String, Option<String>, Option<f64>);
            let genre_map: std::collections::HashMap<i64, GenreRow> = state_guard.db.with_conn(|conn| {
                let sql = format!(
                    "SELECT tg.track_id, g.name, tg.source, tg.confidence
                     FROM track_genres tg
                     JOIN genres g ON g.id = tg.genre_id
                     WHERE tg.track_id IN ({ids_csv})
                     ORDER BY tg.track_id, COALESCE(tg.confidence, 0) DESC"
                );
                let mut stmt = conn.prepare(&sql)?;
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<f64>>(3)?,
                    ))
                })?;
                let mut map = std::collections::HashMap::new();
                for r in rows {
                    let (id, name, source, conf) = r?;
                    map.entry(id).or_insert((name, source, conf));
                }
                Ok(map)
            }).unwrap_or_default();

            for t in &mut space_tracks {
                if let Some((name, source, conf)) = genre_map.get(&t.track_id) {
                    t.top_genre = Some(name.clone());
                    t.top_genre_source = source.clone();
                    t.top_genre_confidence = *conf;
                }
            }
        }
    }

    // ── 3e. Cohort assignment per track (90-day window) ──────────────────────
    if !space_tracks.is_empty() {
        let track_ids: Vec<i64> = space_tracks
            .iter()
            .filter(|t| t.is_in_library)
            .map(|t| t.track_id)
            .collect();

        if track_ids.is_empty() {
            // No library tracks — skip cohort assignment.
        } else {
            let cohort_map: std::collections::HashMap<i64, (String, String)> = state_guard.db.with_conn(|conn| {
                queries::get_track_cohort_assignments(conn, &track_ids, 90)
            }).unwrap_or_default();

            for t in &mut space_tracks {
                if let Some((id, label)) = cohort_map.get(&t.track_id) {
                    t.cohort_id = Some(id.clone());
                    t.cohort_label = Some(label.clone());
                }
            }
        }
    }

    // ── 4. Build edges ───────────────────────────────────────────────────────
    // Seed-based external mode: radial spokes from seed → each external candidate.
    // Otherwise: pull from the pre-computed neighbor graph (Phase 1 behavior).
    let is_external_seed_mode = seed_id > 0
        && prompt.is_empty()
        && space_tracks.iter().any(|t| !t.is_in_library);

    let edges: Vec<Value> = if is_external_seed_mode {
        space_tracks
            .iter()
            .filter(|t| !t.is_in_library)
            .map(|t| {
                json!({
                    "from_id": seed_id,
                    "to_id": t.track_id,
                    "type": "behavioural",
                    "weight": t.similarity_score,
                    "reason_tags": ["external_match"],
                    "behavioral_score": 0.0,
                    "audio_score": 0.0,
                    "metadata_score": t.similarity_score,
                })
            })
            .collect()
    } else {
        let track_id_set: HashSet<i64> = space_tracks.iter().map(|t| t.track_id).collect();
        if track_id_set.len() > 1 {
            let ids_csv: String = track_id_set.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
            state_guard.db.with_conn(|conn| {
                let sql = format!(
                    "SELECT n.track_id, n.neighbor_track_id, n.score,
                            n.behavioral_score, n.audio_score, n.metadata_score, n.reason_json
                     FROM track_neighbors n
                     WHERE n.track_id IN ({ids_csv}) AND n.neighbor_track_id IN ({ids_csv})
                     ORDER BY n.score DESC
                     LIMIT 300"
                );
                let mut stmt = conn.prepare(&sql)?;
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, f64>(2)?,
                        row.get::<_, f64>(3)?,
                        row.get::<_, f64>(4)?,
                        row.get::<_, f64>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                })?;
                let mut result = Vec::new();
                for r in rows { result.push(r?); }
                Ok(result)
            })
            .unwrap_or_default()
            .into_iter()
            .map(|(from_id, to_id, score, behavioral, audio, metadata, reason_json)| {
                let parsed: Vec<Value> = reason_json
                    .as_deref()
                    .and_then(|s| serde_json::from_str::<Vec<Value>>(s).ok())
                    .unwrap_or_default();
                let tags: Vec<String> = parsed
                    .iter()
                    .filter_map(|v| {
                        v.get("key")
                            .and_then(|k| k.as_str())
                            .or_else(|| v.get("label").and_then(|l| l.as_str()))
                            .map(|s| s.to_string())
                    })
                    .collect();

                let edge_type = if tags.iter().any(|t| t == "genre_branch") && audio > 0.4 {
                    "harmonic"
                } else if behavioral > 0.4 {
                    "behavioural"
                } else if tags.iter().any(|t| t == "artist_affinity") {
                    "genre"
                } else if metadata > 0.3 {
                    "bpm_match"
                } else {
                    "behavioural"
                };

                json!({
                    "from_id": from_id,
                    "to_id": to_id,
                    "type": edge_type,
                    "weight": score.clamp(0.0, 1.0),
                    "reason_tags": tags,
                    "behavioral_score": behavioral,
                    "audio_score": audio,
                    "metadata_score": metadata,
                })
            })
            .collect()
        } else {
            vec![]
        }
    };

    // ── 5. Build spatial layout ──────────────────────────────────────────────
    let total = space_tracks.len().max(1);
    let track_nodes: Vec<Value> = space_tracks
        .into_iter()
        .enumerate()
        .map(|(i, t)| {
            let (x, y) = match mode.as_str() {
                "energy_arc" => {
                    let energy = t.energy.unwrap_or(0.5);
                    let jitter_x = (i as f64 * 17.3).sin() * 60.0;
                    let jitter_y = (i as f64 * 31.7).cos() * 200.0;
                    ((energy - 0.5) * 800.0 + jitter_x, jitter_y)
                }
                "harmonic" => {
                    if let Some(ref ck) = t.camelot_key {
                        let num = ck.chars().take_while(|c| c.is_ascii_digit())
                            .collect::<String>().parse::<f64>().unwrap_or(1.0);
                        let is_a = ck.contains('A');
                        let angle = ((num - 1.0) / 12.0) * std::f64::consts::PI * 2.0
                            + if is_a { 0.0 } else { 0.26 };
                        let r = 200.0 + (i as f64 * 23.0).sin() * 80.0;
                        (angle.cos() * r, angle.sin() * r)
                    } else {
                        let angle = (i as f64 / total as f64) * std::f64::consts::PI * 2.0;
                        (angle.cos() * 350.0, angle.sin() * 350.0)
                    }
                }
                _ => {
                    let angle = (i as f64 / total as f64) * std::f64::consts::PI * 2.0;
                    let r = 80.0 + (1.0 - t.similarity_score) * 300.0
                        + (i as f64 * 37.0).sin() * 50.0;
                    (angle.cos() * r, angle.sin() * r)
                }
            };
            let node_radius = 5.0 + t.similarity_score * 20.0 + t.energy.unwrap_or(0.5) * 5.0;
            json!({
                "track_id": t.track_id,
                "title": t.title,
                "artist_name": t.artist_name,
                "album_title": t.album_title,
                "artwork_url": t.artwork_url,
                "duration_ms": t.duration_ms,
                "similarity_score": t.similarity_score,
                "energy": t.energy,
                "danceability": t.danceability,
                "bpm": t.bpm,
                "key_signature": t.key_signature,
                "camelot_key": t.camelot_key,
                "is_instrumental": t.is_instrumental,
                "loudness_lufs": t.loudness_lufs,
                "skip_rate": t.skip_rate,
                "completion_avg": t.completion_avg,
                "cohort_id": t.cohort_id,
                "cohort_label": t.cohort_label,
                "top_genre": t.top_genre,
                "top_genre_source": t.top_genre_source,
                "top_genre_confidence": t.top_genre_confidence,
                "last_played_at": t.last_played_at,
                "play_count": t.play_count,
                "is_in_library": t.is_in_library,
                "source": t.source,
                "radio_source": t.radio_source,
                "radio_reason": t.radio_reason,
                "x": x,
                "y": y,
                "vx": 0.0,
                "vy": 0.0,
                "radius": node_radius,
                "opacity": 0.0,
            })
        })
        .collect();

    Ok(Json(json!({
        "tracks": track_nodes,
        "artists": [],
        "edges": edges,
    })))
}

#[derive(Debug, Deserialize)]
struct RadioSongRequest {
    seed_track_id: i64,
    #[serde(default)]
    blend: Option<crate::services::radio::RadioBlend>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    exclude_track_ids: Option<Vec<i64>>,
}

async fn radio_song(
    State(state): State<SharedState>,
    Json(payload): Json<RadioSongRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    // Reject ephemeral Tidal ids (negative) and zero up front. The
    // orchestrator can only resolve positive library ids; previous
    // behaviour was a 500 with no body and a WARN log, which made
    // mis-routed callers (e.g. menu dispatch bugs) look like server
    // failures rather than bad inputs. Hand back a 400 with a hint
    // pointing at the right endpoint for Tidal-only seeds.
    if payload.seed_track_id <= 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "seed_track_id must be a positive library id",
                "hint": "for Tidal-only seeds, POST /api/discovery/radio with seed_tidal_id"
            })),
        ));
    }

    let blend = payload.blend.unwrap_or_default();
    let limit = payload.limit.unwrap_or(60).clamp(8, 200);
    let exclude = payload.exclude_track_ids.unwrap_or_default();

    let (db, lastfm) = {
        let g = state.read().await;
        let lastfm = crate::metadata::lastfm::LastFmClient::load(g.http_client.clone(), &g.db);
        (g.db.clone(), lastfm)
    };

    let queue = crate::services::radio::orchestrate_song(
        &db,
        lastfm.as_ref(),
        payload.seed_track_id,
        blend,
        limit,
        &exclude,
    )
    .await
    .map_err(|e| {
        tracing::warn!("radio_song failed: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "radio orchestration failed" })),
        )
    })?;

    Ok(Json(serde_json::to_value(queue).unwrap_or(json!({}))))
}

// ─── POST /api/radio/start ───────────────────────────────────────────────────
//
// Atomically builds a radio queue from a seed track, inserting library tracks
// directly and non-library Last.fm results as pending rows, then spawns
// background resolvers bounded by RESOLVER_POOL_SIZE.

#[derive(Debug, Deserialize)]
struct RadioStartRequest {
    seed_track_id: i64,
    #[serde(default)]
    blend: Option<crate::services::radio::RadioBlend>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn radio_start(
    State(state): State<SharedState>,
    Json(payload): Json<RadioStartRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if payload.seed_track_id <= 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "seed_track_id must be a positive library id" })),
        ));
    }

    let blend = payload.blend.unwrap_or_default();
    let limit = payload.limit.unwrap_or(60).clamp(8, 200);

    let (db, lastfm) = {
        let g = state.read().await;
        let lastfm = crate::metadata::lastfm::LastFmClient::load(g.http_client.clone(), &g.db);
        (g.db.clone(), lastfm)
    };

    let radio_queue = crate::services::radio::orchestrate_song(
        &db,
        lastfm.as_ref(),
        payload.seed_track_id,
        blend,
        limit,
        &[],
    )
    .await
    .map_err(|e| {
        tracing::warn!("radio_start: orchestrate_song failed: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "radio orchestration failed" })),
        )
    })?;

    // Seed track leads the queue (matches the user's pick — like prepending it
    // before recommendations). orchestrate_song already excludes the seed from
    // its own results; the filter below is a defensive guard against duplicates.
    let seed_id = payload.seed_track_id;
    let (library_cands, pending_cands): (Vec<_>, Vec<_>) = radio_queue
        .tracks
        .into_iter()
        .filter(|c| c.track_id != seed_id)
        .partition(|c| c.is_in_library && c.track_id > 0);

    // Build queue atomically and collect pending row IDs for background tasks.
    let (first_item, pending_item_ids) = db
        .with_conn(move |conn| {
            let tx = conn.unchecked_transaction()?;

            tx.execute("DELETE FROM queue", [])?;

            let mut pos = 0i32;
            tx.execute(
                "INSERT INTO queue (track_id, position, source, reason) VALUES (?1, ?2, 'radio', NULL)",
                rusqlite::params![seed_id, pos],
            )?;
            pos += 1;
            for c in &library_cands {
                tx.execute(
                    "INSERT INTO queue (track_id, position, source, reason) VALUES (?1, ?2, 'radio', ?3)",
                    rusqlite::params![c.track_id, pos, c.reason],
                )?;
                pos += 1;
            }
            for c in &pending_cands {
                tx.execute(
                    "INSERT INTO queue (track_id, position, source, reason,
                                        pending_artist, pending_title, pending_at)
                     VALUES (NULL, ?1, 'radio_pending', ?2, ?3, ?4, datetime('now'))",
                    rusqlite::params![pos, c.reason, c.artist_name, c.title],
                )?;
                pos += 1;
            }

            tx.commit()?;

            let first: Option<(i64, Option<i64>)> = conn
                .query_row(
                    "SELECT id, track_id FROM queue ORDER BY position ASC LIMIT 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;

            let pending_ids: Vec<i64> = conn
                .prepare(
                    "SELECT id FROM queue WHERE track_id IS NULL AND pending_at IS NOT NULL",
                )?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            Ok((first, pending_ids))
        })
        .map_err(|e| {
            tracing::error!("radio_start: queue build failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to build queue" })),
            )
        })?;

    let first_playable = match first_item {
        Some((queue_item_id, Some(track_id))) => json!({
            "type": "library",
            "queue_item_id": queue_item_id,
            "track_id": track_id
        }),
        Some((queue_item_id, None)) => json!({
            "type": "pending",
            "queue_item_id": queue_item_id,
            "track_id": null
        }),
        None => {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "error": "queue is empty after insert" })),
            ));
        }
    };

    // Spawn bounded background resolvers for all pending rows.
    if !pending_item_ids.is_empty() {
        let tokens_opt: Option<crate::services::tidal::auth::TidalTokens> = {
            let s = state.read().await;
            if let Some(t) = s.tidal_tokens.clone() {
                Some(t)
            } else {
                drop(s);
                load_persisted_tidal_tokens(&state).await.ok().flatten()
            }
        };

        if let Some(tokens) = tokens_opt {
            let semaphore = Arc::new(tokio::sync::Semaphore::new(RESOLVER_POOL_SIZE));
            for item_id in pending_item_ids {
                let sem = semaphore.clone();
                let db_bg = db.clone();
                let tok = tokens.clone();
                tokio::spawn(async move {
                    let _permit = sem.acquire_owned().await.ok();
                    resolve_pending_row(db_bg, tok, item_id).await;
                });
            }
        } else {
            tracing::warn!("radio_start: Tidal tokens unavailable — pending rows will rely on lazy resolution");
        }
    }

    {
        let s = state.read().await;
        let _ = s.event_tx.send(AppEvent::QueueUpdated);
    }

    Ok(Json(json!({ "first_playable": first_playable })))
}

#[derive(Debug, Deserialize)]
struct RadioAlbumRequest {
    seed_album_id: i64,
    #[serde(default)]
    blend: Option<crate::services::radio::RadioBlend>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    exclude_track_ids: Option<Vec<i64>>,
}

async fn radio_album(
    State(state): State<SharedState>,
    Json(payload): Json<RadioAlbumRequest>,
) -> Result<Json<Value>, StatusCode> {
    let blend = payload.blend.unwrap_or_default();
    let limit = payload.limit.unwrap_or(60).clamp(8, 200);
    let exclude = payload.exclude_track_ids.unwrap_or_default();

    let (db, lastfm) = {
        let g = state.read().await;
        let lastfm = crate::metadata::lastfm::LastFmClient::load(g.http_client.clone(), &g.db);
        (g.db.clone(), lastfm)
    };

    let queue = crate::services::radio::orchestrate_album(
        &db,
        lastfm.as_ref(),
        payload.seed_album_id,
        blend,
        limit,
        &exclude,
    )
    .await
    .map_err(|e| {
        tracing::warn!("radio_album failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(serde_json::to_value(queue).unwrap_or(json!({}))))
}

#[derive(Debug, Deserialize)]
struct RadioArtistRequest {
    seed_artist_id: i64,
    #[serde(default)]
    blend: Option<crate::services::radio::RadioBlend>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    exclude_track_ids: Option<Vec<i64>>,
}

async fn radio_artist(
    State(state): State<SharedState>,
    Json(payload): Json<RadioArtistRequest>,
) -> Result<Json<Value>, StatusCode> {
    let blend = payload.blend.unwrap_or_default();
    let limit = payload.limit.unwrap_or(60).clamp(8, 200);
    let exclude = payload.exclude_track_ids.unwrap_or_default();

    let (db, lastfm) = {
        let g = state.read().await;
        let lastfm = crate::metadata::lastfm::LastFmClient::load(g.http_client.clone(), &g.db);
        (g.db.clone(), lastfm)
    };

    let queue = crate::services::radio::orchestrate_artist(
        &db,
        lastfm.as_ref(),
        payload.seed_artist_id,
        blend,
        limit,
        &exclude,
    )
    .await
    .map_err(|e| {
        tracing::warn!("radio_artist failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(serde_json::to_value(queue).unwrap_or(json!({}))))
}

async fn get_discovery_space_meta(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;

    let total_tracks: i64 = state.db.with_conn(|conn| {
        conn.query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get::<_, i64>(0))
            .map_err(Into::into)
    }).unwrap_or(0);

    let model_row: Option<(String, String, Option<String>, i64)> = state.db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT model_key, status, trained_at, dimension
             FROM embedding_models
             WHERE is_active = 1
             ORDER BY trained_at IS NULL, trained_at DESC
             LIMIT 1"
        )?;
        let mut rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }).ok().flatten();

    let (model_key, model_status, trained_at, vector_dim, embedding_count) = match &model_row {
        Some((key, status, trained, dim)) => {
            let count: i64 = state.db.with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM track_embeddings te
                     JOIN embedding_models em ON em.id = te.model_id
                     WHERE em.model_key = ?1",
                    rusqlite::params![key],
                    |row| row.get::<_, i64>(0),
                ).map_err(Into::into)
            }).unwrap_or(0);
            (Some(key.clone()), Some(status.clone()), trained.clone(), Some(*dim), count)
        }
        None => (None, None, None, None, 0),
    };

    let coverage = if total_tracks > 0 {
        embedding_count as f64 / total_tracks as f64
    } else {
        0.0
    };

    Ok(Json(json!({
        "model_key": model_key,
        "model_status": model_status,
        "trained_at": trained_at,
        "vector_dim": vector_dim,
        "neighbor_coverage": coverage,
        "track_count_with_embeddings": embedding_count,
        "track_count_total": total_tracks,
    })))
}

#[derive(Debug, Deserialize)]
struct DiscoveryArtistsQuery {
    limit: Option<i64>,
}

async fn get_discovery_artists(
    State(state): State<SharedState>,
    Query(query): Query<DiscoveryArtistsQuery>,
) -> Result<Json<Value>, StatusCode> {
    let limit = query.limit.unwrap_or(50).max(1).min(200);

    let artists = state.read().await.db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT a.id, a.name, COUNT(th.track_id) as listen_count
             FROM artists a
             LEFT JOIN tracks t ON t.artist_id = a.id
             LEFT JOIN track_history th ON th.track_id = t.id
             GROUP BY a.id, a.name
             ORDER BY listen_count DESC
             LIMIT ?"
        )?;
        let rows = stmt.query_map([limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let mut result = Vec::new();
        for r in rows {
            result.push(r?);
        }
        Ok(result)
    }).unwrap_or_default();

    let max_count = artists.iter().map(|(_, _, c)| *c).max().unwrap_or(1) as f64;
    let artist_count = artists.len();

    let artist_nodes: Vec<Value> = artists
        .into_iter()
        .enumerate()
        .map(|(i, (id, name, count))| {
            let angle = (i as f64 / artist_count.max(1) as f64) * std::f64::consts::PI * 2.0;
            let radius = 80.0 + (i as f64 * 43.0).sin() * 120.0;
            let affinity = if max_count > 0.0 { count as f64 / max_count } else { 0.0 };
            json!({
                "artist_id": id,
                "name": name,
                "top_genre": null,
                "affinity": affinity,
                "x": angle.cos() * radius,
                "y": angle.sin() * radius,
                "vx": 0.0,
                "vy": 0.0,
                "size": 8.0 + affinity * 32.0,
            })
        })
        .collect();

    Ok(Json(json!({
        "artists": artist_nodes,
    })))
}

fn normalize_discovery_mode(mode: Option<&str>) -> String {
    match mode.unwrap_or("mood").trim() {
        "reference" => "reference".to_string(),
        "dj" => "dj".to_string(),
        "word-cloud" => "word-cloud".to_string(),
        _ => "mood".to_string(),
    }
}

fn normalize_discovery_services(services: Option<Vec<String>>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    services
        .unwrap_or_else(|| vec!["tidal".to_string()])
        .into_iter()
        .map(|service| service.trim().to_ascii_lowercase())
        .filter(|service| !service.is_empty())
        .filter(|service| {
            matches!(
                service.as_str(),
                "tidal" | "ytmusic" | "soundcloud" | "bandcamp"
            )
        })
        .filter(|service| seen.insert(service.clone()))
        .collect()
}

fn normalize_external_provider(provider: &str) -> Option<&'static str> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "tidal" => Some("tidal"),
        "soundcloud" => Some("soundcloud"),
        "bandcamp" => Some("bandcamp"),
        "ytmusic" => Some("ytmusic"),
        _ => None,
    }
}

fn internal_discovery_error(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    error!(
        target: "noor.discovery.external",
        event = "internal_discovery_error",
        error = %error,
        "external discovery failed"
    );
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({
            "status": "discovery_internal_error",
            "message": "NOOR could not assemble the discovery context.",
        })),
    )
}

fn discovery_upstream_error(error: anyhow::Error) -> (StatusCode, Json<Value>) {
    error!(
        target: "noor.discovery.external",
        event = "upstream_discovery_error",
        error = %error,
        "upstream discovery provider failed"
    );
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({
            "status": "discovery_upstream_error",
            "message": "The external discovery provider failed to respond cleanly.",
            "details": error.to_string(),
        })),
    )
}

async fn tidal_discovery_provider(
    state: &SharedState,
) -> Result<TidalDiscoveryProvider, (StatusCode, Json<Value>)> {
    let state_guard = state.read().await;
    let tokens = state_guard.tidal_tokens.clone().ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "status": "not_connected",
                "message": "Connect TIDAL in Settings before searching for new music.",
            })),
        )
    })?;

    Ok(TidalDiscoveryProvider::new(
        tokens.access_token,
        tokens.user_id,
        tokens.country_code,
        state_guard.http_client.clone(),
    ))
}

/// When `automix_discover_new` is on, search TIDAL for genre/artist-matched tracks,
/// upsert any new ones into the local library, and append them to the queue so the
/// mix includes songs from outside the existing library.
async fn inject_discovery_tracks(state: &SharedState, current_track: &crate::db::models::Track) {
    let (tokens, http, db, event_tx) = {
        let guard = state.read().await;
        let Some(tokens) = guard.tidal_tokens.clone() else {
            return;
        };
        (
            tokens,
            guard.http_client.clone(),
            guard.db.clone(),
            guard.event_tx.clone(),
        )
    };

    // Build search queries from current track's artist and genres
    let current_track_clone = current_track.clone();
    let (artist_name, genre_hints) = db
        .with_conn(move |conn| {
            let genres = crate::playback::queue::get_track_genres(
                conn,
                std::slice::from_ref(&current_track_clone),
            )?;
            let genre_list = genres
                .get(&current_track_clone.id)
                .cloned()
                .unwrap_or_default();
            Ok((current_track_clone.artist_name.clone(), genre_list))
        })
        .unwrap_or((None, Vec::new()));

    let mut queries: Vec<String> = Vec::new();
    if let Some(ref artist) = artist_name {
        queries.push(artist.clone());
    }
    if let Some(genre) = genre_hints.first() {
        queries.push(genre.clone());
        if let Some(ref artist) = artist_name {
            queries.push(format!("{genre} {artist}"));
        }
    }
    if let Ok(mut learned_queries) =
        discovery_learning::inject_query_seeds_from_neighbors(&db, current_track.id, 6)
    {
        queries.append(&mut learned_queries);
    }
    queries.sort();
    queries.dedup();
    if queries.is_empty() {
        return;
    }

    let provider = TidalDiscoveryProvider::new(
        tokens.access_token,
        tokens.user_id,
        tokens.country_code,
        http,
    );
    let candidates = match provider.search_tracks(&queries, 6).await {
        Ok(c) => c,
        Err(_) => return,
    };

    let candidates: Vec<_> = candidates
        .into_iter()
        .filter(|c| c.is_playable && c.tidal_track_id.is_some())
        .take(4)
        .collect();

    if candidates.is_empty() {
        return;
    }

    let injected = db
        .with_conn(move |conn| {
            let mut count = 0usize;
            for candidate in &candidates {
                let tidal_id = candidate.tidal_track_id.unwrap();

                // Skip if this track is already in the library (normal automix handles it)
                let already_exists: bool = conn
                    .query_row(
                        "SELECT 1 FROM tracks WHERE tidal_id = ?1",
                        rusqlite::params![tidal_id],
                        |_| Ok(true),
                    )
                    .unwrap_or(false);
                if already_exists {
                    continue;
                }

                let artist_display = candidate
                    .artist_name
                    .as_deref()
                    .unwrap_or("Unknown Artist");

                // Find existing artist by tidal_id, then by name, or create a stub
                let maybe_artist_tidal_id = {
                    // DiscoveryCandidateTrack doesn't carry artist tidal_id, so we look
                    // it up from any track already in the library by the same name
                    conn.query_row(
                        "SELECT a.tidal_id FROM artists a
                         JOIN tracks t ON t.artist_id = a.id
                         WHERE LOWER(a.name) = LOWER(?1) AND a.tidal_id IS NOT NULL
                         LIMIT 1",
                        rusqlite::params![artist_display],
                        |row| row.get::<_, Option<i64>>(0),
                    )
                    .ok()
                    .flatten()
                };

                let artist_id: i64 = if let Some(tid) = maybe_artist_tidal_id {
                    // Upsert with known tidal_id
                    conn.execute(
                        "INSERT INTO artists (tidal_id, name) VALUES (?1, ?2)
                         ON CONFLICT(tidal_id) DO UPDATE SET name = excluded.name",
                        rusqlite::params![tid, artist_display],
                    )?;
                    conn.query_row(
                        "SELECT id FROM artists WHERE tidal_id = ?1",
                        rusqlite::params![tid],
                        |row| row.get(0),
                    )?
                } else {
                    // Try to find by name, else create a stub (no tidal_id)
                    match conn.query_row(
                        "SELECT id FROM artists WHERE LOWER(name) = LOWER(?1) LIMIT 1",
                        rusqlite::params![artist_display],
                        |row| row.get::<_, i64>(0),
                    ) {
                        Ok(id) => id,
                        Err(_) => {
                            conn.execute(
                                "INSERT INTO artists (name) VALUES (?1)",
                                rusqlite::params![artist_display],
                            )?;
                            conn.last_insert_rowid()
                        }
                    }
                };

                let duration_ms = candidate.duration_ms.unwrap_or(180_000);
                let quality = candidate.audio_quality.as_deref().unwrap_or("LOSSLESS");
                let fidelity: i32 = match quality {
                    "HI_RES_LOSSLESS" => 900,
                    "HI_RES" => 800,
                    "LOSSLESS" => 700,
                    "HIGH" => 400,
                    _ => 200,
                };

                if conn
                    .execute(
                        "INSERT INTO tracks (tidal_id, title, artist_id, duration_ms, best_quality, best_source, fidelity_score, is_favorite, source)
                         VALUES (?1, ?2, ?3, ?4, ?5, 'tidal', ?6, 0, 'tidal')
                         ON CONFLICT(tidal_id) DO NOTHING",
                        rusqlite::params![
                            tidal_id, candidate.title, artist_id,
                            duration_ms, quality, fidelity
                        ],
                    )
                    .is_err()
                {
                    continue;
                }

                let track_id: i64 = match conn.query_row(
                    "SELECT id FROM tracks WHERE tidal_id = ?1",
                    rusqlite::params![tidal_id],
                    |row| row.get(0),
                ) {
                    Ok(id) => id,
                    Err(_) => continue,
                };

                let max_pos: i32 = conn
                    .query_row(
                        "SELECT COALESCE(MAX(position), 0) FROM queue",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap_or(0);

                if conn
                    .execute(
                        "INSERT INTO queue (track_id, position, source) VALUES (?1, ?2, 'automix-new')",
                        rusqlite::params![track_id, max_pos + 1],
                    )
                    .is_ok()
                {
                    count += 1;
                }
            }
            Ok(count)
        })
        .unwrap_or(0);

    if injected > 0 {
        let _ = event_tx.send(AppEvent::QueueUpdated);
    }
}

async fn load_external_discovery_context(
    state: &SharedState,
) -> anyhow::Result<external_discovery_engine::ExternalDiscoveryContext> {
    let state_guard = state.read().await;
    state_guard.db.with_conn(|conn| {
        Ok(external_discovery_engine::ExternalDiscoveryContext {
            overview: queries::get_analytics_overview(conn)?,
            behavior: queries::get_behavior_metrics(conn)?,
            recent_listens: queries::get_recent_listens(conn, 12)?,
            top_artists: queries::get_top_artists_by_history(conn, 6)?,
            top_genres: queries::get_top_genres_by_history(conn, 6)?,
        })
    })
}

async fn existing_candidate_tidal_ids(
    state: &SharedState,
    candidates: &[crate::services::discovery::DiscoveryCandidateTrack],
) -> anyhow::Result<std::collections::HashSet<i64>> {
    let tidal_ids = candidates
        .iter()
        .filter_map(|candidate| candidate.tidal_track_id)
        .collect::<Vec<_>>();
    let state_guard = state.read().await;
    state_guard
        .db
        .with_conn(|conn| queries::get_existing_tidal_track_ids(conn, &tidal_ids))
}

fn discovery_provider_capabilities() -> Vec<crate::db::models::DiscoveryProviderCapability> {
    vec![
        crate::db::models::DiscoveryProviderCapability {
            provider: "tidal".to_string(),
            can_save: true,
            can_play_inline: true,
            can_fetch_connections: true,
            can_map_genres: true,
        },
        crate::db::models::DiscoveryProviderCapability {
            provider: "soundcloud".to_string(),
            can_save: false,
            can_play_inline: false,
            can_fetch_connections: false,
            can_map_genres: false,
        },
        crate::db::models::DiscoveryProviderCapability {
            provider: "bandcamp".to_string(),
            can_save: false,
            can_play_inline: false,
            can_fetch_connections: false,
            can_map_genres: false,
        },
        crate::db::models::DiscoveryProviderCapability {
            provider: "ytmusic".to_string(),
            can_save: false,
            can_play_inline: false,
            can_fetch_connections: false,
            can_map_genres: false,
        },
    ]
}

async fn augment_connection_queries_with_lastfm(
    state: &SharedState,
    seed: &DiscoveryCandidateSeed,
    base_queries: Vec<String>,
) -> Vec<String> {
    let (http_client, db) = {
        let state_guard = state.read().await;
        (state_guard.http_client.clone(), state_guard.db.clone())
    };
    let Some(lastfm) = LastFmClient::load(http_client, &db) else {
        return base_queries;
    };

    match lastfm.connection_queries(seed).await {
        Ok(extra_queries) => merge_discovery_queries(base_queries, extra_queries, 12),
        Err(error) => {
            warn!(
                target: "noor.discovery.lastfm",
                event = "connection_query_augmentation_failed",
                error = %error,
                seed_title = %seed.title,
                "failed to augment connection queries with Last.fm"
            );
            base_queries
        }
    }
}

async fn augment_search_queries_with_lastfm(
    state: &SharedState,
    request: &external_discovery_engine::ExternalDiscoveryRequest,
    context: &external_discovery_engine::ExternalDiscoveryContext,
    base_queries: Vec<String>,
) -> Vec<String> {
    let (http_client, db) = {
        let state_guard = state.read().await;
        (state_guard.http_client.clone(), state_guard.db.clone())
    };
    let Some(lastfm) = LastFmClient::load(http_client, &db) else {
        return base_queries;
    };

    let prompt_genres = external_discovery_engine::inferred_prompt_genres(&request.prompt);
    let seed_artists = context
        .top_artists
        .iter()
        .take(2)
        .map(|artist| artist.artist_name.clone())
        .collect::<Vec<_>>();

    match lastfm
        .search_queries(&prompt_genres, &seed_artists, &request.mode)
        .await
    {
        Ok(extra_queries) => merge_discovery_queries(base_queries, extra_queries, 12),
        Err(error) => {
            warn!(
                target: "noor.discovery.lastfm",
                event = "search_query_augmentation_failed",
                error = %error,
                prompt = %request.prompt,
                "failed to augment discovery search queries with Last.fm"
            );
            base_queries
        }
    }
}

async fn enrich_candidates_with_metadata(
    state: &SharedState,
    mut candidates: Vec<crate::services::discovery::DiscoveryCandidateTrack>,
) -> Vec<crate::services::discovery::DiscoveryCandidateTrack> {
    let (http_client, db) = {
        let state_guard = state.read().await;
        (state_guard.http_client.clone(), state_guard.db.clone())
    };
    let lastfm = LastFmClient::load(http_client.clone(), &db);
    let discogs = DiscogsClient::new(http_client);

    for candidate in candidates.iter_mut().take(16) {
        if let (Some(lastfm), Some(artist_name)) =
            (lastfm.as_ref(), candidate.artist_name.as_deref())
        {
            match lastfm.track_signals(artist_name, &candidate.title).await {
                Ok(signals) => {
                    candidate.lastfm_tags = signals.tags;
                }
                Err(error) => {
                    warn!(
                        target: "noor.discovery.lastfm",
                        event = "track_signal_enrichment_failed",
                        error = %error,
                        artist = %artist_name,
                        title = %candidate.title,
                        "failed to enrich discovery candidate with Last.fm tags"
                    );
                }
            }
        }

        match discogs
            .enrich_track(
                candidate.artist_name.as_deref(),
                &candidate.title,
                candidate.album_title.as_deref(),
            )
            .await
        {
            Ok(Some(enrichment)) => {
                candidate.discogs_genres = enrichment.genres;
                candidate.discogs_styles = enrichment.styles;
                candidate.discogs_label = enrichment.label;
                candidate.discogs_year = enrichment.year;
                candidate.discogs_confidence = Some(enrichment.confidence);
            }
            Ok(None) => {}
            Err(error) => {
                warn!(
                    target: "noor.discovery.discogs",
                    event = "track_enrichment_failed",
                    error = %error,
                    title = %candidate.title,
                    "failed to enrich discovery candidate with Discogs metadata"
                );
            }
        }
    }

    candidates
}

fn merge_discovery_queries(
    base_queries: Vec<String>,
    extra_queries: Vec<String>,
    limit: usize,
) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    base_queries
        .into_iter()
        .chain(extra_queries)
        .map(|query| query.trim().to_string())
        .filter(|query| !query.is_empty())
        .filter(|query| seen.insert(query.to_ascii_lowercase()))
        .take(limit)
        .collect()
}

fn parse_provider_track_id(provider_track_id: &str) -> Result<i64, (StatusCode, Json<Value>)> {
    provider_track_id.parse::<i64>().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "invalid_provider_track_id",
                "message": "Provider track id was not valid for playback.",
            })),
        )
    })
}

fn discovery_result_to_track(
    payload: &DiscoveryExternalResultRequest,
) -> Result<crate::db::models::Track, (StatusCode, Json<Value>)> {
    let tidal_track_id = parse_provider_track_id(&payload.provider_track_id)?;
    let ephemeral_id = -tidal_track_id;
    Ok(crate::db::models::Track {
        id: ephemeral_id,
        title: payload.title.clone(),
        artist_id: 0,
        artist_name: payload.artist_name.clone(),
        album_id: None,
        album_title: payload.album_title.clone(),
        disc_number: None,
        track_number: None,
        duration_ms: payload.duration_ms,
        isrc: None,
        tidal_id: Some(tidal_track_id),
        ytmusic_id: None,
        soundcloud_id: None,
        best_quality: payload.audio_quality.clone(),
        best_source: Some("tidal".to_string()),
        fidelity_score: 100,
        is_favorite: false,
        play_count: 0,
        last_played_at: None,
        date_added: None,
        source: "tidal-discovery".to_string(),
        artwork_url: payload.artwork_url.clone(),
    })
}

fn discovery_request_to_trail_item(
    payload: &DiscoveryExternalResultRequest,
) -> crate::db::models::DiscoveryConnectionTrailItem {
    crate::db::models::DiscoveryConnectionTrailItem {
        provider: payload.provider.clone(),
        provider_track_id: payload.provider_track_id.clone(),
        title: payload.title.clone(),
        artist_name: payload.artist_name.clone(),
        album_title: payload.album_title.clone(),
        artwork_url: payload.artwork_url.clone(),
        normalized_genres: payload.normalized_genres.clone().unwrap_or_default(),
        connection_reason: if payload
            .normalized_genres
            .as_ref()
            .map(|genres| !genres.is_empty())
            .unwrap_or(false)
        {
            format!(
                "genre cues like {}",
                payload
                    .normalized_genres
                    .clone()
                    .unwrap_or_default()
                    .join(", ")
            )
        } else if let Some(artist) = payload.artist_name.as_deref() {
            format!("adjacent energy around {artist}")
        } else {
            "adjacent energy".to_string()
        },
    }
}

async fn set_external_playback_track(state: &SharedState, track: Option<crate::db::models::Track>) {
    let mut state_guard = state.write().await;
    state_guard.external_playback_track = track;
}

async fn overlay_snapshot_with_external_track(
    state: &SharedState,
    snapshot: player::PlaybackSnapshot,
) -> player::PlaybackSnapshot {
    overlay_snapshot_with_external_track_and_position(state, snapshot, None).await
}

async fn overlay_snapshot_with_external_track_and_position(
    state: &SharedState,
    mut snapshot: player::PlaybackSnapshot,
    live_position_ms: Option<i64>,
) -> player::PlaybackSnapshot {
    let state_guard = state.read().await;
    if let Some(pos) = live_position_ms {
        snapshot.state.position_ms = pos;
    }
    // Ephemeral Tidal track takes priority: it represents a track playing right
    // now that has no DB record.
    if let Some(ephemeral) = &state_guard.ephemeral_tidal_track {
        snapshot.state.current_track = Some(ephemeral.clone());
        snapshot.state.is_playing = true;
    } else if snapshot.state.current_track.is_none() {
        if let Some(track) = state_guard.external_playback_track.as_ref() {
            snapshot.state.current_track = Some(track.clone());
        }
    }
    snapshot
}

async fn batch_add_to_playlist(
    State(state): State<SharedState>,
    Json(payload): Json<BatchPlaylistRequest>,
) -> Result<Json<Value>, StatusCode> {
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

async fn batch_delete_items(
    State(state): State<SharedState>,
    Json(payload): Json<BatchDeleteRequest>,
) -> Result<Json<Value>, StatusCode> {
    let track_ids = dedupe_positive_ids(payload.track_ids.as_deref().unwrap_or(&[]));
    let album_ids = dedupe_positive_ids(payload.album_ids.as_deref().unwrap_or(&[]));
    if track_ids.is_empty() && album_ids.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let (http, tokens) = {
        let state = state.read().await;
        let tokens = state.tidal_tokens.clone().ok_or(StatusCode::UNAUTHORIZED)?;
        (state.http_client.clone(), tokens)
    };

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

    let removed_tracks = tidal_mutations::remove_favorite_tracks(
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
    })?;

    let removed_albums = tidal_mutations::remove_favorite_albums(
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
    })?;

    // Also delete from local DB so removed items disappear immediately.
    let db = {
        let s = state.read().await;
        s.db.clone()
    };
    if let Err(e) = db.with_conn(|conn| {
        for &(local_id, _) in &track_pairs {
            conn.execute(
                "DELETE FROM tracks WHERE id = ?1",
                rusqlite::params![local_id],
            )?;
        }
        for &(local_id, _) in &album_pairs {
            conn.execute(
                "DELETE FROM albums WHERE id = ?1",
                rusqlite::params![local_id],
            )?;
        }
        Ok(())
    }) {
        warn!("Batch delete: local DB cleanup failed: {e}");
    }

    {
        let state = state.read().await;
        let _ = state.event_tx.send(AppEvent::LibrarySynced);
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

async fn set_track_favorite(
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

    let (tidal_tokens, http_client) = {
        let s = state.read().await;
        (s.tidal_tokens.clone(), s.http_client.clone())
    };
    // Fall back to persisted tokens when the in-memory slot is empty (e.g. just after startup).
    let tidal_tokens = if tidal_tokens.is_none() {
        load_persisted_tidal_tokens(&state).await.ok().flatten()
    } else {
        tidal_tokens
    };

    // Update local DB immediately — Tidal sync happens in the background.
    // When liking for the first time, bump date_added so the track sorts to top of the library.
    {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE tracks SET is_favorite = ?1, \
                     date_added = CASE WHEN ?1 = 1 AND is_favorite = 0 THEN datetime('now') ELSE date_added END \
                     WHERE id = ?2",
                    rusqlite::params![if payload.favorite { 1 } else { 0 }, payload.track_id],
                )?;
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

    // Fire Tidal sync in the background so the response returns immediately.
    if let (Some(tidal_id), Some(tokens)) = (tidal_id, tidal_tokens) {
        if state_changed {
            let favorite = payload.favorite;
            let state_for_sync = state.clone();
            tokio::spawn(async move {
                let result = if favorite {
                    tidal_mutations::add_favorite_track(
                        &http_client,
                        &tokens.access_token,
                        &tokens.user_id,
                        tidal_id,
                        &tokens.country_code,
                    )
                    .await
                } else {
                    tidal_mutations::remove_favorite_track(
                        &http_client,
                        &tokens.access_token,
                        &tokens.user_id,
                        tidal_id,
                        &tokens.country_code,
                    )
                    .await
                };
                if let Err(error) = result {
                    if error_looks_like_auth(&error) {
                        // Token expired — refresh and retry once, matching the pattern
                        // used by search/stream/playlist paths in this file.
                        match recover_tidal_session(&state_for_sync, &http_client, &tokens).await {
                            Ok(refreshed) => {
                                let retry = if favorite {
                                    tidal_mutations::add_favorite_track(
                                        &http_client,
                                        &refreshed.access_token,
                                        &refreshed.user_id,
                                        tidal_id,
                                        &refreshed.country_code,
                                    )
                                    .await
                                } else {
                                    tidal_mutations::remove_favorite_track(
                                        &http_client,
                                        &refreshed.access_token,
                                        &refreshed.user_id,
                                        tidal_id,
                                        &refreshed.country_code,
                                    )
                                    .await
                                };
                                if let Err(e2) = retry {
                                    error!(
                                        "Failed to sync {} favorite for tidal track {tidal_id} after session refresh: {e2}",
                                        if favorite { "set" } else { "clear" },
                                    );
                                }
                            }
                            Err(re) => {
                                error!(
                                    "Session refresh failed while syncing {} favorite for tidal track {tidal_id}: {re}",
                                    if favorite { "set" } else { "clear" },
                                );
                            }
                        }
                    } else {
                        warn!(
                            "Failed to background-sync {} favorite for tidal track {tidal_id}: {error}",
                            if favorite { "set" } else { "clear" },
                        );
                    }
                }
            });
        }
    } else if tidal_id.is_some() && state_changed {
        warn!(
            "Track {} has tidal_id but no tokens available for sync",
            payload.track_id
        );
    }

    Ok(Json(json!({
        "track_id": payload.track_id,
        "tidal_id": tidal_id,
        "favorite": payload.favorite,
        "updated": state_changed
    })))
}

async fn batch_set_genre(
    State(state): State<SharedState>,
    Json(payload): Json<BatchGenreRequest>,
) -> Result<Json<Value>, StatusCode> {
    let track_ids = dedupe_positive_ids(&payload.track_ids);
    if track_ids.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let affected = {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| {
                queries::assign_genre_to_tracks(conn, payload.genre_id, &track_ids, "manual")
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
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

// ── MusicBrainz enrichment ─────────────────────────────────────────────────

async fn start_musicbrainz_enrichment(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let (http_client, event_tx) = {
        let g = state.read().await;
        (g.http_client.clone(), g.event_tx.clone())
    };

    let total: usize = {
        let g = state.read().await;
        g.db.with_conn(crate::services::musicbrainz::count_unenriched_tracks)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    if total == 0 {
        return Ok(Json(
            json!({ "status": "already_complete", "remaining": 0 }),
        ));
    }

    tokio::spawn(async move {
        let progress_tx = event_tx.clone();
        let result = crate::services::musicbrainz::run_enrichment(
            state,
            http_client,
            move |progress| {
                let _ = progress_tx.send(AppEvent::SyncProgress {
                    service: "musicbrainz".to_string(),
                    progress: progress.processed as f32 / progress.total.max(1) as f32,
                });
            },
            100,
        )
        .await;
        match result {
            Ok(_) => {
                let _ = event_tx.send(AppEvent::MusicBrainzEnriched);
                let _ = event_tx.send(AppEvent::LibrarySynced);
            }
            Err(err) => {
                warn!("MusicBrainz enrichment error: {err:?}");
            }
        }
    });

    Ok(Json(json!({ "status": "started", "remaining": total })))
}

async fn get_musicbrainz_status(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    let (total, checked, enriched) = state
        .db
        .with_conn(|conn| {
            let total: i64 = conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))?;
            let checked: i64 =
                conn.query_row("SELECT COUNT(*) FROM musicbrainz_checked", [], |r| r.get(0))?;
            let enriched: i64 = conn.query_row(
                "SELECT COUNT(DISTINCT track_id) FROM track_genres WHERE source = 'musicbrainz'",
                [],
                |r| r.get(0),
            )?;
            Ok((total, checked, enriched))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({
        "total_tracks": total,
        "checked_tracks": checked,
        "enriched_tracks": enriched,
        "remaining": (total - checked).max(0),
        "complete": checked >= total
    })))
}

async fn get_musicbrainz_portable_snapshot(
    State(_state): State<SharedState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let snapshot =
        crate::services::musicbrainz::read_portable_snapshot_status().map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "message": "NOOR couldn't read the portable MusicBrainz snapshot status.",
                    "details": error.to_string(),
                })),
            )
        })?;

    Ok(Json(json!({
        "exists": snapshot.exists,
        "path": snapshot.path,
        "generated_at": snapshot.generated_at,
        "checked_rows": snapshot.checked_rows,
        "genre_rows": snapshot.genre_rows,
    })))
}

async fn export_musicbrainz_portable_snapshot(
    State(state): State<SharedState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let snapshot = {
        let state = state.read().await;
        state
            .db
            .with_conn(crate::services::musicbrainz::export_portable_snapshot)
            .map_err(|error| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "message": "NOOR couldn't write the portable MusicBrainz snapshot.",
                        "details": error.to_string(),
                    })),
                )
            })?
            .status
    };

    Ok(Json(json!({
        "status": "exported",
        "snapshot": {
            "exists": snapshot.exists,
            "path": snapshot.path,
            "generated_at": snapshot.generated_at,
            "checked_rows": snapshot.checked_rows,
            "genre_rows": snapshot.genre_rows,
        }
    })))
}

async fn import_musicbrainz_portable_snapshot(
    State(state): State<SharedState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let imported = {
        let state = state.read().await;
        state
            .db
            .with_conn(crate::services::musicbrainz::import_portable_snapshot)
            .map_err(|error| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "message": "NOOR couldn't import the portable MusicBrainz snapshot.",
                        "details": error.to_string(),
                    })),
                )
            })?
    };

    {
        let state = state.read().await;
        let _ = state.event_tx.send(AppEvent::MusicBrainzEnriched);
        let _ = state.event_tx.send(AppEvent::LibrarySynced);
    }

    Ok(Json(json!({
        "status": "imported",
        "checked_inserted": imported.checked_inserted,
        "checked_skipped": imported.checked_skipped,
        "genre_inserted": imported.genre_inserted,
        "track_skipped": imported.track_skipped,
        "genre_skipped": imported.genre_skipped,
        "snapshot": {
            "exists": imported.status.exists,
            "path": imported.status.path,
            "generated_at": imported.status.generated_at,
            "checked_rows": imported.status.checked_rows,
            "genre_rows": imported.status.genre_rows,
        }
    })))
}

// ── Duplicate detection ───────────────────────────────────────────────────────

/// Scan the library for duplicates. Runs synchronously (usually <5s for 32k tracks).
async fn scan_duplicates(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let stats = {
        let s = state.read().await;
        s.db.with_conn(dup::scan)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    Ok(Json(json!({
        "groups_found": stats.groups_found,
        "tracks_affected": stats.tracks_affected,
        "isrc_matches": stats.isrc_matches,
        "title_matches": stats.title_matches,
    })))
}

/// List pending duplicate groups with full track data (paginated).
async fn get_duplicates(
    State(state): State<SharedState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Value>, StatusCode> {
    let limit = params.limit.unwrap_or(50);
    let offset = params.offset.unwrap_or(0);

    let s = state.read().await;
    s.db.with_conn(|conn| {
        let total = dup::count_pending_groups(conn)?;
        let groups = dup::load_groups(conn, limit, offset)?;
        Ok(Json(json!({ "groups": groups, "total": total })))
    })
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Keep `preferred_track_id`, delete the rest from DB, return TIDAL IDs to unfavorite.
async fn resolve_duplicate_group(
    State(state): State<SharedState>,
    Path(group_id): Path<i64>,
    Json(payload): Json<ResolveGroupRequest>,
) -> Result<Json<Value>, StatusCode> {
    // Get TIDAL tokens for unfavorite calls.
    let (tokens, http) = {
        let s = state.read().await;
        let tokens = s.tidal_tokens.clone();
        (tokens, s.http_client.clone())
    };

    let result = {
        let s = state.read().await;
        s.db.with_conn(|conn| dup::resolve_group(conn, group_id, payload.preferred_track_id))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    // Best-effort unfavorite on TIDAL with session refresh retry.
    if let Some(t) = tokens.clone() {
        for tidal_id in &result.tidal_ids_to_unfavorite {
            if let Err(e) = tidal_mutations::remove_favorite_track(
                &http,
                &t.access_token,
                &t.user_id,
                *tidal_id,
                &t.country_code,
            )
            .await
            {
                // If it looks like a session expiry, try to refresh and retry once.
                if e.to_string().contains("401")
                    || e.to_string().to_lowercase().contains("unauthorized")
                {
                    if let Ok(refreshed) = recover_tidal_session(&state, &http, &t).await {
                        if let Err(e2) = tidal_mutations::remove_favorite_track(
                            &http,
                            &refreshed.access_token,
                            &refreshed.user_id,
                            *tidal_id,
                            &refreshed.country_code,
                        )
                        .await
                        {
                            error!(
                                "Failed to unfavorite TIDAL track {tidal_id} after session refresh: {e2}"
                            );
                        }
                        continue;
                    }
                }
                warn!("Failed to unfavorite TIDAL track {tidal_id}: {e}");
            }
        }
    }

    Ok(Json(json!({
        "removed": result.removed_track_ids,
        "unfavorited_tidal": result.tidal_ids_to_unfavorite,
    })))
}

/// Dismiss a duplicate group without deleting anything.
async fn dismiss_duplicate_group(
    State(state): State<SharedState>,
    Path(group_id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    s.db.with_conn(|conn| dup::dismiss_group(conn, group_id))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "status": "dismissed" })))
}

async fn search(
    State(state): State<SharedState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    let limit = params.limit.unwrap_or(20);

    state
        .db
        .with_conn(|conn| {
            let results = queries::search(conn, &params.q, limit)?;
            Ok(Json(json!(results)))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[derive(Debug, Deserialize)]
struct AudioSearchRequest {
    free_text: Option<String>,
    bpm_min: Option<f64>,
    bpm_max: Option<f64>,
    energy_min: Option<f64>,
    energy_max: Option<f64>,
    danceability_min: Option<f64>,
    danceability_max: Option<f64>,
    key_signature: Option<String>,
    camelot_key: Option<String>,
    year_min: Option<i64>,
    year_max: Option<i64>,
    genre_ids: Option<Vec<i64>>,
    track_type: Option<String>,
    is_instrumental: Option<bool>,
    limit: Option<usize>,
}

async fn search_audio(
    State(state): State<SharedState>,
    Json(body): Json<AudioSearchRequest>,
) -> Result<Json<Value>, StatusCode> {
    let filters = queries::AudioFilters {
        bpm_min: body.bpm_min,
        bpm_max: body.bpm_max,
        energy_min: body.energy_min,
        energy_max: body.energy_max,
        danceability_min: body.danceability_min,
        danceability_max: body.danceability_max,
        key_signature: body.key_signature,
        camelot_key: body.camelot_key,
        year_min: body.year_min,
        year_max: body.year_max,
        genre_ids: body.genre_ids.unwrap_or_default(),
        track_type: body.track_type,
        is_instrumental: body.is_instrumental,
    };
    let free_text = body.free_text.unwrap_or_default();
    let limit = body.limit.unwrap_or(50);

    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let tracks = queries::search_with_audio_filters(conn, &free_text, &filters, limit)?;
            Ok(Json(json!({ "tracks": tracks })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[derive(Deserialize)]
struct VibeParams {
    track_id: i64,
    limit: Option<usize>,
}

async fn search_vibe(
    State(state): State<SharedState>,
    Query(params): Query<VibeParams>,
) -> Result<Json<Value>, StatusCode> {
    let limit = params.limit.unwrap_or(6);
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let results = queries::get_same_vibe_tracks(conn, params.track_id, limit as i64)?;
            Ok(Json(json!({ "tracks": results })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[derive(Deserialize)]
struct UnderratedParams {
    artist_id: i64,
    limit: Option<usize>,
}

async fn search_underrated(
    State(state): State<SharedState>,
    Query(params): Query<UnderratedParams>,
) -> Result<Json<Value>, StatusCode> {
    let limit = params.limit.unwrap_or(5);
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let results = queries::get_underrated_tracks(conn, params.artist_id, limit as i64)?;
            Ok(Json(json!({ "tracks": results })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_playback_state(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let (live_position_ms, ephemeral_playing, audio_active) = {
        let state_guard = state.read().await;
        let live_pos = state_guard
            .playback_runtime
            .as_ref()
            .zip(state_guard.playback_runtime_info.as_ref())
            .map(|(rt, info)| rt.handle.get_position_ms(info.sample_rate, info.channels));
        let ephemeral = state_guard.ephemeral_tidal_track.is_some();
        let active = state_guard.audio_active.load(std::sync::atomic::Ordering::Relaxed);
        (live_pos, ephemeral, active)
    };

    let snapshot = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(player::load_snapshot)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    let mut snapshot =
        overlay_snapshot_with_external_track_and_position(&state, snapshot, live_position_ms).await;

    // Correct a stale is_playing flag before sending to the frontend:
    // - no runtime at all (server restarted, runtime crashed), OR
    // - runtime exists but CPAL buffer hasn't started draining yet (buffering phase).
    // Ephemeral TIDAL tracks bypass this check — they set is_playing themselves.
    if (!audio_active || live_position_ms.is_none()) && !ephemeral_playing {
        snapshot.state.is_playing = false;
    }

    Ok(Json(json!({
        "state": snapshot.state,
        "queue": snapshot.queue
    })))
}

async fn get_playback_runtime(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    let runtime = state.playback_runtime_info.as_ref().map(|info| {
        json!({
            "device_name": info.device_name,
            "sample_rate": info.sample_rate,
            "channels": info.channels,
            "active_track_id": info.active_track_id,
            "last_error": info.last_error,
        })
    });
    let stream = state.current_stream_display.as_ref().map(|d| {
        json!({
            "audio_quality": d.audio_quality,
            "sample_rate": d.sample_rate,
            "bit_depth": d.bit_depth,
        })
    });

    Ok(Json(json!({
        "available": runtime.is_some(),
        "runtime": runtime,
        "stream": stream,
    })))
}

async fn get_playback_queue(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let queue = queue::load_queue(conn)?;
            Ok(Json(json!({ "queue": queue })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn play_track(
    State(state): State<SharedState>,
    Json(payload): Json<PlaybackTrackRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let previous_track_id = current_playback_track_id(&state).await;
    let track = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(|conn| queue::get_track_by_id(conn, payload.track_id))
            .map_err(|error| {
                tracing::error!(
                    target: "noor.playback.tidal",
                    event = "playback_track_lookup_failed",
                    track_id = payload.track_id,
                    error = %error,
                    "failed to load track before playback"
                );
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "status": "track_lookup_failed",
                        "message": "Failed to load track before playback.",
                        "track_id": payload.track_id,
                    })),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    Json(json!({
                        "status": "track_not_found",
                        "message": "Track not found.",
                        "track_id": payload.track_id,
                    })),
                )
            })?
    };

    tracing::info!(
        target: "noor.playback.tidal",
        event = "playback_start_requested",
        track_id = track.id,
        source = %player::playback_source_kind(&track),
        "playback start requested"
    );

    let user_quality = current_user_audio_quality(&state).await;
    let stream_request = player::build_tidal_stream_request(&track, user_quality.clone()).ok_or_else(|| {
        (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({
                "status": "local_playback_not_supported",
                "message": "Local-library playback is not wired into the host audio runtime yet.",
                "track_id": track.id,
            })),
        )
    })?;
    let stream_info = resolve_tidal_playback_stream(&state, &track, &stream_request)
        .await
        .map_err(|error| {
            tidal_playback_error_response(
                track.id,
                error,
                "TIDAL stream could not be resolved before playback.",
            )
        })?;
    tracing::info!(
        target: "noor.playback.tidal",
        event = "playback_stream_ready",
        track_id = track.id,
        "TIDAL stream resolved before playback start"
    );

    let runtime_handle = ensure_playback_runtime_for_track(&state, &track).await?;
    let crossfade_ms = current_crossfade_ms(&state).await;
    let job = player::build_playback_preparation(
        &track,
        Some(&stream_info),
        crossfade_ms,
        user_quality,
    );
    runtime_handle.play(job).map_err(|error| {
        let message = format!("Failed to start host audio playback: {error}");
        report_playback_failure(&state, &message);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "status": "playback_runtime_failed",
                "message": message,
                "track_id": track.id,
            })),
        )
    })?;
    // Fire-and-forget play event — session health + artist attribution
    if let Some(tidal_id) = track.tidal_id {
        let http = {
            let g = state.read().await;
            g.http_client.clone()
        };
        let token = {
            let g = state.read().await;
            g.tidal_tokens.as_ref().map(|t| t.access_token.clone())
        };
        if let Some(token) = token {
            let quality = stream_info.audio_quality.clone();
            let duration_ms = track.duration_ms.unwrap_or(0);
            tokio::spawn(async move {
                if let Err(e) = crate::services::tidal::play_reporter::report_play(
                    &http, &token, tidal_id, &quality, duration_ms,
                )
                .await
                {
                    tracing::warn!("play report failed: {e}");
                }
            });
        }
    }
    {
        let mut state_guard = state.write().await;
        state_guard.current_stream_display = Some(crate::StreamDisplayInfo {
            audio_quality: stream_info.audio_quality.clone(),
            sample_rate: stream_info.sample_rate,
            bit_depth: stream_info.bit_depth,
        });
        state_guard.pending_stream_display = None;
    }

    let snapshot = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(|conn| player::play_track_now(conn, payload.track_id))
            .map_err(|error| {
                tracing::error!(
                    target: "noor.playback.tidal",
                    event = "playback_start_failed",
                    track_id = payload.track_id,
                    error = %error,
                    "failed to start playback"
                );
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "status": "playback_start_failed",
                        "message": "Failed to start playback.",
                        "track_id": payload.track_id,
                    })),
                )
            })?
    };
    set_external_playback_track(&state, None).await;
    {
        let mut state_guard = state.write().await;
        state_guard.ephemeral_tidal_track = None;
    }
    record_transition_if_changed(&state, previous_track_id, &snapshot, "user", false).await;

    sync_session_after_snapshot(
        &state,
        &snapshot,
        Some(player::ListenSessionEndReason::Replaced),
    )
    .await;

    // If automix is enabled, fill the queue in the background now that
    // the new current track is committed to DB. Doing this here (rather than
    // at automix-enable time) ensures the fill uses the correct track context
    // and doesn't race with this play_track DB operation.
    if snapshot.state.automix_enabled {
        let bg_db = {
            let g = state.read().await;
            g.db.clone()
        };
        let bg_tx = {
            let g = state.read().await;
            g.event_tx.clone()
        };
        tokio::spawn(async move {
            let result = bg_db.with_conn(|conn| {
                player::ensure_automix_queue_depth(conn, player::AUTOMIX_MIN_UPCOMING)
            });
            if result.is_ok() {
                let _ = bg_tx.send(AppEvent::QueueUpdated);
            }
        });
    }

    let state_guard = state.read().await;
    let _ = state_guard.event_tx.send(AppEvent::TrackChanged {
        track_id: payload.track_id,
    });
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);

    Ok(Json(json!({
        "state": snapshot.state,
        "queue": snapshot.queue
    })))
}

enum TidalPlaybackError {
    NotConnected,
    SessionRefreshFailed(String),
    StreamResolve(tidal_stream::StreamResolveError),
}

async fn resolve_tidal_playback_stream(
    state: &SharedState,
    track: &crate::db::models::Track,
    request: &tidal_stream::StreamRequest,
) -> Result<tidal_stream::StreamInfo, TidalPlaybackError> {
    let tokens = {
        let state_guard = state.read().await;
        state_guard.tidal_tokens.clone()
    }
    .ok_or(TidalPlaybackError::NotConnected)?;

    let http = {
        let state_guard = state.read().await;
        state_guard.http_client.clone()
    };

    match tidal_stream::resolve_stream(&http, &tokens.access_token, request).await {
        Ok(info) => Ok(info),
        Err(err) if err.is_session_expired() => {
            tracing::warn!(
                target: "noor.playback.tidal",
                event = "playback_stream_session_expired",
                track_id = track.id,
                error = %err,
                "TIDAL session expired while resolving playback stream"
            );

            let refreshed = match recover_tidal_session(state, &http, &tokens).await {
                Ok(tokens) => tokens,
                Err(recover_err) => {
                    // Do NOT clear the session here — a transient network error during
                    // token refresh should not log the user out permanently.
                    tracing::error!(
                        target: "noor.playback.tidal",
                        event = "playback_stream_refresh_failed",
                        track_id = track.id,
                        error = %recover_err,
                        original_error = %err,
                        "TIDAL session refresh failed while starting playback; keeping stored tokens"
                    );
                    return Err(TidalPlaybackError::SessionRefreshFailed(
                        recover_err.to_string(),
                    ));
                }
            };

            tracing::info!(
                target: "noor.playback.tidal",
                event = "playback_stream_session_recovered",
                track_id = track.id,
                "TIDAL session refreshed; retrying stream resolution"
            );

            match tidal_stream::resolve_stream(&http, &refreshed.access_token, request).await {
                Ok(info) => Ok(info),
                Err(retry_err) if retry_err.is_session_expired() => {
                    // Still expired after a successful refresh — TIDAL revoked the account.
                    // Only clear now since we know the refresh token itself is dead.
                    let _ = clear_tidal_session(state).await;
                    tracing::error!(
                        target: "noor.playback.tidal",
                        event = "playback_stream_retry_session_expired",
                        track_id = track.id,
                        error = %retry_err,
                        "TIDAL stream still rejected the refreshed session; session cleared"
                    );
                    Err(TidalPlaybackError::SessionRefreshFailed(
                        retry_err.to_string(),
                    ))
                }
                Err(retry_err) => Err(TidalPlaybackError::StreamResolve(retry_err)),
            }
        }
        Err(err) => Err(TidalPlaybackError::StreamResolve(err)),
    }
}

fn tidal_playback_error_response(
    track_id: i64,
    error: TidalPlaybackError,
    fallback_message: &str,
) -> (StatusCode, Json<Value>) {
    match error {
        TidalPlaybackError::NotConnected => (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "status": "not_connected",
                "message": "Connect TIDAL in Settings before playing.",
                "track_id": track_id,
            })),
        ),
        TidalPlaybackError::SessionRefreshFailed(message) => (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "status": "session_refresh_failed",
                "message": "TIDAL session could not be refreshed before playback.",
                "details": message,
                "track_id": track_id,
            })),
        ),
        TidalPlaybackError::StreamResolve(err) => match err {
            tidal_stream::StreamResolveError::SessionExpired { message } => (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "status": "session_expired",
                    "message": "TIDAL session expired while starting playback.",
                    "details": message,
                    "track_id": track_id,
                })),
            ),
            tidal_stream::StreamResolveError::SessionRefreshFailed { message } => (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "status": "session_refresh_failed",
                    "message": "TIDAL session could not be refreshed before playback.",
                    "details": message,
                    "track_id": track_id,
                })),
            ),
            tidal_stream::StreamResolveError::ResponseParseFailed { message } => (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "status": "response_parse_failed",
                    "message": fallback_message,
                    "details": message,
                    "track_id": track_id,
                })),
            ),
            tidal_stream::StreamResolveError::ManifestDecodeFailed { message } => (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "status": "manifest_decode_failed",
                    "message": "TIDAL playback manifest could not be decoded.",
                    "details": message,
                    "track_id": track_id,
                })),
            ),
            tidal_stream::StreamResolveError::ManifestParseFailed { message } => (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "status": "manifest_parse_failed",
                    "message": "TIDAL playback manifest could not be parsed.",
                    "details": message,
                    "track_id": track_id,
                })),
            ),
            tidal_stream::StreamResolveError::MissingStreamUrl => (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "status": "missing_stream_url",
                    "message": "TIDAL playback manifest did not contain a stream URL.",
                    "track_id": track_id,
                })),
            ),
            tidal_stream::StreamResolveError::MissingManifest => (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "status": "missing_manifest",
                    "message": "TIDAL playback response did not contain a manifest.",
                    "track_id": track_id,
                })),
            ),
            tidal_stream::StreamResolveError::StreamRejected { message } => (
                StatusCode::FORBIDDEN,
                Json(json!({
                    "status": "stream_rejected",
                    "message": "TIDAL rejected the playback request.",
                    "details": message,
                    "track_id": track_id,
                })),
            ),
            tidal_stream::StreamResolveError::RequestFailed { message } => (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "status": "stream_request_failed",
                    "message": fallback_message,
                    "details": message,
                    "track_id": track_id,
                })),
            ),
            tidal_stream::StreamResolveError::UpstreamHttp { status, body } => (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "status": "stream_upstream_http",
                    "message": format!("TIDAL returned {} while starting playback.", status),
                    "details": body,
                    "track_id": track_id,
                })),
            ),
        },
    }
}

async fn pause_playback(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    if let Some(runtime_handle) = current_playback_runtime(&state).await {
        if let Err(error) = runtime_handle.pause() {
            let message = format!("Failed to pause host audio playback: {error}");
            report_playback_failure(&state, &message);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    let snapshot = {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| player::pause(conn))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    pause_active_session(&state).await;

    let state_guard = state.read().await;
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);

    let snapshot = overlay_snapshot_with_external_track(&state, snapshot).await;
    Ok(Json(json!({ "state": snapshot.state })))
}

async fn resume_playback(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    if let Some(runtime_handle) = current_playback_runtime(&state).await {
        if let Err(error) = runtime_handle.resume() {
            let message = format!("Failed to resume host audio playback: {error}");
            report_playback_failure(&state, &message);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    }

    let snapshot = {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| player::resume(conn))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    resume_session_after_snapshot(&state, &snapshot).await;

    let state_guard = state.read().await;
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);

    let snapshot = overlay_snapshot_with_external_track(&state, snapshot).await;
    Ok(Json(json!({ "state": snapshot.state })))
}

// ─── Pending-row resolution ──────────────────────────────────────────────────
//
// Both the lazy (next_track caller) and background-eager (radio_start) paths
// share the same scoring constants. The lazy path also closes the
// playback_state NULL window after promotion; the background path does not.

const MATCH_QUALITY_THRESHOLD: f64 = 0.85;
const RESOLVER_POOL_SIZE: usize = 4;

// Scoring weights (two-field, no album metadata available from Last.fm).
// Three-field variant (0.55/0.35/0.10) applies when pending_album is stored —
// not yet in schema; constants named here to make the future wiring obvious.
const SCORE_W_ARTIST: f64 = 0.60;
const SCORE_W_TITLE: f64 = 0.40;

fn score_tidal_candidate(
    result_artist: &str,
    result_title: &str,
    pending_artist: &str,
    pending_title: &str,
) -> f64 {
    fn normalize(s: &str) -> String {
        s.to_ascii_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { ' ' })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
    let a = strsim::jaro_winkler(&normalize(result_artist), &normalize(pending_artist));
    let t = strsim::jaro_winkler(&normalize(result_title), &normalize(pending_title));
    SCORE_W_ARTIST * a + SCORE_W_TITLE * t
}

/// Background-eager resolver for a single pending queue row.
///
/// Spawned by `radio_start` after inserting pending rows. Bounded by
/// `Arc<Semaphore>` (RESOLVER_POOL_SIZE permits). Unlike the lazy path, this
/// does **not** update `playback_state.current_track_id` — the playing row
/// may not be the one being resolved.
async fn resolve_pending_row(
    db: crate::db::Database,
    tokens: crate::services::tidal::auth::TidalTokens,
    queue_item_id: i64,
) {
    let row: Option<(String, String)> = db
        .with_conn(move |conn| {
            Ok(conn
                .query_row(
                    "SELECT pending_artist, pending_title FROM queue
                     WHERE id = ?1 AND track_id IS NULL AND pending_at IS NOT NULL",
                    rusqlite::params![queue_item_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?)
        })
        .unwrap_or(None);

    let (pending_artist, pending_title) = match row {
        Some(r) => r,
        None => return,
    };

    let claimed = db
        .with_conn(move |conn| {
            Ok(conn.execute(
                "UPDATE queue SET resolving_at = datetime('now')
                 WHERE id = ?1 AND resolving_at IS NULL AND track_id IS NULL",
                rusqlite::params![queue_item_id],
            )? == 1)
        })
        .unwrap_or(false);
    if !claimed {
        return;
    }

    let release = |db: &crate::db::Database, qid: i64| {
        let _ = db.with_conn(move |conn| {
            conn.execute(
                "UPDATE queue SET resolving_at = NULL WHERE id = ?1",
                rusqlite::params![qid],
            )
            .map_err(anyhow::Error::from)
        });
    };

    let client = TidalClient::new(tokens.access_token.clone(), tokens.country_code.clone());
    let query = format!("{} {}", pending_artist, pending_title);
    let results = match client.search(&query, 5).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(queue_item_id, error = %e, "background resolver: Tidal search failed");
            release(&db, queue_item_id);
            return;
        }
    };

    let best = results
        .into_iter()
        .filter_map(|t| {
            let s = score_tidal_candidate(
                t.artist_name.as_deref().unwrap_or(""),
                &t.title,
                &pending_artist,
                &pending_title,
            );
            if s >= MATCH_QUALITY_THRESHOLD { Some((s, t)) } else { None }
        })
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let (score, tidal_track) = match best {
        Some(p) => p,
        None => {
            tracing::debug!(
                queue_item_id,
                artist = %pending_artist,
                title = %pending_title,
                "background resolver: no match above threshold"
            );
            release(&db, queue_item_id);
            return;
        }
    };

    let tidal_id = tidal_track.id;
    let imported = crate::services::tidal::import::import_track_from_metadata(
        &db,
        tidal_id,
        tidal_track.title,
        tidal_track.artist_name.unwrap_or_default(),
        tidal_track.artist_id,
        tidal_track.album_title,
        Some(tidal_track.duration * 1000),
    )
    .await;

    let local_id = match imported {
        Ok(imp) => imp.local_id,
        Err(e) => {
            tracing::warn!(queue_item_id, error = %e, "background resolver: import failed");
            release(&db, queue_item_id);
            return;
        }
    };

    let score_stored = (score * 1000.0) as i32;
    let promoted = db
        .with_conn(move |conn| {
            Ok(conn.execute(
                "UPDATE queue
                 SET track_id = ?1, resolved_at = datetime('now'),
                     tidal_match_score = ?2, resolving_at = NULL
                 WHERE id = ?3 AND track_id IS NULL",
                rusqlite::params![local_id, score_stored, queue_item_id],
            )? == 1)
        })
        .unwrap_or(false);

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
}

/// Attempts to resolve the current pending queue item to a Tidal track.
/// Called when the current queue item is a pending (unresolved) row — track_id IS NULL.
/// Claims ownership via resolving_at, searches Tidal with combined Jaro-Winkler scoring
/// (0.60×artist + 0.40×title, threshold 0.85), imports the match via
/// import_track_from_metadata, and atomically promotes the queue row.
///
/// Returns the resolved Track on success, or None if no acceptable match or on error.
/// On failure the resolving_at ownership lock is always released.
async fn resolve_pending_current_queue_item(
    state: &SharedState,
) -> Option<crate::db::models::Track> {
    let db = {
        let s = state.read().await;
        s.db.clone()
    };

    let (queue_item_id, pending_artist, pending_title): (i64, String, String) = db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT q.id, q.pending_artist, q.pending_title
                 FROM playback_state ps
                 JOIN queue q ON q.id = ps.current_queue_item_id
                 WHERE ps.id = 1 AND q.track_id IS NULL AND q.pending_at IS NOT NULL",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?)
        })
        .ok()
        .flatten()?;

    // Claim ownership; bail if another resolver already claimed this row.
    let claimed = db
        .with_conn(|conn| {
            Ok(conn.execute(
                "UPDATE queue SET resolving_at = datetime('now')
                 WHERE id = ?1 AND resolving_at IS NULL AND track_id IS NULL",
                rusqlite::params![queue_item_id],
            )? == 1)
        })
        .unwrap_or(false);
    if !claimed {
        return None;
    }

    let release_lock = |db: &crate::db::Database, qid: i64| {
        let _ = db.with_conn(move |conn| {
            conn.execute(
                "UPDATE queue SET resolving_at = NULL WHERE id = ?1",
                rusqlite::params![qid],
            )
            .map_err(anyhow::Error::from)
        });
    };

    let tokens = {
        let persisted = match load_persisted_tidal_tokens(state).await.ok().flatten() {
            Some(t) => t,
            None => { release_lock(&db, queue_item_id); return None; }
        };
        let s = state.read().await;
        s.tidal_tokens.clone().unwrap_or(persisted)
    };

    let client = TidalClient::new(tokens.access_token.clone(), tokens.country_code.clone());
    let query = format!("{} {}", pending_artist, pending_title);
    let results = match client.search(&query, 5).await {
        Ok(r) => r,
        Err(_) => { release_lock(&db, queue_item_id); return None; }
    };

    let best = results
        .into_iter()
        .filter_map(|t| {
            let s = score_tidal_candidate(
                t.artist_name.as_deref().unwrap_or(""),
                &t.title,
                &pending_artist,
                &pending_title,
            );
            if s >= MATCH_QUALITY_THRESHOLD { Some((s, t)) } else { None }
        })
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let (score, tidal_track) = match best {
        Some(pair) => pair,
        None => { release_lock(&db, queue_item_id); return None; }
    };

    let tidal_id = tidal_track.id;
    let duration_ms = Some(tidal_track.duration * 1000);
    let imported = crate::services::tidal::import::import_track_from_metadata(
        &db,
        tidal_id,
        tidal_track.title,
        tidal_track.artist_name.unwrap_or_default(),
        tidal_track.artist_id,
        tidal_track.album_title,
        duration_ms,
    )
    .await;

    let local_id = match imported {
        Ok(imp) => imp.local_id,
        Err(_) => { release_lock(&db, queue_item_id); return None; }
    };

    let score_stored = (score * 1000.0) as i32;
    // Atomic promotion: only one resolver wins even under a race.
    let promoted = db
        .with_conn(move |conn| {
            Ok(conn.execute(
                "UPDATE queue
                 SET track_id = ?1, resolved_at = datetime('now'),
                     tidal_match_score = ?2, resolving_at = NULL
                 WHERE id = ?3 AND track_id IS NULL",
                rusqlite::params![local_id, score_stored, queue_item_id],
            )? == 1)
        })
        .unwrap_or(false);

    if !promoted {
        return None;
    }

    // Close the NULL window so playback_state reflects the real track.
    let _ = db.with_conn(move |conn| {
        conn.execute(
            "UPDATE playback_state SET current_track_id = ?1 WHERE id = 1",
            rusqlite::params![local_id],
        )
        .map_err(anyhow::Error::from)
    });

    db.with_conn(move |conn| queue::get_track_by_id(conn, local_id))
        .ok()
        .flatten()
}

async fn next_track(
    State(state): State<SharedState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let previous_track_id = current_playback_track_id(&state).await;
    let mut snapshot = {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| player::next_track(conn))
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "status": "playback_state_update_failed",
                        "message": "Failed to advance playback state.",
                    })),
                )
            })?
    };

    set_external_playback_track(&state, None).await;

    // If the new current item has no library track (pending row), resolve it to Tidal now.
    let effective_current_track: Option<crate::db::models::Track> =
        if let Some(t) = snapshot.state.current_track.clone() {
            Some(t)
        } else {
            let resolved = resolve_pending_current_queue_item(&state).await;
            match resolved {
                Some(ref t) => {
                    // Store for snapshot overlay at the bottom of this handler.
                    set_external_playback_track(&state, Some(t.clone())).await;
                }
                None => {
                    // Unresolvable: advance one more step to skip the dead row.
                    if let Ok(next_snapshot) = {
                        let s = state.read().await;
                        s.db.with_conn(|conn| player::next_track(conn))
                    } {
                        snapshot = next_snapshot;
                    }
                }
            }
            resolved
        };

    record_transition_if_changed(&state, previous_track_id, &snapshot, "queue", true).await;

    // When "Include New" is enabled, search TIDAL for genre/artist-matched tracks and
    // inject any that aren't already in the library. Runs as a detached background task
    // so the next_track response returns immediately without blocking on TIDAL API calls.
    if snapshot.state.automix_discover_new {
        let current_pos = snapshot
            .state
            .current_queue_item_id
            .and_then(|qid| snapshot.queue.iter().find(|q| q.id == qid).map(|q| q.position))
            .or_else(|| {
                snapshot.state.current_track.as_ref().and_then(|t| {
                    snapshot.queue.iter().find(|q| q.track.id == t.id).map(|q| q.position)
                })
            })
            .unwrap_or(0);
        let new_upcoming = snapshot
            .queue
            .iter()
            .filter(|q| q.position > current_pos && q.source == "automix-new")
            .count();
        if new_upcoming < 2 {
            if let Some(track) = snapshot.state.current_track.clone() {
                let bg_state = state.clone();
                tokio::spawn(async move {
                    inject_discovery_tracks(&bg_state, &track).await;
                });
            }
        }
    }

    // A resolved pending track counts as Replaced, not QueueEnded.
    let end_reason = if effective_current_track.is_some() || snapshot.state.current_track.is_some() {
        Some(player::ListenSessionEndReason::Replaced)
    } else {
        Some(player::ListenSessionEndReason::QueueEnded)
    };
    sync_session_after_snapshot(&state, &snapshot, end_reason).await;

    // Use the resolved pending track if the snapshot has no library track.
    let play_track = effective_current_track
        .as_ref()
        .or(snapshot.state.current_track.as_ref());

    if let Some(track) = play_track {
        let user_quality = current_user_audio_quality(&state).await;
        let stream_request = player::build_tidal_stream_request(track, user_quality.clone()).ok_or_else(|| {
            (
                StatusCode::NOT_IMPLEMENTED,
                Json(json!({
                    "status": "local_playback_not_supported",
                    "message": "Local-library playback is not wired into the host audio runtime yet.",
                    "track_id": track.id,
                })),
            )
        })?;
        let stream_info = resolve_tidal_playback_stream(&state, track, &stream_request)
            .await
            .map_err(|error| {
                tidal_playback_error_response(
                    track.id,
                    error,
                    "TIDAL stream could not be resolved while advancing playback.",
                )
            })?;
        let runtime_handle = ensure_playback_runtime_for_track(&state, track)
            .await
            .map_err(|_| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({
                        "status": "playback_runtime_unavailable",
                        "message": "Playback runtime was not available for advancing playback.",
                        "track_id": track.id,
                    })),
                )
            })?;
        let job = player::build_playback_preparation(
            track,
            Some(&stream_info),
            effective_crossfade_ms(&state, snapshot.state.crossfade_ms).await,
            user_quality,
        );
        runtime_handle.switch_to(job).map_err(|error| {
            let message = format!("Failed to switch host audio playback: {error}");
            report_playback_failure(&state, &message);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "status": "playback_runtime_failed",
                    "message": message,
                    "track_id": track.id,
                })),
            )
        })?;
        {
            let mut state_guard = state.write().await;
            state_guard.current_stream_display = Some(crate::StreamDisplayInfo {
                audio_quality: stream_info.audio_quality.clone(),
                sample_rate: stream_info.sample_rate,
                bit_depth: stream_info.bit_depth,
            });
            state_guard.pending_stream_display = None;
        }
    } else if let Some(runtime_handle) = current_playback_runtime(&state).await {
        let _ = runtime_handle.stop();
    }

    let state_guard = state.read().await;
    let event_track_id = effective_current_track
        .as_ref()
        .or(snapshot.state.current_track.as_ref())
        .map(|t| t.id);
    if let Some(track_id) = event_track_id {
        let _ = state_guard
            .event_tx
            .send(AppEvent::TrackChanged { track_id });
    }
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);

    let snapshot = overlay_snapshot_with_external_track(&state, snapshot).await;
    Ok(Json(json!({
        "state": snapshot.state,
        "queue": snapshot.queue
    })))
}

async fn previous_track(
    State(state): State<SharedState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let previous_track_id = current_playback_track_id(&state).await;
    let snapshot = {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| player::previous_track(conn))
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "status": "playback_state_update_failed",
                        "message": "Failed to move to the previous track.",
                    })),
                )
            })?
    };

    set_external_playback_track(&state, None).await;
    record_transition_if_changed(&state, previous_track_id, &snapshot, "user", false).await;

    sync_session_after_snapshot(
        &state,
        &snapshot,
        Some(player::ListenSessionEndReason::Replaced),
    )
    .await;

    if let Some(track) = snapshot.state.current_track.as_ref() {
        let user_quality = current_user_audio_quality(&state).await;
        let stream_request = player::build_tidal_stream_request(track, user_quality.clone()).ok_or_else(|| {
            (
                StatusCode::NOT_IMPLEMENTED,
                Json(json!({
                    "status": "local_playback_not_supported",
                    "message": "Local-library playback is not wired into the host audio runtime yet.",
                    "track_id": track.id,
                })),
            )
        })?;
        let stream_info = resolve_tidal_playback_stream(&state, track, &stream_request)
            .await
            .map_err(|error| {
                tidal_playback_error_response(
                    track.id,
                    error,
                    "TIDAL stream could not be resolved while moving to the previous track.",
                )
            })?;
        let runtime_handle = ensure_playback_runtime_for_track(&state, track)
            .await
            .map_err(|_| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({
                        "status": "playback_runtime_unavailable",
                        "message": "Playback runtime was not available for moving to the previous track.",
                        "track_id": track.id,
                    })),
                )
            })?;
        let job = player::build_playback_preparation(
            track,
            Some(&stream_info),
            effective_crossfade_ms(&state, snapshot.state.crossfade_ms).await,
            user_quality,
        );
        runtime_handle.switch_to(job).map_err(|error| {
            let message = format!("Failed to switch host audio playback: {error}");
            report_playback_failure(&state, &message);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "status": "playback_runtime_failed",
                    "message": message,
                    "track_id": track.id,
                })),
            )
        })?;
        {
            let mut state_guard = state.write().await;
            state_guard.current_stream_display = Some(crate::StreamDisplayInfo {
                audio_quality: stream_info.audio_quality.clone(),
                sample_rate: stream_info.sample_rate,
                bit_depth: stream_info.bit_depth,
            });
            state_guard.pending_stream_display = None;
        }
    }

    let state_guard = state.read().await;
    if let Some(track) = snapshot.state.current_track.as_ref() {
        let _ = state_guard
            .event_tx
            .send(AppEvent::TrackChanged { track_id: track.id });
    }
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);

    let snapshot = overlay_snapshot_with_external_track(&state, snapshot).await;
    Ok(Json(json!({
        "state": snapshot.state,
        "queue": snapshot.queue
    })))
}

async fn set_playback_position(
    State(state): State<SharedState>,
    Json(payload): Json<PositionRequest>,
) -> Result<Json<Value>, StatusCode> {
    let state_guard = state.read().await;
    // Seek in the live audio runtime (updates the CPAL sample cursor).
    if let Some(runtime) = state_guard.playback_runtime.as_ref() {
        let _ = runtime.handle.seek(payload.position_ms);
    }
    let snapshot = state_guard
        .db
        .with_conn(|conn| player::set_position(conn, payload.position_ms))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    drop(state_guard);
    let snapshot = overlay_snapshot_with_external_track(&state, snapshot).await;
    Ok(Json(json!({ "state": snapshot.state })))
}

async fn set_playback_volume(
    State(state): State<SharedState>,
    Json(payload): Json<VolumeRequest>,
) -> Result<Json<Value>, StatusCode> {
    let state_guard = state.read().await;
    // Apply volume to the live audio stream immediately.
    if let Some(runtime) = state_guard.playback_runtime.as_ref() {
        runtime.handle.set_volume(payload.volume as f32);
    }
    let snapshot = state_guard
        .db
        .with_conn(|conn| player::set_volume(conn, payload.volume))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    drop(state_guard);
    let snapshot = overlay_snapshot_with_external_track(&state, snapshot).await;
    Ok(Json(json!({ "state": snapshot.state })))
}

async fn set_playback_shuffle(
    State(state): State<SharedState>,
    Json(payload): Json<ShuffleModeRequest>,
) -> Result<Json<Value>, StatusCode> {
    let mode = queue::ShuffleMode::parse(&payload.mode);
    let state_guard = state.read().await;
    let snapshot = state_guard
        .db
        .with_conn(|conn| player::set_shuffle_mode(conn, mode))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
    drop(state_guard);
    let snapshot = overlay_snapshot_with_external_track(&state, snapshot).await;
    Ok(Json(json!({
        "state": snapshot.state,
        "queue": snapshot.queue
    })))
}

async fn set_playback_repeat(
    State(state): State<SharedState>,
    Json(payload): Json<RepeatModeRequest>,
) -> Result<Json<Value>, StatusCode> {
    let state_guard = state.read().await;
    let snapshot = state_guard
        .db
        .with_conn(|conn| player::set_repeat_mode(conn, &payload.mode))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    drop(state_guard);
    let snapshot = overlay_snapshot_with_external_track(&state, snapshot).await;
    Ok(Json(json!({ "state": snapshot.state })))
}

async fn set_playback_automix(
    State(state): State<SharedState>,
    Json(payload): Json<AutomixRequest>,
) -> Result<Json<Value>, StatusCode> {
    let state_guard = state.read().await;
    let snapshot = state_guard
        .db
        .with_conn(|conn| {
            if let Some(ms) = payload.crossfade_ms {
                player::set_crossfade_ms(conn, ms)?;
            }
            if let Some(dn) = payload.discover_new {
                player::set_automix_discover_new(conn, dn)?;
            }
            if let Some(use_learning) = payload.use_learning {
                player::set_automix_use_learning(conn, use_learning)?;
            }
            if let Some(allow_external) = payload.allow_external {
                player::set_automix_allow_external(conn, allow_external)?;
            }
            player::set_automix_enabled(conn, payload.enabled)
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);

    drop(state_guard);
    let snapshot = overlay_snapshot_with_external_track(&state, snapshot).await;
    Ok(Json(
        json!({ "state": snapshot.state, "queue": snapshot.queue }),
    ))
}

async fn add_queue_track(
    State(state): State<SharedState>,
    Json(payload): Json<PlaybackTrackRequest>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let queue = player::enqueue_track(conn, payload.track_id, "user")?;
            let _ = state.event_tx.send(AppEvent::QueueUpdated);
            Ok(Json(json!({ "queue": queue })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn replace_playback_queue(
    State(state): State<SharedState>,
    Json(payload): Json<QueueReplaceRequest>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            // Replace with library tracks first.
            match payload.reasons.as_ref() {
                Some(reasons) => player::replace_queue_with_reasons(
                    conn,
                    &payload.track_ids,
                    reasons,
                    "user",
                )?,
                None => player::replace_queue_with_tracks(conn, &payload.track_ids, "user")?,
            };
            // Phase 2c-ii-a: append pending (last.fm) candidates after library tracks.
            if let Some(pending) = &payload.pending_candidates {
                if !pending.is_empty() {
                    use crate::playback::queue::{PendingCandidate, append_pending_tracks};
                    let candidates: Vec<PendingCandidate> = pending
                        .iter()
                        .map(|p| PendingCandidate {
                            artist: p.artist.clone(),
                            title: p.title.clone(),
                            reason: p.reason.clone(),
                        })
                        .collect();
                    append_pending_tracks(conn, &candidates)?;
                }
            }
            let final_queue = crate::playback::queue::load_queue(conn)?;
            let _ = state.event_tx.send(AppEvent::QueueUpdated);
            Ok(Json(json!({ "queue": final_queue })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn remove_queue_track(
    State(state): State<SharedState>,
    Json(payload): Json<QueueRemoveRequest>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            queue::remove_queue_item(conn, payload.queue_item_id)?;
            let queue = queue::load_queue(conn)?;
            let _ = state.event_tx.send(AppEvent::QueueUpdated);
            Ok(Json(json!({ "queue": queue })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn move_queue_track(
    State(state): State<SharedState>,
    Json(payload): Json<QueueMoveRequest>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            queue::move_queue_item(conn, payload.item_id, payload.new_pos)?;
            let queue = queue::load_queue(conn)?;
            let _ = state.event_tx.send(AppEvent::QueueUpdated);
            Ok(Json(json!({ "queue": queue })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn clear_queue_route(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let (current_track_id, current_queue_item_id): (Option<i64>, Option<i64>) =
                conn.query_row(
                    "SELECT current_track_id, current_queue_item_id FROM playback_state WHERE id = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
            match (current_track_id, current_queue_item_id) {
                (Some(track_id), _) => {
                    // Library track playing — preserve by track_id.
                    conn.execute("DELETE FROM queue WHERE track_id != ?1", params![track_id])?;
                }
                (None, Some(qid)) => {
                    // Pending row playing — preserve by queue item id.
                    conn.execute("DELETE FROM queue WHERE id != ?1", params![qid])?;
                }
                (None, None) => {
                    queue::clear_queue(conn)?;
                }
            }
            let queue = queue::load_queue(conn)?;
            let _ = state.event_tx.send(AppEvent::QueueUpdated);
            Ok(Json(json!({ "queue": queue })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn create_playlist_from_queue(
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
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let items = queue::load_queue(conn)?;
            let track_ids: Vec<i64> = items
                .iter()
                .filter(|item| {
                    if include_tidal_only {
                        true
                    } else {
                        // "tidal_stream" / "tidal_ephemeral" sources are tidal-only;
                        // local + best_source != tidal counts as on-disk.
                        item.track.source.as_str() != "tidal_stream"
                            && item.track.source.as_str() != "tidal_ephemeral"
                    }
                })
                .map(|item| item.track.id)
                .filter(|id| *id > 0)
                .collect();

            // Insert empty playlist row, then bulk-add tracks.
            conn.execute(
                "INSERT INTO playlists (name, description, is_smart, is_synced, track_count)
                 VALUES (?1, NULL, 0, 0, 0)",
                params![name],
            )?;
            let playlist_id = conn.last_insert_rowid();
            let added = queries::add_tracks_to_playlist(conn, playlist_id, &track_ids)?;
            let playlist = queries::get_playlist(conn, playlist_id)?
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
        })
}

async fn status() -> Json<Value> {
    Json(json!({
        "name": "NOOR",
        "version": env!("CARGO_PKG_VERSION"),
        "status": "running"
    }))
}

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
    let genre_map = queries::get_track_genre_paths(conn)?;
    let playlist_memberships = queries::get_playlist_memberships(conn)?;
    let dsp_rows = queries::get_all_audio_dsp_features(conn)?;
    let acrcloud_ids = queries::get_track_ids_with_acrcloud_match(conn)?;
    let fingerprint_ids = queries::get_track_ids_with_fingerprint(conn)?;

    let mut context = PlaylistEvaluationContext::new();
    for (track_id, genres) in genre_map {
        context = context.with_track_genres(track_id, genres);
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

// ─── TIDAL Endpoints ──────────────────────────────────────

/// Start device code login flow. Returns user_code and verify_url.
async fn tidal_login(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let http = {
        let s = state.read().await;
        s.http_client.clone()
    };

    let (device_code, user_code, verify_url, interval) =
        tidal_auth::start_device_login(&http).await.map_err(|e| {
            tracing::error!("TIDAL login error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Cancel any previous in-flight login polling
    {
        let mut s = state.write().await;
        s.tidal_login_cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        s.tidal_login_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    }

    // Poll for token in background, then persist to DB
    let state_clone = state.clone();
    let http_clone = http.clone();
    let cancel = {
        let s = state.read().await;
        s.tidal_login_cancel.clone()
    };
    tokio::spawn(async move {
        loop {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                tracing::info!("TIDAL login polling cancelled (new login started)");
                return;
            }
            match tidal_auth::poll_for_token(&http_clone, &device_code, interval).await {
                Ok(tokens) => {
                    tracing::info!("TIDAL auth successful! User: {}", tokens.user_id);
                    // Persist tokens to DB so they survive restarts
                    {
                        let s = state_clone.read().await;
                        let _ = s.db.with_conn(|conn| {
                            let token_json = serde_json::to_string(&tokens)?;
                            conn.execute(
                                "INSERT INTO service_auth (service, access_token_enc, user_id, connected_at)
                                 VALUES ('tidal', ?1, ?2, datetime('now'))
                                 ON CONFLICT(service) DO UPDATE SET access_token_enc=excluded.access_token_enc,
                                 user_id=excluded.user_id, connected_at=excluded.connected_at",
                                rusqlite::params![token_json.as_bytes(), tokens.user_id],
                            )?;
                            Ok(())
                        });
                    }
                    let mut s = state_clone.write().await;
                    s.tidal_tokens = Some(tokens);
                    let _ = s.event_tx.send(AppEvent::PlaybackStateChanged);
                    return;
                }
                Err(e) => {
                    tracing::error!("TIDAL polling failed: {}", e);
                    return;
                }
            }
        }
    });

    Ok(Json(json!({
        "user_code": user_code,
        "verify_url": verify_url,
        "interval": interval,
    })))
}

/// Check if polling has completed (frontend polls this).
async fn tidal_poll(State(state): State<SharedState>) -> Json<Value> {
    let in_memory_tokens = {
        let s = state.read().await;
        s.tidal_tokens.clone()
    };
    let tokens = match in_memory_tokens {
        Some(tokens) => Some(tokens),
        None => match load_persisted_tidal_tokens(&state).await {
            Ok(tokens) => tokens,
            Err(error) => {
                tracing::warn!(
                    "Failed to rehydrate persisted TIDAL tokens during login poll: {}",
                    error
                );
                None
            }
        },
    };

    if let Some(tokens) = tokens {
        Json(json!({
            "status": "authenticated",
            "user_id": tokens.user_id,
            "country_code": tokens.country_code,
        }))
    } else {
        Json(json!({
            "status": "pending",
        }))
    }
}

async fn load_persisted_tidal_tokens(
    state: &SharedState,
) -> anyhow::Result<Option<tidal_auth::TidalTokens>> {
    let db = {
        let s = state.read().await;
        s.db.clone()
    };

    let tokens = db.with_conn(|conn| {
        let result = conn.query_row(
            "SELECT access_token_enc FROM service_auth WHERE service='tidal'",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        );

        Ok(match result {
            Ok(bytes) => String::from_utf8(bytes)
                .ok()
                .and_then(|json| serde_json::from_str::<tidal_auth::TidalTokens>(&json).ok()),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(error) => return Err(error.into()),
        })
    })?;

    if let Some(ref tokens) = tokens {
        let mut s = state.write().await;
        s.tidal_tokens = Some(tokens.clone());
    }

    Ok(tokens)
}

/// Get TIDAL backoff gate status.
async fn get_tidal_backoff_status() -> impl axum::response::IntoResponse {
    let state = crate::services::tidal::backoff::global().state();
    axum::Json(state)
}

/// Get TIDAL connection status.
async fn tidal_status(State(state): State<SharedState>) -> Json<Value> {
    let in_memory_tokens = {
        let s = state.read().await;
        s.tidal_tokens.clone()
    };
    let tokens = match in_memory_tokens {
        Some(tokens) => Some(tokens),
        None => match load_persisted_tidal_tokens(&state).await {
            Ok(tokens) => tokens,
            Err(error) => {
                tracing::warn!("Failed to rehydrate persisted TIDAL tokens: {}", error);
                None
            }
        },
    };

    if let Some(tokens) = tokens {
        Json(json!({
            "connected": true,
            "user_id": tokens.user_id,
            "country_code": tokens.country_code,
        }))
    } else {
        Json(json!({
            "connected": false,
        }))
    }
}

/// Clear TIDAL session (logout).
async fn tidal_logout(State(state): State<SharedState>) -> Json<Value> {
    tracing::info!(target: "noor.sync.tidal", event = "session_logout", "TIDAL session cleared by user");
    let _ = clear_tidal_session(&state).await;
    Json(json!({ "status": "logged_out" }))
}

// ─── TIDAL Search ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TidalSearchParams {
    q: String,
    limit: Option<i32>,
}

#[derive(Serialize)]
struct TidalSearchTrackResp {
    tidal_id: i64,
    title: String,
    duration_ms: i64,
    artist_id: Option<i64>,
    artist_name: Option<String>,
    album_title: Option<String>,
    album_tidal_id: Option<i64>,
    artwork_url: Option<String>,
    audio_quality: Option<String>,
    stream_ready: Option<bool>,
    local_id: Option<i64>,
    in_library: bool,
}

#[derive(Serialize)]
struct TidalSearchAlbumResp {
    tidal_id: i64,
    title: String,
    artist_name: Option<String>,
    artwork_url: Option<String>,
    local_id: Option<i64>,
    in_library: bool,
}

#[derive(Serialize)]
struct TidalSearchArtistResp {
    tidal_id: i64,
    name: String,
    artwork_url: Option<String>,
    local_id: Option<i64>,
    in_library: bool,
}

async fn tidal_search(
    State(state): State<SharedState>,
    Query(params): Query<TidalSearchParams>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let tokens = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
        })?;
        let s = state.read().await;
        s.tidal_tokens.clone().or(persisted)
    };

    let Some(tokens) = tokens else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let limit = params.limit.unwrap_or(20).min(50);
    let http_client = state.read().await.http_client.clone();

    let client = TidalClient::new(tokens.access_token.clone(), tokens.country_code.clone());
    let results = match client.search_catalog(&params.q, limit).await {
        Ok(r) => r,
        Err(e) if error_looks_like_auth(&e) => {
            let refreshed = recover_tidal_session(&state, &http_client, &tokens)
                .await
                .map_err(|re| (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": format!("TIDAL session refresh failed: {}", re) })),
                ))?;
            let retry_client = TidalClient::new(
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            retry_client.search_catalog(&params.q, limit).await.map_err(|e2| {
                (StatusCode::BAD_GATEWAY, Json(json!({ "error": e2.to_string() })))
            })?
        }
        Err(e) => return Err((
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": e.to_string() })),
        )),
    };

    // Batch-lookup which Tidal IDs are in the local library so the frontend can
    // route to local pages and badge entries as in-library.
    let track_tidal_ids: Vec<i64> = results.tracks.iter().map(|t| t.id).collect();
    let album_tidal_ids: Vec<i64> = results.albums.iter().map(|a| a.id).collect();
    let artist_tidal_ids: Vec<i64> = results.artists.iter().map(|a| a.id).collect();
    let (track_map, known_albums, known_artists, artist_photos) = {
        let s = state.read().await;
        s.db
            .with_conn(|conn| {
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

    Ok(Json(json!({ "tracks": tracks, "albums": albums, "artists": artists })))
}

// ─── TIDAL Playlist Search + Tracks ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TidalPlaylistSearchParams {
    q: String,
    #[serde(default)]
    limit: Option<i32>,
}

async fn tidal_playlist_search(
    State(state): State<SharedState>,
    Query(params): Query<TidalPlaylistSearchParams>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let tokens = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
        })?;
        let s = state.read().await;
        s.tidal_tokens.clone().or(persisted)
    };
    let Some(tokens) = tokens else {
        return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "TIDAL not connected" }))));
    };

    let limit = params.limit.unwrap_or(20).min(50);
    let http_client = state.read().await.http_client.clone();
    let client = TidalClient::new(tokens.access_token.clone(), tokens.country_code.clone());
    let playlists = match client.search_playlists(&params.q, limit).await {
        Ok(r) => r,
        Err(e) if error_looks_like_auth(&e) => {
            let refreshed = recover_tidal_session(&state, &http_client, &tokens)
                .await
                .map_err(|re| (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": format!("TIDAL session refresh failed: {}", re) })),
                ))?;
            let retry_client = TidalClient::new(
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            retry_client.search_playlists(&params.q, limit).await.map_err(|e2| {
                (StatusCode::BAD_GATEWAY, Json(json!({ "error": e2.to_string() })))
            })?
        }
        Err(e) => return Err((StatusCode::BAD_GATEWAY, Json(json!({ "error": e.to_string() })))),
    };

    Ok(Json(json!({ "playlists": playlists })))
}

async fn tidal_playlist_tracks(
    State(state): State<SharedState>,
    Path(uuid): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let tokens = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
        })?;
        let s = state.read().await;
        s.tidal_tokens.clone().or(persisted)
    };
    let Some(tokens) = tokens else {
        return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "TIDAL not connected" }))));
    };

    let http_client = state.read().await.http_client.clone();
    let client = TidalClient::new(tokens.access_token.clone(), tokens.country_code.clone());
    let resp = match client.get_playlist_tracks(&uuid, 100, 0).await {
        Ok(r) => r,
        Err(e) if error_looks_like_auth(&e) => {
            let refreshed = recover_tidal_session(&state, &http_client, &tokens)
                .await
                .map_err(|re| (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": format!("TIDAL session refresh failed: {}", re) })),
                ))?;
            let retry_client = TidalClient::new(
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            retry_client.get_playlist_tracks(&uuid, 100, 0).await.map_err(|e2| {
                (StatusCode::BAD_GATEWAY, Json(json!({ "error": e2.to_string() })))
            })?
        }
        Err(e) => return Err((StatusCode::BAD_GATEWAY, Json(json!({ "error": e.to_string() })))),
    };

    // TidalTrack: id (i64), title (String), duration (i64), artist (TidalArtist, not Option),
    // album: Option<TidalAlbumRef> with cover: Option<String>
    let playable: Vec<serde_json::Value> = resp.items.iter().map(|t| {
        json!({
            "tidal_id": t.id,
            "title": t.title,
            "artist_name": t.artist.name,
            "album_title": t.album.as_ref().map(|a| &a.title),
            "artwork_url": t.album.as_ref().and_then(|a| a.cover.as_ref()).map(|c| {
                format!("https://resources.tidal.com/images/{}/320x320.jpg", c.replace('-', "/"))
            }),
            "duration_ms": t.duration * 1000,
            "track_id": 0,
            "is_in_library": false,
        })
    }).collect();

    Ok(Json(json!({ "tracks": playable })))
}

#[derive(Debug, serde::Deserialize)]
struct PlayTidalRequest {
    tidal_track_id: i64,
    title: String,
    artist_name: Option<String>,
    album_title: Option<String>,
    artwork_url: Option<String>,
    duration_ms: Option<i64>,
}

async fn play_tidal_ephemeral(
    State(state): State<SharedState>,
    Json(body): Json<PlayTidalRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    // Resolve TIDAL tokens (same pattern as tidal_search)
    let tokens = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
        })?;
        let s = state.read().await;
        s.tidal_tokens.clone().or(persisted)
    };

    let Some(tokens) = tokens else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    // Resolve stream URL
    let http_client = {
        let s = state.read().await;
        s.http_client.clone()
    };
    let stream_req = tidal_stream::StreamRequest::new(body.tidal_track_id, "LOSSLESS");
    let stream_info = match tidal_stream::resolve_stream(
        &http_client,
        &tokens.access_token,
        &stream_req,
    )
    .await
    {
        Ok(info) => info,
        Err(e) if e.is_session_expired() => {
            let refreshed = recover_tidal_session(&state, &http_client, &tokens)
                .await
                .map_err(|re| (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": format!("TIDAL session refresh failed: {}", re) })),
                ))?;
            tidal_stream::resolve_stream(
                &http_client,
                &refreshed.access_token,
                &stream_req,
            )
            .await
            .map_err(|e2| (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("TIDAL stream resolve failed: {e2}") })),
            ))?
        }
        Err(e) => return Err((
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": format!("TIDAL stream resolve failed: {e}") })),
        )),
    };

    // Build a synthetic Track with a negative id to avoid any DB collision
    let synthetic = crate::db::models::Track {
        id: -body.tidal_track_id,
        title: body.title.clone(),
        artist_id: 0,
        artist_name: body.artist_name.clone(),
        album_id: None,
        album_title: body.album_title.clone(),
        disc_number: None,
        track_number: None,
        duration_ms: body.duration_ms,
        isrc: None,
        tidal_id: Some(body.tidal_track_id),
        ytmusic_id: None,
        soundcloud_id: None,
        best_quality: Some("LOSSLESS".to_string()),
        best_source: Some("tidal".to_string()),
        fidelity_score: 0,
        is_favorite: false,
        play_count: 0,
        last_played_at: None,
        date_added: None,
        source: "tidal_ephemeral".to_string(),
        artwork_url: body.artwork_url.clone(),
    };

    // Build the playback job and start it via the runtime
    let crossfade_ms = current_crossfade_ms(&state).await;
    let job = player::build_playback_preparation(&synthetic, Some(&stream_info), crossfade_ms, None);
    let runtime_handle = ensure_playback_runtime_for_track(&state, &synthetic).await?;
    runtime_handle.play(job).map_err(|e| {
        let message = format!("Failed to start host audio playback: {e}");
        report_playback_failure(&state, &message);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": message })),
        )
    })?;

    // Store ephemeral track for state overlay and clear DB current_track_id
    {
        let mut state_guard = state.write().await;
        state_guard.ephemeral_tidal_track = Some(synthetic);
        state_guard.current_stream_display = Some(crate::StreamDisplayInfo {
            audio_quality: stream_info.audio_quality.clone(),
            sample_rate: stream_info.sample_rate,
            bit_depth: stream_info.bit_depth,
        });
        state_guard.pending_stream_display = None;
    }
    let snapshot = {
        let state_guard = state.read().await;
        let snapshot = state_guard
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE playback_state SET current_track_id = NULL, position_ms = 0, is_playing = 1 WHERE id = 1",
                    [],
                )?;
                player::load_snapshot(conn)
            })
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": format!("DB update failed: {e}") })),
                )
            })?;
        let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
        snapshot
    };
    sync_session_after_snapshot(&state, &snapshot, Some(player::ListenSessionEndReason::Replaced)).await;

    Ok(Json(json!({ "ok": true })))
}

async fn tidal_artist_profile(
    State(state): State<SharedState>,
    Path(tidal_artist_id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let tokens = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
        })?;
        let s = state.read().await;
        s.tidal_tokens.clone().or(persisted)
    };

    let Some(tokens) = tokens else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let http_client = state.read().await.http_client.clone();
    let client = TidalClient::new(tokens.access_token.clone(), tokens.country_code.clone());
    let (top_tracks_page, albums_page) = match tokio::try_join!(
        client.get_artist_top_tracks(tidal_artist_id, 10, 0),
        client.get_artist_albums(tidal_artist_id, 50, 0, Some("ALBUMS")),
    ) {
        Ok(pair) => pair,
        Err(e) if error_looks_like_auth(&e) => {
            let refreshed = recover_tidal_session(&state, &http_client, &tokens)
                .await
                .map_err(|re| (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": format!("TIDAL session refresh failed: {}", re) })),
                ))?;
            let retry_client = TidalClient::new(
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            tokio::try_join!(
                retry_client.get_artist_top_tracks(tidal_artist_id, 10, 0),
                retry_client.get_artist_albums(tidal_artist_id, 50, 0, Some("ALBUMS")),
            )
            .map_err(|e2| (StatusCode::BAD_GATEWAY, Json(json!({ "error": e2.to_string() }))))?
        }
        Err(e) => return Err((
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": e.to_string() })),
        )),
    };

    let artist_name = top_tracks_page
        .items
        .first()
        .map(|t| t.artist.name.clone());

    Ok(Json(json!({
        "artist_name": artist_name,
        "top_tracks": top_tracks_page.items,
        "albums": albums_page.items,
    })))
}

/// Get sync info (last sync time, auto-sync settings).
async fn get_sync_info(
    State(state): State<SharedState>,
    Query(params): Query<serde_json::Map<String, serde_json::Value>>,
) -> Result<Json<Value>, StatusCode> {
    let service = params.get("service").and_then(|v| v.as_str()).unwrap_or("tidal");
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let info = queries::get_sync_info(conn, service)
                .map_err(|e| anyhow::anyhow!("sync info failed: {e}"))?;
            Ok(Json(json!({ "sync": info })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Set auto-sync daily toggle.
#[derive(Debug, Deserialize)]
struct AutoSyncRequest {
    service: Option<String>,
    enabled: bool,
}

async fn set_auto_sync(
    State(state): State<SharedState>,
    Json(payload): Json<AutoSyncRequest>,
) -> Result<Json<Value>, StatusCode> {
    let service = payload.service.as_deref().unwrap_or("tidal");
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            queries::set_auto_sync_daily(conn, service, payload.enabled)
                .map_err(|e| anyhow::anyhow!("set auto sync failed: {e}"))?;
            Ok(Json(json!({ "service": service, "auto_sync_daily": payload.enabled })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Public function to trigger auto-sync from server startup.
pub async fn trigger_auto_sync(state: &SharedState, service: &str) -> anyhow::Result<SyncStats> {
    if service != "tidal" {
        return Err(anyhow::anyhow!("Unsupported auto-sync service: {}", service));
    }

    // Get tokens
    let persisted_tokens = load_persisted_tidal_tokens(state).await?;
    let tokens = {
        let s = state.read().await;
        s.tidal_tokens
            .clone()
            .or(persisted_tokens)
            .ok_or_else(|| anyhow::anyhow!("No TIDAL tokens available for auto-sync"))?
    };

    let client = TidalClient::new(tokens.access_token.clone(), tokens.country_code.clone());
    
    // Run sync
    let stats = run_tidal_sync_with_reauth(&client, state, tokens).await?;
    
    // Record sync timestamp
    state.read().await.db.with_conn(|conn| {
        queries::update_sync_timestamp(conn, "tidal", stats.tracks as i64, stats.albums as i64)
    })?;
    
    // Broadcast event
    let s = state.read().await;
    let _ = s.event_tx.send(AppEvent::LibrarySynced);
    
    Ok(stats)
}

/// Sync TIDAL library into local database.
async fn tidal_sync_library(
    State(state): State<SharedState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    // Get tokens and http client
    let persisted_tokens = load_persisted_tidal_tokens(&state).await.map_err(|error| {
        TidalSyncStartError::SessionCheckFailed(error.to_string()).into_response()
    })?;
    let (tokens, _http) = {
        let s = state.read().await;
        let tokens = s
            .tidal_tokens
            .clone()
            .or(persisted_tokens)
            .ok_or_else(|| TidalSyncStartError::NotConnected)?;
        (tokens, s.http_client.clone())
    };

    // Create TIDAL client
    let client = TidalClient::new(tokens.access_token.clone(), tokens.country_code.clone());

    let (session, session_state) = ensure_tidal_session(&state, &tokens, &client)
        .await
        .map_err(|error| error.into_response())?;

    // Run sync in background
    let state_clone = state.clone();
    let sync_tokens = session.clone();
    tokio::spawn(async move {
        tracing::info!(
            target: "noor.sync.tidal",
            event = "background_start",
            session_state = session_state.as_str(),
            user_id = %sync_tokens.user_id,
            "TIDAL sync background task started"
        );
        let client = TidalClient::new(
            sync_tokens.access_token.clone(),
            sync_tokens.country_code.clone(),
        );
        match run_tidal_sync_with_reauth(&client, &state_clone, sync_tokens).await {
            Ok(stats) => {
                tracing::info!(
                    target: "noor.sync.tidal",
                    event = "sync_complete",
                    artists = stats.artists,
                    albums = stats.albums,
                    tracks = stats.tracks,
                    playlists = stats.playlists,
                    "TIDAL sync complete"
                );
                // Record sync timestamp in DB
                if let Err(e) = state_clone.read().await.db.with_conn(|conn| {
                    queries::update_sync_timestamp(conn, "tidal", stats.tracks as i64, stats.albums as i64)
                }) {
                    tracing::warn!("Failed to record sync timestamp: {}", e);
                }
                let s = state_clone.read().await;
                let _ = s.event_tx.send(AppEvent::LibrarySynced);
            }
            Err(e) => {
                tracing::error!(
                    target: "noor.sync.tidal",
                    event = "sync_failure",
                    user_id = %session.user_id,
                    error = %e,
                    "TIDAL sync failed"
                );
            }
        }
    });

    let mut response = json!({
        "status": "sync_started",
    });
    if matches!(session_state, TidalSyncSessionState::Recovered) {
        response["session_state"] = json!("recovered");
    }

    Ok(Json(response))
}

/// Perform the actual TIDAL sync (runs in background task).
async fn do_tidal_sync(
    client: &TidalClient,
    state: &SharedState,
    user_id: &str,
) -> anyhow::Result<SyncStats> {
    use crate::services::tidal::client::TidalClient as TC;
    use futures::stream::{self, StreamExt};
    let mut stats = SyncStats::default();
    let mut favorite_album_ids = HashSet::new();
    let mut favorite_track_ids = HashSet::new();

    // ── Sync favorite artists ────────────────────────
    tracing::info!("Syncing TIDAL artists...");
    let mut offset = 0;
    loop {
        let resp = client.get_favorite_artists(user_id, 100, offset).await?;
        if resp.items.is_empty() {
            break;
        }
        {
            let s = state.read().await;
            s.db.with_conn(|conn| {
                for fav in &resp.items {
                    let a = &fav.item;
                    let photo = a.picture.as_ref().map(|p| {
                        let path = p.replace('-', "/");
                        format!("https://resources.tidal.com/images/{}/480x480.jpg", path)
                    });
                    conn.execute(
                        "INSERT INTO artists (tidal_id, name, photo_url) VALUES (?1, ?2, ?3)
                         ON CONFLICT(tidal_id) DO UPDATE SET name=excluded.name, photo_url=COALESCE(excluded.photo_url, artists.photo_url)",
                        rusqlite::params![a.id, a.name, photo],
                    )?;
                    stats.artists += 1;
                }
                Ok(())
            })?;
        }
        offset += resp.items.len() as i32;
        if resp
            .total_number_of_items
            .map_or(true, |t| offset as i64 >= t)
        {
            break;
        }
    }
    tracing::info!("Synced {} artists", stats.artists);

    // ── Sync favorite albums ─────────────────────────
    tracing::info!("Syncing TIDAL albums...");
    offset = 0;
    loop {
        let resp = client.get_favorite_albums(user_id, 100, offset).await?;
        if resp.items.is_empty() {
            break;
        }
        for fav in &resp.items {
            let album = &fav.item;
            let artwork = TC::get_artwork_url(&album.cover, 640);
            let year: Option<i32> = album
                .release_date
                .as_ref()
                .and_then(|d| d.split('-').next())
                .and_then(|y| y.parse().ok());

            {
                let s = state.read().await;
                s.db.with_conn(|conn| {
                    // Ensure artist exists
                    let photo = album.artist.picture.as_ref().map(|p| {
                        let path = p.replace('-', "/");
                        format!("https://resources.tidal.com/images/{}/480x480.jpg", path)
                    });
                    conn.execute(
                        "INSERT INTO artists (tidal_id, name, photo_url) VALUES (?1, ?2, ?3)
                         ON CONFLICT(tidal_id) DO UPDATE SET name=excluded.name, photo_url=COALESCE(excluded.photo_url, artists.photo_url)",
                        rusqlite::params![album.artist.id, album.artist.name, photo],
                    )?;

                    // Insert album
                    conn.execute(
                        "INSERT INTO albums (tidal_id, title, artist_id, year, artwork_url, release_type, track_count, is_favorite, source)
                         VALUES (?1, ?2, (SELECT id FROM artists WHERE tidal_id=?3), ?4, ?5, ?6, ?7, 1, 'tidal')
                         ON CONFLICT(tidal_id) DO UPDATE SET title=excluded.title, year=COALESCE(excluded.year, albums.year),
                         artwork_url=COALESCE(excluded.artwork_url, albums.artwork_url), track_count=COALESCE(excluded.track_count, albums.track_count),
                         is_favorite=1",
                        rusqlite::params![album.id, album.title, album.artist.id, year, artwork, album.release_type, album.number_of_tracks],
                    )?;
                    Ok(())
                })?;
            }
            stats.albums += 1;
            favorite_album_ids.insert(album.id);
        }

        // Hydrate album tracks with bounded concurrency so the UI keeps moving
        // instead of stalling on one giant page-wide batch.
        let album_ids: Vec<i64> = resp.items.iter().map(|f| f.item.id).collect();
        let album_total = resp
            .total_number_of_items
            .unwrap_or((offset + resp.items.len() as i32) as i64)
            .max(1) as f32;
        let mut albums_hydrated_in_page = 0usize;

        for album_chunk in album_ids.chunks(10) {
            let mut fetches = stream::iter(album_chunk.iter().copied())
                .map(|album_id| async move { client.get_album_tracks(album_id).await })
                .buffer_unordered(10);

            while let Some(result) = fetches.next().await {
                if let Ok(tracks_resp) = result {
                    let s = state.read().await;
                    s.db.with_conn(|conn| {
                        for track in &tracks_resp.items {
                            insert_tidal_track(conn, track, false)?;
                            stats.tracks += 1;
                        }
                        Ok(())
                    })?;
                }

                albums_hydrated_in_page += 1;
                let processed_albums = offset as usize + albums_hydrated_in_page;
                let progress_fraction =
                    ((processed_albums as f32 / album_total) * 0.5).clamp(0.0, 0.5);
                send_tidal_sync_progress(state, progress_fraction).await;
            }
        }

        offset += resp.items.len() as i32;
        if resp
            .total_number_of_items
            .map_or(true, |t| offset as i64 >= t)
        {
            break;
        }
    }
    tracing::info!(
        "Synced {} albums, {} tracks so far",
        stats.albums,
        stats.tracks
    );

    // ── Sync favorite tracks ─────────────────────────
    tracing::info!("Syncing TIDAL favorite tracks...");
    offset = 0;
    loop {
        let resp = client.get_favorite_tracks(user_id, 100, offset).await?;
        if resp.items.is_empty() {
            break;
        }
        {
            let s = state.read().await;
            s.db.with_conn(|conn| {
                for fav in &resp.items {
                    let track = &fav.item;
                    favorite_track_ids.insert(track.id);
                    // Ensure artist
                    conn.execute(
                        "INSERT INTO artists (tidal_id, name) VALUES (?1, ?2) ON CONFLICT(tidal_id) DO UPDATE SET name=excluded.name",
                        rusqlite::params![track.artist.id, track.artist.name],
                    )?;
                    // Ensure album ref
                    if let Some(ref album_ref) = track.album {
                        let artwork = TC::get_artwork_url(&album_ref.cover, 640);
                        conn.execute(
                            "INSERT OR IGNORE INTO albums (tidal_id, title, artist_id, artwork_url, is_favorite, source)
                             VALUES (?1, ?2, (SELECT id FROM artists WHERE tidal_id=?3), ?4, 0, 'tidal')",
                            rusqlite::params![album_ref.id, album_ref.title, track.artist.id, artwork],
                        )?;
                    }
                    insert_tidal_track(conn, track, true)?;
                    stats.tracks += 1;
                }
                Ok(())
            })?;
        }
        offset += resp.items.len() as i32;
        let processed_tracks = offset as f32;
        let track_progress = resp
            .total_number_of_items
            .map(|t| 0.5 + (processed_tracks / t.max(1) as f32) * 0.4)
            .unwrap_or(0.85)
            .clamp(0.5, 0.9);
        send_tidal_sync_progress(state, track_progress).await;
        if resp
            .total_number_of_items
            .map_or(true, |t| offset as i64 >= t)
        {
            break;
        }
    }
    tracing::info!("Synced {} tracks total", stats.tracks);

    // ── Sync playlists ───────────────────────────────
    tracing::info!("Syncing TIDAL playlists...");
    let mut playlist_offset = 0;
    let mut all_playlists: Vec<_> = vec![];
    loop {
        let resp = client.get_playlists(user_id, 100, playlist_offset).await?;
        if resp.items.is_empty() {
            break;
        }
        let fetched = resp.items.len() as i32;
        all_playlists.extend(resp.items);
        playlist_offset += fetched;
        if resp
            .total_number_of_items
            .map_or(true, |t| playlist_offset as i64 >= t)
        {
            break;
        }
    }
    let total_playlists = all_playlists.len().max(1);
    for (playlist_index, playlist) in all_playlists.iter().enumerate() {
        {
            let s = state.read().await;
            s.db.with_conn(|conn| {
                conn.execute(
                    "INSERT OR REPLACE INTO playlists (tidal_uuid, name, description, track_count)
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![
                        playlist.uuid,
                        playlist.title,
                        playlist.description,
                        playlist.number_of_tracks.unwrap_or(0)
                    ],
                )?;
                Ok(())
            })?;
        }

        // Get playlist tracks
        let playlist_id: Option<i64> = {
            let s = state.read().await;
            s.db.with_conn(|conn| {
                Ok(conn
                    .query_row(
                        "SELECT id FROM playlists WHERE tidal_uuid=?1",
                        rusqlite::params![playlist.uuid],
                        |row| row.get(0),
                    )
                    .ok())
            })?
        };

        if let Some(pid) = playlist_id {
            // Clear old tracks
            {
                let s = state.read().await;
                s.db.with_conn(|conn| {
                    conn.execute(
                        "DELETE FROM playlist_tracks WHERE playlist_id=?1",
                        rusqlite::params![pid],
                    )?;
                    Ok(())
                })?;
            }

            let mut track_offset = 0;
            let mut position = 0;
            loop {
                let tracks_resp = client
                    .get_playlist_tracks(&playlist.uuid, 100, track_offset)
                    .await?;
                if tracks_resp.items.is_empty() {
                    break;
                }
                {
                    let s = state.read().await;
                    s.db.with_conn(|conn| {
                        for track in &tracks_resp.items {
                            conn.execute(
                                "INSERT INTO artists (tidal_id, name) VALUES (?1, ?2) ON CONFLICT(tidal_id) DO UPDATE SET name=excluded.name",
                                rusqlite::params![track.artist.id, track.artist.name],
                            )?;
                            if let Some(ref album_ref) = track.album {
                                let artwork = TC::get_artwork_url(&album_ref.cover, 640);
                                conn.execute(
                                    "INSERT OR IGNORE INTO albums (tidal_id, title, artist_id, artwork_url, is_favorite, source)
                                     VALUES (?1, ?2, (SELECT id FROM artists WHERE tidal_id=?3), ?4, 0, 'tidal')",
                                    rusqlite::params![album_ref.id, album_ref.title, track.artist.id, artwork],
                                )?;
                            }
                            insert_tidal_track(conn, track, false)?;

                            let track_id: Option<i64> = conn.query_row(
                                "SELECT id FROM tracks WHERE tidal_id=?1",
                                rusqlite::params![track.id],
                                |row| row.get(0),
                            ).ok();
                            if let Some(tid) = track_id {
                                conn.execute(
                                    "INSERT OR IGNORE INTO playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
                                    rusqlite::params![pid, tid, position],
                                )?;
                                position += 1;
                            }
                        }
                        Ok(())
                    })?;
                }
                track_offset += tracks_resp.items.len() as i32;
                if tracks_resp
                    .total_number_of_items
                    .map_or(true, |t| track_offset as i64 >= t)
                {
                    break;
                }
            }
        }
        stats.playlists += 1;
        let playlist_progress =
            0.9 + (((playlist_index + 1) as f32 / total_playlists as f32) * 0.1);
        send_tidal_sync_progress(state, playlist_progress.clamp(0.9, 0.99)).await;
    }
    tracing::info!("Synced {} playlists", stats.playlists);

    {
        let s = state.read().await;
        s.db.with_conn(|conn| {
            apply_tidal_favorite_flags(conn, "albums", "tidal_id", &favorite_album_ids)?;
            apply_tidal_favorite_flags(conn, "tracks", "tidal_id", &favorite_track_ids)?;
            Ok(())
        })?;
    }

    Ok(stats)
}

async fn send_tidal_sync_progress(state: &SharedState, progress: f32) {
    let s = state.read().await;
    let _ = s.event_tx.send(AppEvent::SyncProgress {
        service: "tidal".to_string(),
        progress,
    });
}

async fn run_tidal_sync_with_reauth(
    client: &TidalClient,
    state: &SharedState,
    tokens: tidal_auth::TidalTokens,
) -> anyhow::Result<SyncStats> {
    match do_tidal_sync(client, state, &tokens.user_id).await {
        Ok(stats) => Ok(stats),
        Err(err) if error_looks_like_auth(&err) => {
            tracing::warn!(
                target: "noor.sync.tidal",
                event = "sync_auth_failure",
                user_id = %tokens.user_id,
                error = %err,
                "TIDAL sync hit an auth error; trying refresh-token recovery"
            );

            let http = {
                let s = state.read().await;
                s.http_client.clone()
            };

            let refreshed = match recover_tidal_session(state, &http, &tokens).await {
                Ok(tokens) => tokens,
                Err(recover_err) => {
                    // Do NOT clear the session — a transient network error during refresh
                    // should not permanently log the user out.
                    tracing::error!(
                        target: "noor.sync.tidal",
                        event = "sync_recovery_failed",
                        user_id = %tokens.user_id,
                        error = %recover_err,
                        original_error = %err,
                        "TIDAL sync recovery failed; keeping stored tokens"
                    );
                    return Err(anyhow::anyhow!(
                        "TIDAL session recovery failed after auth error: {}; original sync error: {}",
                        recover_err,
                        err
                    ));
                }
            };
            let retry_client = TidalClient::new(
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            tracing::info!(
                target: "noor.sync.tidal",
                event = "sync_recovered",
                user_id = %refreshed.user_id,
                "TIDAL sync session recovered; retrying sync"
            );
            do_tidal_sync(&retry_client, state, &refreshed.user_id).await
        }
        Err(err) => Err(err),
    }
}

#[derive(Clone, Copy)]
enum TidalSyncSessionState {
    Valid,
    Recovered,
}

impl TidalSyncSessionState {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Recovered => "recovered",
        }
    }
}

enum TidalSyncStartError {
    NotConnected,
    SessionCheckFailed(String),
    PreflightRefreshFailed(String),
}

impl TidalSyncStartError {
    fn into_response(self) -> (StatusCode, Json<Value>) {
        match self {
            Self::NotConnected => (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "status": "not_connected",
                    "message": "Connect TIDAL in Settings before syncing."
                })),
            ),
            Self::SessionCheckFailed(message) => (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "status": "session_check_failed",
                    "message": message,
                })),
            ),
            Self::PreflightRefreshFailed(message) => (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "status": "preflight_refresh_failed",
                    "message": "TIDAL session expired or rejected. Reconnect in Settings before syncing again.",
                    "details": message,
                })),
            ),
        }
    }
}

impl From<TidalSyncStartError> for (StatusCode, Json<Value>) {
    fn from(value: TidalSyncStartError) -> Self {
        value.into_response()
    }
}

async fn ensure_tidal_session(
    state: &SharedState,
    tokens: &tidal_auth::TidalTokens,
    client: &TidalClient,
) -> std::result::Result<(tidal_auth::TidalTokens, TidalSyncSessionState), TidalSyncStartError> {
    match client.validate_session(&tokens.user_id).await {
        Ok(()) => Ok((tokens.clone(), TidalSyncSessionState::Valid)),
        Err(err) if error_looks_like_auth(&err) => {
            tracing::warn!(
                target: "noor.sync.tidal",
                event = "preflight_stale_session",
                user_id = %tokens.user_id,
                error = %err,
                "TIDAL session looks stale before sync"
            );
            let http = {
                let s = state.read().await;
                s.http_client.clone()
            };
            match recover_tidal_session(state, &http, tokens).await {
                Ok(tokens) => Ok((tokens, TidalSyncSessionState::Recovered)),
                Err(recover_err) => {
                    // Do NOT clear — a transient refresh failure should not log the user out.
                    tracing::error!(
                        target: "noor.sync.tidal",
                        event = "preflight_refresh_failed",
                        user_id = %tokens.user_id,
                        error = %recover_err,
                        original_error = %err,
                        "TIDAL preflight refresh failed; keeping stored tokens"
                    );
                    Err(TidalSyncStartError::PreflightRefreshFailed(
                        recover_err.to_string(),
                    ))
                }
            }
        }
        Err(err) => {
            tracing::error!(
                target: "noor.sync.tidal",
                event = "preflight_check_failed",
                user_id = %tokens.user_id,
                error = %err,
                "TIDAL session check failed before sync"
            );
            Err(TidalSyncStartError::SessionCheckFailed(format!(
                "TIDAL session check failed before sync: {}",
                err
            )))
        }
    }
}

async fn recover_tidal_session(
    state: &SharedState,
    http: &reqwest::Client,
    tokens: &tidal_auth::TidalTokens,
) -> anyhow::Result<tidal_auth::TidalTokens> {
    tracing::info!(
        target: "noor.sync.tidal",
        event = "session_refresh_start",
        user_id = %tokens.user_id,
        "Refreshing TIDAL session"
    );
    let mut refreshed = tidal_auth::refresh_token(http, &tokens.refresh_token).await?;
    if refreshed.user_id.is_empty() {
        refreshed.user_id = tokens.user_id.clone();
    }
    if refreshed.country_code.is_empty() {
        refreshed.country_code = tokens.country_code.clone();
    }

    persist_tidal_tokens(state, &refreshed).await?;
    let validation_client = TidalClient::new(
        refreshed.access_token.clone(),
        refreshed.country_code.clone(),
    );
    validation_client
        .validate_session(&refreshed.user_id)
        .await
        .context("Refreshed TIDAL session still failed validation")?;

    tracing::info!(
        target: "noor.sync.tidal",
        event = "session_refresh_success",
        user_id = %refreshed.user_id,
        "TIDAL session refresh succeeded"
    );

    Ok(refreshed)
}

fn error_looks_like_auth(err: &anyhow::Error) -> bool {
    let message = err.to_string().to_ascii_lowercase();
    message.contains("401")
        || message.contains("substatus\":6001")
        || message.contains("valid session")
        || message.contains("unauthorized")
}

async fn current_playback_runtime(
    state: &SharedState,
) -> Option<playback_runtime::PlaybackRuntimeHandle> {
    let state = state.read().await;
    state
        .playback_runtime
        .as_ref()
        .map(|runtime| runtime.handle.clone())
}

async fn ensure_playback_runtime_for_track(
    state: &SharedState,
    track: &crate::db::models::Track,
) -> Result<playback_runtime::PlaybackRuntimeHandle, (StatusCode, Json<Value>)> {
    let access_token = {
        let state = state.read().await;
        state
            .tidal_tokens
            .as_ref()
            .map(|tokens| tokens.access_token.clone())
    }
    .ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "status": "not_connected",
                "message": "Connect TIDAL in Settings before playing.",
                "track_id": track.id,
            })),
        )
    })?;

    let mut state_guard = state.write().await;
    let needs_respawn = state_guard
        .playback_runtime
        .as_ref()
        .map(|runtime| runtime.access_token != access_token)
        .unwrap_or(true);
    let mut spawned_handle = None;

    if needs_respawn {
        if let Some(runtime) = state_guard.playback_runtime.take() {
            let _ = runtime.handle.shutdown();
        }

        let config = playback_runtime::PlaybackRuntimeConfig::new(
            state_guard.http_client.clone(),
            access_token.clone(),
            state_guard.analysis_tx.clone(),
        );
        let handle = playback_runtime::spawn_runtime(config).map_err(|error| {
            let message = format!("Failed to start host audio runtime: {error}");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "status": "playback_runtime_unavailable",
                    "message": message,
                    "track_id": track.id,
                })),
            )
        })?;

        // Restore persisted volume to the new runtime.
        let persisted_volume = state_guard
            .db
            .with_conn(|conn| {
                let vol: f64 = conn.query_row(
                    "SELECT volume FROM playback_state WHERE id = 1",
                    [],
                    |row| row.get(0),
                )?;
                Ok(vol)
            })
            .unwrap_or(1.0);
        handle.set_volume(persisted_volume as f32);

        state_guard.playback_runtime = Some(PlaybackRuntimeState {
            access_token,
            handle: handle.clone(),
        });
        spawned_handle = Some(handle.clone());
    }

    let handle = state_guard
        .playback_runtime
        .as_ref()
        .map(|runtime| runtime.handle.clone())
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "status": "playback_runtime_unavailable",
                    "message": "Playback runtime was not available after initialization.",
                    "track_id": track.id,
                })),
            )
        })?;
    drop(state_guard);

    if let Some(listener_handle) = spawned_handle {
        spawn_playback_runtime_listener(state.clone(), listener_handle);
    }

    Ok(handle)
}

fn spawn_playback_runtime_listener(
    state: SharedState,
    handle: playback_runtime::PlaybackRuntimeHandle,
) {
    tokio::spawn(async move {
        let mut rx = handle.subscribe();

        loop {
            match rx.recv().await {
                Ok(playback_runtime::PlaybackRuntimeEvent::Finished { track_id }) => {
                    // Track is no longer producing audio — clear the flag before advancing.
                    state.write().await.audio_active.store(false, std::sync::atomic::Ordering::Relaxed);
                    if let Err(error) = handle_runtime_finished(state.clone(), track_id).await {
                        let message =
                            format!("Failed to advance playback after track end: {error}");
                        report_playback_failure(&state, &message);
                        error!("{message}");
                    }
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::Error { message }) => {
                    handle_runtime_error(state.clone(), &message).await;
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::Ready {
                    device_name,
                    sample_rate,
                    channels,
                }) => {
                    let mut state_guard = state.write().await;
                    state_guard.audio_active.store(false, std::sync::atomic::Ordering::Relaxed);
                    let last_error = state_guard
                        .playback_runtime_info
                        .as_ref()
                        .and_then(|info| info.last_error.clone());
                    state_guard.playback_runtime_info = Some(PlaybackRuntimeInfo {
                        device_name,
                        sample_rate,
                        channels,
                        active_track_id: None,
                        last_error,
                    });
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::Started { track_id, .. }) => {
                    let mut state_guard = state.write().await;
                    // CPAL buffer threshold crossed — samples are actually flowing now.
                    state_guard.audio_active.store(true, std::sync::atomic::Ordering::Relaxed);
                    if let Some(info) = state_guard.playback_runtime_info.as_mut() {
                        info.active_track_id = Some(track_id);
                        info.last_error = None;
                    }
                    if let Some(pending) = state_guard.pending_stream_display.take() {
                        state_guard.current_stream_display = Some(pending);
                    }
                    drop(state_guard);
                    let state_guard = state.read().await;
                    let _ = state_guard
                        .event_tx
                        .send(AppEvent::TrackChanged { track_id });
                    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::Paused { .. })
                | Ok(playback_runtime::PlaybackRuntimeEvent::Resumed { .. })
                | Ok(playback_runtime::PlaybackRuntimeEvent::Preparing { .. }) => {}
                Ok(playback_runtime::PlaybackRuntimeEvent::Stopped) => {
                    let mut state_guard = state.write().await;
                    state_guard.audio_active.store(false, std::sync::atomic::Ordering::Relaxed);
                    if let Some(info) = state_guard.playback_runtime_info.as_mut() {
                        info.active_track_id = None;
                    }
                    state_guard.external_playback_track = None;
                    state_guard.current_stream_display = None;
                    state_guard.pending_stream_display = None;
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::NearEnd { track_id }) => {
                    // Pre-decode the next track so the transition is gapless.
                    if let Err(err) = handle_near_end(state.clone(), track_id).await {
                        warn!("Failed to pre-buffer next track: {err:?}");
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!("Playback runtime listener lagged by {skipped} events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Peek at what would play next without advancing the queue, then send `PrepareNext` to the
/// runtime so it can pre-decode the track and swap it in gaplessly when the current one ends.
async fn handle_near_end(state: SharedState, current_track_id: i64) -> anyhow::Result<()> {
    let (next_track, runtime_handle, crossfade_ms) = {
        let state_guard = state.read().await;

        // Guard: only proceed if the current track is still the one that fired NearEnd.
        let active_id = state_guard
            .playback_runtime_info
            .as_ref()
            .and_then(|info| info.active_track_id);
        if active_id != Some(current_track_id) {
            return Ok(());
        }

        // Skip pre-decode in exclusive mode — only one stream can grab the
        // device exclusively, and a paused pre-buffer engine would force-share
        // the device which the OS rejects with AUDCLNT_E_DEVICE_IN_USE.
        // The next track will cold-start when the current one finishes.
        let exclusive = state_guard
            .db
            .with_conn(|conn| crate::db::audio_settings::load(conn).map_err(anyhow::Error::from))
            .map(|s| s.exclusive_mode)
            .unwrap_or(false);
        if exclusive {
            return Ok(());
        }

        let next = state_guard.db.with_conn(player::peek_next_track)?;
        let handle = state_guard
            .playback_runtime
            .as_ref()
            .map(|r| r.handle.clone());
        let crossfade = state_guard.db.with_conn(|conn| {
            conn.query_row(
                "SELECT crossfade_ms FROM playback_state WHERE id = 1",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map_err(anyhow::Error::from)
        })?;

        (next, handle, crossfade)
    };

    let (Some(next), Some(handle)) = (next_track, runtime_handle) else {
        return Ok(());
    };

    // Resolve the stream URL for the next track (we need a live access token).
    let user_quality = current_user_audio_quality(&state).await;
    let stream_request = match player::build_tidal_stream_request(&next, user_quality.clone()) {
        Some(req) => req,
        None => return Ok(()), // local library — skip pre-buffer for now
    };

    let (stream_info, access_token) = {
        let state_guard = state.read().await;
        let token = state_guard
            .tidal_tokens
            .as_ref()
            .map(|t| t.access_token.clone())
            .unwrap_or_default();
        let http = state_guard.http_client.clone();
        drop(state_guard);
        let info = crate::services::tidal::stream::resolve_stream(&http, &token, &stream_request)
            .await
            .ok();
        (info, token)
    };

    // If sample_rate_follow is enabled and the next track's rate differs from current,
    // rebuild the output device at the new rate before PrepareNext.
    {
        let state_guard = state.read().await;
        if let (Some(stream), Some(info)) = (stream_info.as_ref(), &state_guard.playback_runtime_info) {
            let audio_settings = state_guard
                .db
                .with_conn(|conn| crate::db::audio_settings::load(conn).map_err(anyhow::Error::from))
                .ok();
            if let Some(settings) = audio_settings {
                if settings.sample_rate_follow {
                    if let Some(next_rate) = stream.sample_rate {
                        let current_rate = info.sample_rate;
                        if next_rate as u32 != current_rate {
                            let device_sel = match settings.output_device {
                                Some(device_id) => {
                                    playback_runtime::OutputDeviceSelection::Named(device_id)
                                }
                                None => playback_runtime::OutputDeviceSelection::Default,
                            };
                            // StreamInfo.sample_rate is Option<i32>; cast is safe (fits in u32).
                            if let Err(e) = handle.device_swap(
                                device_sel,
                                settings.exclusive_mode,
                                settings.sample_rate_follow,
                                Some(next_rate as u32),
                            ) {
                                warn!(
                                    "Failed to rebuild stream for next track {} at {} Hz: {e}",
                                    next.id, next_rate
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    let _gapless = crate::playback::gapless::plan_from_stream(
        stream_info.as_ref(),
        crate::playback::gapless::GaplessSettings::new(true, crossfade_ms),
    );
    let job = player::build_playback_preparation(
        &next,
        stream_info.as_ref(),
        crossfade_ms,
        user_quality,
    );
    let _ = access_token; // already embedded in the config held by the runtime

    let _ = handle.prepare_next(job);
    if let Some(ref si) = stream_info {
        let mut state_guard = state.write().await;
        state_guard.pending_stream_display = Some(crate::StreamDisplayInfo {
            audio_quality: si.audio_quality.clone(),
            sample_rate: si.sample_rate,
            bit_depth: si.bit_depth,
        });
    }
    info!("Pre-buffering next track: {} (id {})", next.title, next.id);
    Ok(())
}

async fn handle_runtime_finished(state: SharedState, finished_track_id: i64) -> anyhow::Result<()> {
    let external_finished = {
        let state_guard = state.read().await;
        state_guard
            .external_playback_track
            .as_ref()
            .map(|track| track.id == finished_track_id)
            .unwrap_or(false)
    };
    if external_finished {
        {
            let mut state_guard = state.write().await;
            state_guard.external_playback_track = None;
            if let Some(info) = state_guard.playback_runtime_info.as_mut() {
                info.active_track_id = None;
            }
            let _ = state_guard.db.with_conn(|conn| {
                conn.execute(
                    "UPDATE playback_state
                     SET current_track_id = NULL, position_ms = 0, is_playing = 0
                     WHERE id = 1",
                    [],
                )?;
                Ok(())
            });
        }
        let state_guard = state.read().await;
        let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
        return Ok(());
    }

    let snapshot = {
        let state_guard = state.read().await;
        state_guard.db.with_conn(|conn| {
            let current_track_id = player::current_track_id(conn)?;
            let current_state = player::load_state(conn)?;
            if current_track_id != Some(finished_track_id) || !current_state.is_playing {
                return Ok(None);
            }

            let snapshot = player::next_track(conn)?;
            Ok(Some(snapshot))
        })?
    };

    let Some(snapshot) = snapshot else {
        return Ok(());
    };

    let end_reason = if snapshot.state.current_track.is_some() {
        Some(player::ListenSessionEndReason::Replaced)
    } else {
        Some(player::ListenSessionEndReason::QueueEnded)
    };
    sync_session_after_snapshot(&state, &snapshot, end_reason).await;

    {
        let mut state_guard = state.write().await;
        if let Some(info) = state_guard.playback_runtime_info.as_mut() {
            info.active_track_id = snapshot.state.current_track.as_ref().map(|track| track.id);
        }
    }

    if let Some(track) = snapshot.state.current_track.as_ref() {
        let user_quality = current_user_audio_quality(&state).await;
        let Some(stream_request) = player::build_tidal_stream_request(track, user_quality.clone())
        else {
            handle_runtime_error(
                state.clone(),
                "Local library playback is not wired into the host audio runtime yet.",
            )
            .await;
            return Ok(());
        };
        let stream_info = resolve_tidal_playback_stream(&state, track, &stream_request)
            .await
            .map_err(|error| {
                anyhow::anyhow!(
                    "playback stream resolve failed: {}",
                    describe_tidal_playback_error(&error)
                )
            })?;
        let runtime_handle = ensure_playback_runtime_for_track(&state, track)
            .await
            .map_err(|(status, body)| {
                anyhow::anyhow!("playback runtime unavailable ({status}): {}", body.0)
            })?;
        let job = player::build_playback_preparation(
            track,
            Some(&stream_info),
            effective_crossfade_ms(&state, snapshot.state.crossfade_ms).await,
            user_quality,
        );
        runtime_handle.switch_to(job)?;
        {
            let mut state_guard = state.write().await;
            state_guard.current_stream_display = Some(crate::StreamDisplayInfo {
                audio_quality: stream_info.audio_quality.clone(),
                sample_rate: stream_info.sample_rate,
                bit_depth: stream_info.bit_depth,
            });
            state_guard.pending_stream_display = None;
        }
    }

    let state_guard = state.read().await;
    if let Some(track) = snapshot.state.current_track.as_ref() {
        let _ = state_guard
            .event_tx
            .send(AppEvent::TrackChanged { track_id: track.id });
    }
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
    Ok(())
}

async fn handle_runtime_error(state: SharedState, message: &str) {
    {
        let mut state_guard = state.write().await;
        if let Some(info) = state_guard.playback_runtime_info.as_mut() {
            info.last_error = Some(message.to_string());
            info.active_track_id = None;
        }
        state_guard.external_playback_track = None;
    }
    report_playback_failure(&state, message);

    let snapshot = {
        let state_guard = state.read().await;
        state_guard.db.with_conn(player::pause).ok()
    };

    if let Some(snapshot) = snapshot {
        sync_session_after_snapshot(
            &state,
            &snapshot,
            Some(player::ListenSessionEndReason::Stopped),
        )
        .await;
    }

    let state_guard = state.read().await;
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
}

fn describe_tidal_playback_error(error: &TidalPlaybackError) -> String {
    match error {
        TidalPlaybackError::NotConnected => "TIDAL is not connected.".to_string(),
        TidalPlaybackError::SessionRefreshFailed(message) => message.clone(),
        TidalPlaybackError::StreamResolve(error) => error.to_string(),
    }
}

fn report_playback_failure(state: &SharedState, message: &str) {
    let state = state.clone();
    let message = message.to_string();
    tokio::spawn(async move {
        let state = state.read().await;
        let _ = state.event_tx.send(AppEvent::PlaybackFailed { message });
    });
}

async fn pause_active_session(state: &SharedState) {
    let mut state = state.write().await;
    if let Some(session) = state.active_listen_session.as_mut() {
        session.pause(chrono::Utc::now());
    }
}

async fn resume_session_after_snapshot(state: &SharedState, snapshot: &player::PlaybackSnapshot) {
    let Some(track) = snapshot.state.current_track.as_ref() else {
        return;
    };

    let mut state = state.write().await;
    let now = chrono::Utc::now();
    match state.active_listen_session.as_mut() {
        Some(session) if session.track_id == track.id => session.resume(now),
        _ => {
            state.active_listen_session = Some(player::ActiveListenSession::start(track.id, now));
        }
    }
}

/// Read the user's configured crossfade length from `playback_state`.
///
/// Every code path that calls `player::build_playback_preparation` to start a
/// track on the host audio runtime must source `crossfade_ms` through this
/// helper (or `effective_crossfade_ms` to apply the exclusive-mode override).
/// Passing a hardcoded 0 disables the per-engine fade-out ramp AND prevents
/// `CrossfadeStart` from firing, which silently breaks both gapless and
/// crossfade transitions.
/// Returns `configured` unless exclusive mode is on, in which case it returns 0.
/// Used by callsites that already have a snapshot's `crossfade_ms` and want the
/// same exclusive-mode override that `current_crossfade_ms` applies, without
/// re-querying `playback_state`.
async fn effective_crossfade_ms(state: &SharedState, configured: i32) -> i32 {
    let guard = state.read().await;
    let exclusive = guard
        .db
        .with_conn(|conn| crate::db::audio_settings::load(conn).map_err(Into::into))
        .map(|s| s.exclusive_mode)
        .unwrap_or(false);
    if exclusive { 0 } else { configured }
}

async fn current_crossfade_ms(state: &SharedState) -> i32 {
    let guard = state.read().await;
    let configured = guard
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT crossfade_ms FROM playback_state WHERE id = 1",
                [],
                |row| row.get::<_, i32>(0),
            )
            .map_err(Into::into)
        })
        .unwrap_or(0);

    // Exclusive mode owns the device with a single stream — crossfade
    // requires two simultaneous streams, which the OS rejects with
    // AUDCLNT_E_DEVICE_IN_USE. Force 0 here so the fade-out ramp on the
    // outgoing track also goes away (otherwise we'd attenuate the last
    // few seconds of every track for no reason).
    let exclusive = guard
        .db
        .with_conn(|conn| crate::db::audio_settings::load(conn).map_err(Into::into))
        .map(|s| s.exclusive_mode)
        .unwrap_or(false);
    if exclusive { 0 } else { configured }
}

async fn current_user_audio_quality(
    state: &SharedState,
) -> Option<crate::db::audio_settings::AudioQuality> {
    let guard = state.read().await;
    guard
        .db
        .with_conn(|conn| crate::db::audio_settings::load(conn).map_err(Into::into))
        .ok()
        .map(|s| s.quality)
}

// ───── Audio output settings ────────────────────────────────────────────────
//
// `GET /api/audio/devices`     — enumerate cpal output devices
// `GET /api/audio/settings`    — current persisted AudioSettings
// `PUT /api/audio/settings`    — persist + (if device/exclusive/SR-follow changed) live-swap

async fn get_audio_devices(
    State(_state): State<SharedState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let devices = crate::playback::runtime::enumerate_output_devices();
    Ok(Json(serde_json::json!({ "devices": devices })))
}

async fn get_audio_settings(
    State(state): State<SharedState>,
) -> Result<Json<crate::db::audio_settings::AudioSettings>, StatusCode> {
    let guard = state.read().await;
    guard
        .db
        .with_conn(|conn| crate::db::audio_settings::load(conn).map_err(Into::into))
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// PUT body is the full `AudioSettings` struct. The frontend always knows the
/// complete current state (the store hydrates on mount), so it sends the whole
/// thing on every change. This avoids the partial-update / `Option<Option<T>>`
/// footgun.
async fn put_audio_settings(
    State(state): State<SharedState>,
    Json(new): Json<crate::db::audio_settings::AudioSettings>,
) -> Result<Json<crate::db::audio_settings::AudioSettings>, (StatusCode, Json<serde_json::Value>)> {
    // Reject exclusive_mode on non-Windows.
    if new.exclusive_mode && !cfg!(target_os = "windows") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "message": "exclusive_mode is only supported on Windows"
            })),
        ));
    }

    let (old, new) = {
        let guard = state.read().await;
        let (old, saved) = guard
            .db
            .with_conn(|conn| {
                let old = crate::db::audio_settings::load(conn)?;
                crate::db::audio_settings::save(conn, &new)?;
                Ok((old, new.clone()))
            })
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "message": e.to_string() })),
                )
            })?;

        // Live-apply iff anything affecting the output stream changed.
        let needs_swap = old.output_device != saved.output_device
            || old.exclusive_mode != saved.exclusive_mode
            || old.sample_rate_follow != saved.sample_rate_follow;

        if needs_swap {
            if let Some(runtime) = guard.playback_runtime.as_ref() {
                if let Err(e) = runtime.handle.device_swap(
                    playback_runtime::OutputDeviceSelection::from_pref(
                        saved.output_device.as_deref(),
                    ),
                    saved.exclusive_mode,
                    saved.sample_rate_follow,
                    None,
                ) {
                    warn!("Audio settings update: live device_swap failed: {e}");
                }
            }
        }

        (old, saved)
    };

    // Quality changed → re-issue the current track at the new quality so the
    // user immediately hears (and sees) the new tier. The track restarts from
    // 0; preserving position would require partial-stream offset support that
    // TIDAL's playbackinfo API doesn't expose.
    if old.quality != new.quality {
        if let Err(e) = reissue_current_track_at_new_quality(&state).await {
            warn!("Audio settings update: re-issue at new quality failed: {e}");
        }
    }

    Ok(Json(new))
}

/// Re-resolve the currently-playing track at the user's current quality and
/// switch the runtime to it. Called after `put_audio_settings` when the user
/// flips the quality dropdown — without this, quality changes don't take effect
/// until the next track and the user can't tell the setting did anything.
async fn reissue_current_track_at_new_quality(state: &SharedState) -> anyhow::Result<()> {
    let Some(track_id) = current_playback_track_id(state).await else {
        return Ok(());
    };

    let track = {
        let guard = state.read().await;
        guard
            .db
            .with_conn(|conn| queue::get_track_by_id(conn, track_id))?
    };
    let Some(track) = track else {
        return Ok(());
    };

    let user_quality = current_user_audio_quality(state).await;
    let Some(stream_request) = player::build_tidal_stream_request(&track, user_quality.clone())
    else {
        // Local-library tracks don't have a TIDAL stream to re-resolve.
        return Ok(());
    };

    let stream_info = match resolve_tidal_playback_stream(state, &track, &stream_request).await {
        Ok(info) => info,
        Err(e) => {
            return Err(anyhow::anyhow!(
                "stream resolve failed: {}",
                describe_tidal_playback_error(&e)
            ));
        }
    };

    let runtime_handle = {
        let guard = state.read().await;
        guard.playback_runtime.as_ref().map(|r| r.handle.clone())
    };
    let Some(handle) = runtime_handle else {
        return Ok(());
    };

    let crossfade_ms = current_crossfade_ms(state).await;
    let job =
        player::build_playback_preparation(&track, Some(&stream_info), crossfade_ms, user_quality);

    handle.switch_to(job)?;

    {
        let mut state_guard = state.write().await;
        state_guard.current_stream_display = Some(crate::StreamDisplayInfo {
            audio_quality: stream_info.audio_quality.clone(),
            sample_rate: stream_info.sample_rate,
            bit_depth: stream_info.bit_depth,
        });
        state_guard.pending_stream_display = None;
    }

    Ok(())
}

async fn current_playback_track_id(state: &SharedState) -> Option<i64> {
    let guard = state.read().await;
    guard
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT current_track_id FROM playback_state WHERE id = 1",
                [],
                |row| row.get::<_, Option<i64>>(0),
            )
            .map_err(Into::into)
        })
        .ok()
        .flatten()
}

async fn record_transition_if_changed(
    state: &SharedState,
    previous_track_id: Option<i64>,
    snapshot: &player::PlaybackSnapshot,
    transition_source: &str,
    completed_prev: bool,
) {
    let Some(from_track_id) = previous_track_id else {
        return;
    };
    let Some(to_track_id) = snapshot.state.current_track.as_ref().map(|track| track.id) else {
        return;
    };
    if to_track_id <= 0 {
        return;
    }
    if from_track_id == to_track_id {
        return;
    }
    // If the caller says "queue", check whether the incoming track was actually
    // placed there by the automix engine so the corpus gets the right source label.
    let effective_source = if transition_source == "queue" {
        snapshot
            .queue
            .iter()
            .find(|item| item.track.id == to_track_id)
            .map(|item| {
                if item.source.starts_with("automix") {
                    "automix"
                } else {
                    transition_source
                }
            })
            .unwrap_or(transition_source)
    } else {
        transition_source
    };
    let guard = state.read().await;
    let _ = guard.db.with_conn(|conn| {
        queries::record_playback_transition(
            conn,
            from_track_id,
            to_track_id,
            effective_source,
            completed_prev,
            snapshot.state.crossfade_ms as i64,
        )
    });
}

async fn sync_session_after_snapshot(
    state: &SharedState,
    snapshot: &player::PlaybackSnapshot,
    end_reason: Option<player::ListenSessionEndReason>,
) {
    let flushed_track_id = {
        let mut state = state.write().await;
        let now = chrono::Utc::now();

        let flushed_track_id = if let Some(reason) = end_reason {
            flush_active_listen_session_locked(&mut state, now, reason)
                .map_err(|err| {
                    error!("failed to flush listen session: {err}");
                })
                .ok()
                .flatten()
        } else {
            None
        };

        if snapshot.state.is_playing {
            if let Some(track) = snapshot.state.current_track.as_ref() {
                state.active_listen_session =
                    Some(player::ActiveListenSession::start(track.id, now));
            }
        } else if snapshot.state.current_track.is_none() {
            state.active_listen_session = None;
        }

        flushed_track_id
    };

    if let Some(track_id) = flushed_track_id {
        let state_guard = state.read().await;
        let _ = state_guard
            .event_tx
            .send(AppEvent::ListenHistoryUpdated { track_id });
    }
}

fn flush_active_listen_session_locked(
    state: &mut crate::AppState,
    now: chrono::DateTime<chrono::Utc>,
    _reason: player::ListenSessionEndReason,
) -> anyhow::Result<Option<i64>> {
    let Some(mut session) = state.active_listen_session.take() else {
        return Ok(None);
    };

    session.pause(now);
    let listened_ms = session.listened_ms_at(now);
    // Skip sessions shorter than 5 seconds to avoid spurious near-zero entries
    // from rapid track changes or accidental clicks.
    if listened_ms < 5_000 {
        return Ok(None);
    }

    let started_at = session.started_at.to_rfc3339();
    let track_id = session.track_id;
    if track_id <= 0 {
        return Ok(None);
    }
    let completed = state.db.with_conn(|conn| {
        let track = queue::get_track_by_id(conn, track_id)?.ok_or_else(|| {
            anyhow::anyhow!("track {} missing when flushing listen session", track_id)
        })?;
        let completed = player::is_completed_listen(&track, listened_ms);
        queries::record_listen_history(conn, track_id, &started_at, listened_ms, completed)?;
        queries::increment_track_play_summary(conn, track_id, &started_at, completed)?;
        Ok(completed)
    })?;

    tracing::info!(
        target: "noor.playback.history",
        track_id,
        listened_ms,
        completed,
        "flushed listen session"
    );

    Ok(Some(track_id))
}

async fn clear_tidal_session(state: &SharedState) -> anyhow::Result<()> {
    let mut s = state.write().await;
    if let Some(runtime) = s.playback_runtime.take() {
        let _ = runtime.handle.shutdown();
    }
    s.playback_runtime_info = None;
    s.active_listen_session = None;
    s.external_playback_track = None;
    s.tidal_tokens = None;
    let _ = s.db.with_conn(|conn| {
        conn.execute("DELETE FROM service_auth WHERE service='tidal'", [])?;
        Ok(())
    });
    Ok(())
}

async fn persist_tidal_tokens(
    state: &SharedState,
    tokens: &tidal_auth::TidalTokens,
) -> anyhow::Result<()> {
    {
        let s = state.read().await;
        s.db.with_conn(|conn| {
            let token_json = serde_json::to_string(tokens)?;
            conn.execute(
                "INSERT INTO service_auth (service, access_token_enc, user_id, connected_at)
                 VALUES ('tidal', ?1, ?2, datetime('now'))
                 ON CONFLICT(service) DO UPDATE SET access_token_enc=excluded.access_token_enc,
                 user_id=excluded.user_id, connected_at=excluded.connected_at",
                rusqlite::params![token_json.as_bytes(), tokens.user_id],
            )?;
            Ok(())
        })?;
    }

    let mut s = state.write().await;
    s.tidal_tokens = Some(tokens.clone());
    Ok(())
}

fn insert_tidal_track(
    conn: &rusqlite::Connection,
    track: &crate::services::tidal::client::TidalTrack,
    is_favorite: bool,
) -> anyhow::Result<()> {
    // Ensure artist exists first (tracks.artist_id is NOT NULL)
    conn.execute(
        "INSERT INTO artists (tidal_id, name) VALUES (?1, ?2)
         ON CONFLICT(tidal_id) DO UPDATE SET name=excluded.name",
        rusqlite::params![track.artist.id, track.artist.name],
    )?;

    let quality = track.audio_quality.as_deref().unwrap_or("LOSSLESS");
    let fidelity = match quality {
        "HI_RES_LOSSLESS" => 900,
        "HI_RES" => 800,
        "LOSSLESS" => 700,
        "HIGH" => 400,
        "LOW" => 200,
        _ => 500,
    };
    let album_tidal_id = track.album.as_ref().map(|a| a.id);

    conn.execute(
        "INSERT INTO tracks (tidal_id, title, artist_id, album_id, disc_number, track_number, duration_ms, isrc, best_quality, best_source, fidelity_score, is_favorite, source)
         VALUES (?1, ?2, (SELECT id FROM artists WHERE tidal_id=?3), (SELECT id FROM albums WHERE tidal_id=?4), ?5, ?6, ?7, ?8, ?9, 'tidal', ?10, ?11, 'tidal')
         ON CONFLICT(tidal_id) DO UPDATE SET
            title=excluded.title, best_quality=excluded.best_quality,
            fidelity_score=MAX(tracks.fidelity_score, excluded.fidelity_score),
            is_favorite=MAX(tracks.is_favorite, excluded.is_favorite)",
        rusqlite::params![
            track.id, track.title, track.artist.id, album_tidal_id,
            track.volume_number.unwrap_or(1), track.track_number,
            track.duration * 1000, track.isrc,
            quality, fidelity, is_favorite as i32,
        ],
    )?;

    let local_track_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM tracks WHERE tidal_id = ?1",
            rusqlite::params![track.id],
            |row| row.get(0),
        )
        .ok();
    if let Some(local_track_id) = local_track_id {
        let canonical_genres = infer_tidal_track_genres(track);
        queries::replace_track_source_genres(
            conn,
            local_track_id,
            &canonical_genres,
            "tidal",
            0.82,
        )?;
    }

    Ok(())
}

fn apply_tidal_favorite_flags(
    conn: &rusqlite::Connection,
    table: &str,
    id_column: &str,
    favorite_ids: &HashSet<i64>,
) -> anyhow::Result<()> {
    let reset_sql = format!("UPDATE {table} SET is_favorite = 0 WHERE {id_column} IS NOT NULL");
    conn.execute(&reset_sql, [])?;

    let mut sorted_ids: Vec<i64> = favorite_ids.iter().copied().collect();
    sorted_ids.sort_unstable();

    for chunk in sorted_ids.chunks(800) {
        let placeholders = std::iter::repeat("?")
            .take(chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql =
            format!("UPDATE {table} SET is_favorite = 1 WHERE {id_column} IN ({placeholders})");
        conn.execute(&sql, rusqlite::params_from_iter(chunk.iter()))?;
    }

    Ok(())
}

fn infer_tidal_track_genres(track: &crate::services::tidal::client::TidalTrack) -> Vec<String> {
    let mut candidates = extract_genre_candidates_from_extra(&track.extra);
    if let Some(album) = track.album.as_ref() {
        candidates.extend(extract_genre_candidates_from_extra(&album.extra));
    }

    crate::genre::builder::collect_clear_genres(candidates)
}

fn extract_genre_candidates_from_extra(
    extra: &std::collections::HashMap<String, Value>,
) -> Vec<String> {
    let mut candidates = Vec::new();

    for key in [
        "genre",
        "subGenre",
        "subgenre",
        "genres",
        "subGenres",
        "subgenres",
    ] {
        let Some(value) = extra.get(key) else {
            continue;
        };
        collect_genre_values(value, &mut candidates);
    }

    candidates
}

fn collect_genre_values(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::String(raw) => {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                output.push(trimmed.to_string());
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_genre_values(item, output);
            }
        }
        Value::Object(map) => {
            for key in ["name", "title", "genre", "subGenre", "subgenre"] {
                if let Some(inner) = map.get(key) {
                    collect_genre_values(inner, output);
                }
            }
        }
        _ => {}
    }
}

#[derive(Default)]
pub struct SyncStats {
    pub artists: usize,
    pub albums: usize,
    pub tracks: usize,
    pub playlists: usize,
}

// ─── Home Page Discovery Endpoints ───────────────────────────────────────────────

/// Get new album releases from AllMusic RSS
async fn get_home_releases(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let aggregator = state.read().await.rss_aggregator.clone();
    let releases = aggregator.get_new_releases().await;
    
    Ok(Json(json!({
        "releases": releases,
        "source": "allmusic_rss"
    })))
}

/// Get daily picks curated from user's library using learning model
async fn get_home_picks(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let state_guard = state.read().await;
    let db = &state_guard.db;
    
    // Get top tracks from listening history with variety
    let picks = db.with_conn(|conn| {
        // Fetch recent top tracks that aren't played in last 7 days (rediscovery)
        let tracks = queries::get_tracks(conn, "play_count", "desc", 20, 0, false)?;
        
        // Get tracks from different genres for variety
        let mut genre_tracks = conn.prepare(
            "SELECT t.*, g.name as genre_name
             FROM tracks t
             JOIN track_genres tg ON t.id = tg.track_id
             JOIN genres g ON tg.genre_id = g.id
             WHERE t.play_count > 0
             ORDER BY RANDOM()
             LIMIT 10"
        )?;
        
        let genre_picks: Vec<serde_json::Value> = genre_tracks.query_map([], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, i64>(0)?,
                "title": row.get::<_, String>(1)?,
                "artist_name": row.get::<_, Option<String>>(2)?,
                "album_title": row.get::<_, Option<String>>(3)?,
                "artwork_url": row.get::<_, Option<String>>(4)?,
                "duration_ms": row.get::<_, Option<i64>>(5)?,
                "play_count": row.get::<_, i64>(6)?,
                "genre": row.get::<_, String>(7)?,
            }))
        })?.filter_map(|r| r.ok()).collect();
        
        Ok((tracks, genre_picks))
    }).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
    let (top_tracks, genre_picks) = picks;
    
    Ok(Json(json!({
        "top_picks": top_tracks.iter().take(10).map(|t| serde_json::json!({
            "id": t.id,
            "title": t.title,
            "artist_name": t.artist_name,
            "album_title": t.album_title,
            "artwork_url": t.artwork_url,
            "duration_ms": t.duration_ms,
            "play_count": t.play_count,
            "reason": "Most played"
        })).collect::<Vec<_>>(),
        "genre_variety": genre_picks,
        "source": "library_curation"
    })))
}

/// Get weekly articles from AllMusic RSS
async fn get_home_articles(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let aggregator = state.read().await.rss_aggregator.clone();
    let articles = aggregator.get_articles().await;
    
    Ok(Json(json!({
        "articles": articles,
        "source": "allmusic_rss"
    })))
}

/// Get music industry news from multiple RSS sources
async fn get_home_news(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let aggregator = state.read().await.rss_aggregator.clone();
    let news = aggregator.get_news().await;
    
    Ok(Json(json!({
        "news": news,
        "sources": ["billboard", "nme", "spin", "pitchfork", "rolling_stone", "consequence", "the_guardian"],
        "source": "aggregated_rss"
    })))
}

// ── Spotify Config & Enrichment ──────────────────────────────────────────────

#[derive(Deserialize)]
struct SpotifyConfigRequest {
    client_id: String,
    client_secret: String,
}

async fn spotify_save_config(
    State(state): State<SharedState>,
    Json(payload): Json<SpotifyConfigRequest>,
) -> Result<Json<Value>, StatusCode> {
    let http = state.read().await.http_client.clone();
    let creds = spotify::auth::SpotifyCredentials {
        client_id: payload.client_id.trim().to_string(),
        client_secret: payload.client_secret.trim().to_string(),
    };

    if creds.client_id.is_empty() || creds.client_secret.is_empty() {
        return Ok(Json(json!({
            "status": "error",
            "message": "Client ID and Client Secret are both required."
        })));
    }

    // Verify the credentials work by fetching a token before saving.
    match spotify::auth::fetch_app_token(&http, &creds).await {
        Ok(tokens) => {
            let _ = state.read().await.db.with_conn(|conn| {
                spotify::auth::save_credentials(conn, &creds)?;
                Ok(())
            });
            {
                let mut s = state.write().await;
                s.spotify_tokens = Some(tokens);
            }
            Ok(Json(json!({"status": "ok"})))
        }
        Err(e) => Ok(Json(json!({
            "status": "error",
            "message": format!("Spotify rejected the credentials: {}", e)
        }))),
    }
}

async fn spotify_status(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let configured = state
        .read()
        .await
        .db
        .with_conn(|conn| {
            Ok(spotify::auth::load_credentials(conn)
                .ok()
                .flatten()
                .is_some())
        })
        .unwrap_or(false);
    Ok(Json(json!({"configured": configured})))
}

async fn spotify_clear_config(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let _ = state.read().await.db.with_conn(|conn| {
        spotify::auth::clear_credentials(conn)?;
        Ok(())
    });
    {
        let mut s = state.write().await;
        s.spotify_tokens = None;
    }
    Ok(Json(json!({"status": "cleared"})))
}

async fn start_spotify_enrichment(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    use std::sync::atomic::Ordering;

    let (http, event_tx, running, total_atom, processed_atom) = {
        let s = state.read().await;
        (
            s.http_client.clone(),
            s.event_tx.clone(),
            s.spotify_enrich_running.clone(),
            s.spotify_enrich_total.clone(),
            s.spotify_enrich_processed.clone(),
        )
    };

    if running.load(Ordering::SeqCst) {
        let total = total_atom.load(Ordering::SeqCst);
        let processed = processed_atom.load(Ordering::SeqCst);
        return Ok(Json(json!({
            "status": "already_running",
            "total": total,
            "processed": processed
        })));
    }

    // Require credentials and prime a fresh token before enqueueing work.
    let creds = state
        .read()
        .await
        .db
        .with_conn(|conn| Ok(spotify::auth::load_credentials(conn).ok().flatten()))
        .unwrap_or(None);
    let Some(creds) = creds else {
        return Ok(Json(json!({
            "status": "error",
            "message": "Spotify credentials not configured."
        })));
    };

    match spotify::auth::fetch_app_token(&http, &creds).await {
        Ok(tokens) => {
            let mut s = state.write().await;
            s.spotify_tokens = Some(tokens);
        }
        Err(e) => {
            return Ok(Json(json!({
                "status": "error",
                "message": format!("Failed to fetch Spotify token: {}", e)
            })));
        }
    }

    let total: usize = state.read().await.db.with_conn(|conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM tracks t
             WHERE (t.is_favorite = 1 OR t.album_id IN (SELECT id FROM albums WHERE is_favorite = 1))
               AND NOT EXISTS (SELECT 1 FROM spotify_checked sc WHERE sc.track_id = t.id)",
            [], |r| r.get(0)
        )?)
    }).unwrap_or(0);

    if total == 0 {
        return Ok(Json(json!({"status": "already_complete"})));
    }

    running.store(true, Ordering::SeqCst);
    total_atom.store(total, Ordering::SeqCst);
    processed_atom.store(0, Ordering::SeqCst);

    tokio::spawn(async move {
        let progress_tx = event_tx.clone();
        let total_atom_cb = total_atom.clone();
        let processed_atom_cb = processed_atom.clone();
        let result = crate::services::spotify::enrichment::run_enrichment(
            state,
            http,
            move |current, total| {
                processed_atom_cb.store(current, Ordering::SeqCst);
                if total > 0 {
                    total_atom_cb.store(total, Ordering::SeqCst);
                }
                let _ = progress_tx.send(AppEvent::SyncProgress {
                    service: "spotify".to_string(),
                    progress: current as f32 / total.max(1) as f32,
                });
            },
        ).await;

        running.store(false, Ordering::SeqCst);
        if result.is_ok() {
            let _ = event_tx.send(AppEvent::MusicBrainzEnriched);
        }
    });

    Ok(Json(json!({"status": "started", "total": total})))
}

async fn get_spotify_enrichment_status(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    use std::sync::atomic::Ordering;
    let s = state.read().await;
    let enriched: i64 = s.db.with_conn(|conn| {
        Ok(conn.query_row("SELECT COUNT(DISTINCT track_id) FROM track_genres WHERE source = 'spotify'", [], |r| r.get(0))?)
    }).unwrap_or(0);
    let remaining: i64 = s.db.with_conn(|conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM tracks t
             WHERE (t.is_favorite = 1 OR t.album_id IN (SELECT id FROM albums WHERE is_favorite = 1))
               AND NOT EXISTS (SELECT 1 FROM spotify_checked sc WHERE sc.track_id = t.id)",
            [],
            |r| r.get(0),
        )?)
    }).unwrap_or(0);
    let is_running = s.spotify_enrich_running.load(Ordering::SeqCst);
    let run_total = s.spotify_enrich_total.load(Ordering::SeqCst);
    let run_processed = s.spotify_enrich_processed.load(Ordering::SeqCst);

    Ok(Json(json!({
        "enriched_tracks": enriched,
        "remaining_tracks": remaining,
        "is_running": is_running,
        "run_total": run_total,
        "run_processed": run_processed,
    })))
}

// Wipes the spotify_checked table and any track_genres rows from source
// 'spotify'. Use after fixing rate-limiting bugs that may have wrongly
// stamped tracks as "checked" with no tags. Refuses while a run is active.
async fn reset_spotify_enrichment(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use std::sync::atomic::Ordering;
    let s = state.read().await;
    if s.spotify_enrich_running.load(Ordering::SeqCst) {
        return Ok(Json(json!({
            "status": "error",
            "message": "Cannot reset while enrichment is running."
        })));
    }
    let result: anyhow::Result<(usize, usize)> = s.db.with_conn(|conn| {
        let checks = conn.execute("DELETE FROM spotify_checked", [])?;
        let tags = conn.execute("DELETE FROM track_genres WHERE source = 'spotify'", [])?;
        Ok((checks, tags))
    });
    match result {
        Ok((checks, tags)) => Ok(Json(json!({
            "status": "ok",
            "checks_cleared": checks,
            "tags_cleared": tags,
        }))),
        Err(e) => Ok(Json(json!({
            "status": "error",
            "message": format!("Reset failed: {}", e),
        }))),
    }
}

// ── Last.fm Endpoints ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct LastFmConfigRequest {
    api_key: String,
}

async fn lastfm_save_config(
    State(state): State<SharedState>,
    Json(payload): Json<LastFmConfigRequest>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::lastfm;
    let api_key = payload.api_key.trim().to_string();
    if api_key.is_empty() {
        return Ok(Json(json!({
            "status": "error",
            "message": "API key is required."
        })));
    }

    // Verify the key works by hitting a free, parameterless endpoint.
    let http = state.read().await.http_client.clone();
    let probe = http
        .get("https://ws.audioscrobbler.com/2.0/")
        .query(&[
            ("method", "tag.getTopTags"),
            ("api_key", &api_key),
            ("format", "json"),
        ])
        .send()
        .await;
    match probe {
        Ok(resp) if resp.status().is_success() => {
            let body_text = resp.text().await.unwrap_or_default();
            if body_text.contains("\"error\"") {
                return Ok(Json(json!({
                    "status": "error",
                    "message": format!("Last.fm rejected the key: {}",
                        body_text.chars().take(200).collect::<String>())
                })));
            }
            let creds = lastfm::auth::LastFmCredentials { api_key };
            let _ = state.read().await.db.with_conn(|conn| {
                lastfm::auth::save_credentials(conn, &creds)?;
                Ok(())
            });
            Ok(Json(json!({"status": "ok"})))
        }
        Ok(resp) => Ok(Json(json!({
            "status": "error",
            "message": format!("Last.fm rejected the key: HTTP {}", resp.status())
        }))),
        Err(e) => Ok(Json(json!({
            "status": "error",
            "message": format!("Could not reach Last.fm: {}", e)
        }))),
    }
}

async fn lastfm_status(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    use crate::services::lastfm;
    let configured = state
        .read()
        .await
        .db
        .with_conn(|conn| {
            Ok(lastfm::auth::load_credentials(conn)
                .ok()
                .flatten()
                .is_some())
        })
        .unwrap_or(false);
    Ok(Json(json!({"configured": configured})))
}

async fn lastfm_clear_config(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    use crate::services::lastfm;
    let _ = state.read().await.db.with_conn(|conn| {
        lastfm::auth::clear_credentials(conn)?;
        Ok(())
    });
    Ok(Json(json!({"status": "cleared"})))
}

async fn start_lastfm_enrichment(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::lastfm;
    use std::sync::atomic::Ordering;

    let (http, event_tx, running, cancel, total_atom, processed_atom, prefetch_total_atom, prefetch_done_atom, started_at_atom) = {
        let s = state.read().await;
        (
            s.http_client.clone(),
            s.event_tx.clone(),
            s.lastfm_enrich_running.clone(),
            s.lastfm_enrich_cancel.clone(),
            s.lastfm_enrich_total.clone(),
            s.lastfm_enrich_processed.clone(),
            s.lastfm_prefetch_total.clone(),
            s.lastfm_prefetch_done.clone(),
            s.lastfm_enrich_started_at.clone(),
        )
    };

    if running.load(Ordering::SeqCst) {
        let total = total_atom.load(Ordering::SeqCst);
        let processed = processed_atom.load(Ordering::SeqCst);
        return Ok(Json(json!({
            "status": "already_running",
            "total": total,
            "processed": processed
        })));
    }

    let creds = state
        .read()
        .await
        .db
        .with_conn(|conn| Ok(lastfm::auth::load_credentials(conn).ok().flatten()))
        .unwrap_or(None);
    let Some(creds) = creds else {
        return Ok(Json(json!({
            "status": "error",
            "message": "Last.fm API key not configured."
        })));
    };

    let total: usize = state.read().await.db.with_conn(|conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM tracks t
             WHERE (t.is_favorite = 1 OR t.album_id IN (SELECT id FROM albums WHERE is_favorite = 1))
               AND NOT EXISTS (SELECT 1 FROM lastfm_checked lc WHERE lc.track_id = t.id)",
            [], |r| r.get(0)
        )?)
    }).unwrap_or(0);

    if total == 0 {
        return Ok(Json(json!({"status": "already_complete"})));
    }

    cancel.store(false, Ordering::SeqCst);
    running.store(true, Ordering::SeqCst);
    total_atom.store(total, Ordering::SeqCst);
    processed_atom.store(0, Ordering::SeqCst);
    prefetch_total_atom.store(0, Ordering::SeqCst);
    prefetch_done_atom.store(0, Ordering::SeqCst);
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    started_at_atom.store(now_secs, Ordering::SeqCst);

    let api_key = creds.api_key.clone();
    let started_at_atom_cleanup = started_at_atom.clone();
    tokio::spawn(async move {
        let progress_tx = event_tx.clone();
        let artist_tx = event_tx.clone();
        let total_atom_cb = total_atom.clone();
        let processed_atom_cb = processed_atom.clone();
        let prefetch_total_cb = prefetch_total_atom.clone();
        let prefetch_done_cb = prefetch_done_atom.clone();
        let result = lastfm::enrichment::run_enrichment(
            state,
            http,
            api_key,
            cancel,
            move |done, artist_total| {
                prefetch_total_cb.store(artist_total, Ordering::SeqCst);
                prefetch_done_cb.store(done, Ordering::SeqCst);
                let _ = artist_tx.send(AppEvent::SyncProgress {
                    service: "lastfm".to_string(),
                    progress: done as f32 / artist_total.max(1) as f32,
                });
            },
            move |current, total| {
                processed_atom_cb.store(current, Ordering::SeqCst);
                if total > 0 {
                    total_atom_cb.store(total, Ordering::SeqCst);
                }
                let _ = progress_tx.send(AppEvent::SyncProgress {
                    service: "lastfm".to_string(),
                    progress: current as f32 / total.max(1) as f32,
                });
            },
        )
        .await;
        running.store(false, Ordering::SeqCst);
        started_at_atom_cleanup.store(0, Ordering::SeqCst);
        if result.is_ok() {
            let _ = event_tx.send(AppEvent::MusicBrainzEnriched);
        }
    });

    Ok(Json(json!({"status": "started", "total": total})))
}

async fn get_lastfm_enrichment_status(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use std::sync::atomic::Ordering;
    let s = state.read().await;
    let enriched: i64 = s
        .db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(DISTINCT track_id) FROM track_genres WHERE source = 'lastfm'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap_or(0);
    let remaining: i64 = s
        .db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM tracks t
             WHERE (t.is_favorite = 1 OR t.album_id IN (SELECT id FROM albums WHERE is_favorite = 1))
               AND NOT EXISTS (SELECT 1 FROM lastfm_checked lc WHERE lc.track_id = t.id)",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap_or(0);
    let is_running = s.lastfm_enrich_running.load(Ordering::SeqCst);
    let run_total = s.lastfm_enrich_total.load(Ordering::SeqCst);
    let run_processed = s.lastfm_enrich_processed.load(Ordering::SeqCst);
    let prefetch_total = s.lastfm_prefetch_total.load(Ordering::SeqCst);
    let prefetch_done = s.lastfm_prefetch_done.load(Ordering::SeqCst);
    let run_started_at = s.lastfm_enrich_started_at.load(Ordering::SeqCst);
    Ok(Json(json!({
        "enriched_tracks": enriched,
        "remaining_tracks": remaining,
        "is_running": is_running,
        "run_total": run_total,
        "run_processed": run_processed,
        "prefetch_total": prefetch_total,
        "prefetch_done": prefetch_done,
        "run_started_at": run_started_at,
    })))
}

async fn reset_lastfm_enrichment(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use std::sync::atomic::Ordering;
    let s = state.read().await;
    if s.lastfm_enrich_running.load(Ordering::SeqCst) {
        return Ok(Json(json!({
            "status": "error",
            "message": "Cannot reset while enrichment is running."
        })));
    }
    let result: anyhow::Result<(usize, usize)> = s.db.with_conn(|conn| {
        let checks = conn.execute("DELETE FROM lastfm_checked", [])?;
        let tags = conn.execute("DELETE FROM track_genres WHERE source = 'lastfm'", [])?;
        conn.execute("DELETE FROM lastfm_artist_cache", [])?;
        Ok((checks, tags))
    });
    match result {
        Ok((checks, tags)) => Ok(Json(json!({
            "status": "ok",
            "checks_cleared": checks,
            "tags_cleared": tags,
        }))),
        Err(e) => Ok(Json(json!({
            "status": "error",
            "message": format!("Reset failed: {}", e),
        }))),
    }
}

async fn stop_lastfm_enrichment(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use std::sync::atomic::Ordering;
    let s = state.read().await;
    s.lastfm_enrich_cancel.store(true, Ordering::Relaxed);
    Ok(Json(json!({ "status": "stopping" })))
}

// ── Audio Analysis Endpoints ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct AudioAnalysisRequest {
    mode: String, // "preview" or "local"
    local_path: Option<String>,
}

async fn start_audio_analysis(
    State(state): State<SharedState>,
    Json(payload): Json<AudioAnalysisRequest>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::audio_analysis::scanner;

    let mode = payload.mode.clone();
    let local_path = payload.local_path.clone();
    let (analysis_tx, cancel, running) = {
        let s = state.read().await;
        (s.analysis_tx.clone(), s.audio_analysis_cancel.clone(), s.audio_analysis_running.clone())
    };

    let Some(tx) = analysis_tx else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };

    // Reset cancel flag and mark as running before spawning
    cancel.store(false, std::sync::atomic::Ordering::Relaxed);
    running.store(true, std::sync::atomic::Ordering::Relaxed);

    let mode_for_spawn = mode.clone();
    tokio::spawn(async move {
        match mode_for_spawn.as_str() {
            "preview" => {
                scanner::run_preview_scan(state, tx, cancel).await;
            }
            "local" => {
                if let Some(raw) = local_path {
                    // Reject traversal sequences and resolve to a real absolute path
                    let candidate = std::path::PathBuf::from(&raw);
                    let resolved = match std::fs::canonicalize(&candidate) {
                        Ok(p) if p.is_dir() => p,
                        _ => {
                            tracing::warn!("local scan rejected invalid path: {:?}", raw);
                            return;
                        }
                    };
                    scanner::run_local_scan(state, tx, cancel, resolved, Default::default()).await;
                }
            }
            _ => {}
        }
        running.store(false, std::sync::atomic::Ordering::Relaxed);
    });

    Ok(Json(json!({ "status": "started", "mode": mode })))
}

async fn stop_audio_analysis(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    s.audio_analysis_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    s.audio_analysis_running.store(false, std::sync::atomic::Ordering::Relaxed);
    Ok(Json(json!({ "status": "stopped" })))
}

async fn get_audio_analysis_status(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    let analyzed = s.db.with_conn(|conn| queries::count_audio_dsp_features(conn)).unwrap_or(0);
    Ok(Json(json!({
        "running": s.audio_analysis_running.load(std::sync::atomic::Ordering::Relaxed),
        "analyzed": analyzed,
    })))
}

async fn get_track_audio_features(
    State(state): State<SharedState>,
    Path(track_id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    let features = s.db.with_conn(|conn| queries::get_audio_dsp_features(conn, track_id))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "features": features })))
}

async fn get_audio_features_stats(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    let stats = s.db.with_conn(|conn| queries::get_audio_features_stats(conn))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "stats": stats })))
}

async fn get_library_analytics(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    let summary = s
        .db
        .with_conn(|conn| {
            let tracks = queries::get_all_tracks(conn)?;
            let playlists = queries::get_playlists(conn)?;
            let genre_paths = queries::get_track_genre_paths(conn)?;
            let mut context = crate::smart::analytics::AnalyticsContext::new();
            for (track_id, paths) in genre_paths {
                context = context.with_track_genres(track_id, paths);
            }
            Ok(crate::smart::analytics::summarize_library(
                &tracks, &playlists, &context,
            ))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "analytics": summary })))
}

async fn reset_audio_analysis(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    s.db.with_conn(|conn| queries::delete_all_audio_dsp_features(conn))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "status": "reset" })))
}

/// GET /api/library/audio-features/quality — coverage / confidence breakdown.
async fn get_audio_features_quality(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    let q = s
        .db
        .with_conn(|conn| queries::get_audio_features_quality(conn))
        .map_err(|e| {
            tracing::error!("audio-features/quality query failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(json!({
        "total_tracks": q.total_tracks,
        "analyzed": q.analyzed,
        "analysis_v1": q.analysis_v1,
        "analysis_stale": q.analysis_stale,
        "low_confidence_bpm": q.low_confidence_bpm,
        "low_confidence_key": q.low_confidence_key,
        "no_preview_url": q.no_preview_url,
        "fingerprinted": q.fingerprinted,
    })))
}

/// GET /api/library/analyze/reanalyze-stale — re-queue every track whose
/// stored `analysis_version` is not the current `"v1"`. If the analysis
/// actor isn't wired we still return the count of stale tracks so the
/// caller can decide what to do next.
async fn reanalyze_stale_tracks(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let (db, analysis_tx) = {
        let s = state.read().await;
        (s.db.clone(), s.analysis_tx.clone())
    };

    let stale_ids = db
        .with_conn(|conn| queries::get_stale_analysis_track_ids(conn))
        .map_err(|e| {
            tracing::error!("reanalyze-stale query failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let total = stale_ids.len();

    // Drop the DSP rows so the next scan picks them up, and optionally
    // nudge the analysis actor (it only accepts jobs with decoded samples,
    // so here we simply log the queue size — a fresh scan will actually
    // re-decode & re-analyse).
    if total > 0 {
        db.with_conn(|conn| -> anyhow::Result<()> {
            conn.execute(
                "DELETE FROM audio_dsp_features WHERE analysis_version != 'v1'",
                [],
            )?;
            Ok(())
        })
        .map_err(|e| {
            tracing::error!("reanalyze-stale delete failed: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    }

    let actor_configured = analysis_tx.is_some();

    Ok(Json(json!({
        "status": "queued",
        "stale_count": total,
        "actor_configured": actor_configured,
        "note": if actor_configured {
            "Stale analyses cleared. Run /api/library/analyze/audio-features to re-scan."
        } else {
            "Analysis actor not configured. Stale rows cleared but no scan queued."
        }
    })))
}

// ─── Trending / Charts (Phase 5) ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ChartParams {
    /// "lastfm" (default) or "tidal".
    source: Option<String>,
    /// Max entries to return (clamped 1..=100).
    limit: Option<u32>,
    /// Optional country (Last.fm only): full English name e.g. "United States".
    country: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ChartTidalPlayable {
    tidal_id: i64,
    title: String,
    artist_name: Option<String>,
    artist_tidal_id: Option<i64>,
    album_title: Option<String>,
    artwork_url: Option<String>,
    duration_ms: Option<i64>,
    track_id: Option<i64>,
    is_in_library: bool,
}

#[derive(Debug, Clone, Serialize)]
struct ChartEntryDto {
    /// Local library Track when the chart entry was resolved to a known track.
    /// Frontend renders these via `<TrackRow>` and gets full menu support.
    local_track: Option<crate::db::models::Track>,
    /// Otherwise a TidalPlayable-shaped DTO that the frontend renders via
    /// `<TidalTrackRow>`. May be `None` only when both resolutions failed (rare).
    tidal_playable: Option<ChartTidalPlayable>,
    /// Optional preview image (mostly for Last.fm entries with no Tidal match).
    image_url: Option<String>,
    /// Source-tagged for the frontend ("lastfm" | "tidal").
    source: String,
}

/// Cached chart payload with insertion timestamp.
struct ChartCacheEntry {
    inserted_at: Instant,
    payload: serde_json::Value,
}

fn chart_cache() -> &'static StdMutex<HashMap<String, ChartCacheEntry>> {
    static CACHE: OnceLock<StdMutex<HashMap<String, ChartCacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| StdMutex::new(HashMap::new()))
}

/// Charts don't move minute-to-minute and Last.fm rate-limits aggressively.
/// 2-hour TTL is a comfortable middle ground (within the 1–6 h spec).
const CHART_CACHE_TTL: Duration = Duration::from_secs(2 * 60 * 60);

fn chart_cache_get(key: &str) -> Option<serde_json::Value> {
    let cache = chart_cache().lock().ok()?;
    let entry = cache.get(key)?;
    if entry.inserted_at.elapsed() > CHART_CACHE_TTL {
        return None;
    }
    Some(entry.payload.clone())
}

fn chart_cache_put(key: String, payload: serde_json::Value) {
    if let Ok(mut cache) = chart_cache().lock() {
        // Bound cache size to avoid unbounded growth from arbitrary
        // source/country/limit combos. 32 entries is plenty.
        if cache.len() >= 32 {
            cache.clear();
        }
        cache.insert(key, ChartCacheEntry { inserted_at: Instant::now(), payload });
    }
}

async fn get_charts(
    State(state): State<SharedState>,
    Query(params): Query<ChartParams>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let source = params
        .source
        .as_deref()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "lastfm".to_string());
    let limit = params.limit.unwrap_or(50).clamp(1, 100);
    let country_norm = params
        .country
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    // Cache key: source + limit + country.
    let cache_key = format!(
        "{}|{}|{}",
        source,
        limit,
        country_norm.as_deref().unwrap_or("")
    );
    if let Some(cached) = chart_cache_get(&cache_key) {
        return Ok(Json(cached));
    }

    let entries: Vec<ChartEntryDto> = match source.as_str() {
        "tidal" => fetch_tidal_chart(&state, limit as i32).await.map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("tidal chart: {e}") })),
            )
        })?,
        _ => fetch_lastfm_chart(&state, limit, country_norm.as_deref())
            .await
            .map_err(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": format!("lastfm chart: {e}") })),
                )
            })?,
    };

    let payload = json!({
        "source": source,
        "limit": limit,
        "country": country_norm,
        "tracks": entries,
    });
    chart_cache_put(cache_key, payload.clone());
    Ok(Json(payload))
}

async fn fetch_lastfm_chart(
    state: &SharedState,
    limit: u32,
    country: Option<&str>,
) -> anyhow::Result<Vec<ChartEntryDto>> {
    let (http, db) = {
        let s = state.read().await;
        (s.http_client.clone(), s.db.clone())
    };
    let client = LastFmClient::load(http, &db)
        .ok_or_else(|| anyhow::anyhow!("Last.fm API key not configured"))?;
    let tracks = client.get_top_chart(limit, country).await?;

    // Resolve each (artist, title) to a local library track when present.
    // We do this in a single DB call by collecting all (artist, title) pairs
    // and matching case-insensitively.
    let pairs: Vec<(String, String)> = tracks
        .iter()
        .map(|t| (t.artist.clone(), t.title.clone()))
        .collect();
    let local_map = resolve_chart_pairs_to_local(&db, &pairs).unwrap_or_default();

    let mut out = Vec::with_capacity(tracks.len());
    for t in tracks {
        let key = format!(
            "{}\u{0001}{}",
            t.artist.to_ascii_lowercase(),
            t.title.to_ascii_lowercase()
        );
        let local_track = local_map.get(&key).cloned();
        let tidal_playable = if local_track.is_none() {
            // No local match; expose a TidalPlayable-shaped placeholder. The
            // frontend will resolve to a real Tidal id via search if the user
            // clicks play (existing ephemeral-play flow does this).
            Some(ChartTidalPlayable {
                tidal_id: 0,
                title: t.title.clone(),
                artist_name: Some(t.artist.clone()),
                artist_tidal_id: None,
                album_title: None,
                artwork_url: t.image_url.clone(),
                duration_ms: None,
                track_id: None,
                is_in_library: false,
            })
        } else {
            None
        };
        out.push(ChartEntryDto {
            local_track,
            tidal_playable,
            image_url: t.image_url,
            source: "lastfm".to_string(),
        });
    }
    Ok(out)
}

async fn fetch_tidal_chart(
    state: &SharedState,
    limit: i32,
) -> anyhow::Result<Vec<ChartEntryDto>> {
    let (tokens_opt, http, db) = {
        let s = state.read().await;
        (s.tidal_tokens.clone(), s.http_client.clone(), s.db.clone())
    };
    let persisted = load_persisted_tidal_tokens(state).await?;
    let tokens = tokens_opt.or(persisted);
    let Some(tokens) = tokens else {
        // Tidal not connected; degrade to empty list rather than failing.
        tracing::warn!("Tidal chart requested but Tidal not connected");
        return Ok(Vec::new());
    };
    let client = TidalClient::new(tokens.access_token.clone(), tokens.country_code.clone());
    let tracks = match client.get_editorial_top_tracks(limit).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("Tidal editorial chart failed: {}", e);
            Vec::new()
        }
    };
    if tracks.is_empty() {
        return Ok(Vec::new());
    }
    let _ = http; // currently unused; reserved for future fallback paths

    // Resolve to local tracks via tidal_id batch lookup.
    let tidal_ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    let known: HashMap<i64, i64> = db
        .with_conn(|conn| queries::get_tidal_track_local_ids(conn, &tidal_ids))
        .unwrap_or_default();

    // Pull full Track rows for any local matches.
    let local_ids: Vec<i64> = known.values().copied().collect();
    let local_tracks: HashMap<i64, crate::db::models::Track> = db
        .with_conn(|conn| {
            let mut map: HashMap<i64, crate::db::models::Track> = HashMap::new();
            if local_ids.is_empty() {
                return Ok(map);
            }
            let mut stmt = conn.prepare(
                "SELECT t.id, t.title, t.artist_id, a.name as artist_name, t.album_id, al.title as album_title,
                        t.disc_number, t.track_number, t.duration_ms, t.isrc, t.tidal_id, t.ytmusic_id,
                        t.soundcloud_id, t.best_quality, t.best_source, t.fidelity_score, t.is_favorite,
                        t.play_count, t.last_played_at, t.date_added, t.source, t.artwork_url
                 FROM tracks t
                 LEFT JOIN artists a ON t.artist_id = a.id
                 LEFT JOIN albums al ON t.album_id = al.id
                 WHERE t.id = ?1
                 LIMIT 1",
            )?;
            for id in &local_ids {
                let mut rows = stmt.query(rusqlite::params![id])?;
                if let Some(row) = rows.next()? {
                    let track = crate::db::models::Track {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        artist_id: row.get(2)?,
                        artist_name: row.get(3)?,
                        album_id: row.get(4)?,
                        album_title: row.get(5)?,
                        disc_number: row.get(6)?,
                        track_number: row.get(7)?,
                        duration_ms: row.get(8)?,
                        isrc: row.get(9)?,
                        tidal_id: row.get(10)?,
                        ytmusic_id: row.get(11)?,
                        soundcloud_id: row.get(12)?,
                        best_quality: row.get(13)?,
                        best_source: row.get(14)?,
                        fidelity_score: row.get(15)?,
                        is_favorite: row.get::<_, i64>(16)? != 0,
                        play_count: row.get(17)?,
                        last_played_at: row.get(18)?,
                        date_added: row.get(19)?,
                        source: row.get(20)?,
                        artwork_url: row.get(21)?,
                    };
                    map.insert(*id, track);
                }
            }
            Ok(map)
        })
        .unwrap_or_default();

    let mut out = Vec::with_capacity(tracks.len());
    for t in tracks {
        let local_track = known
            .get(&t.id)
            .and_then(|lid| local_tracks.get(lid))
            .cloned();
        let tidal_playable = if local_track.is_none() {
            Some(ChartTidalPlayable {
                tidal_id: t.id,
                title: t.title.clone(),
                artist_name: t.artist_name.clone(),
                artist_tidal_id: t.artist_id,
                album_title: t.album_title.clone(),
                artwork_url: t.artwork_url.clone(),
                duration_ms: Some(t.duration * 1000),
                track_id: None,
                is_in_library: false,
            })
        } else {
            None
        };
        out.push(ChartEntryDto {
            image_url: t.artwork_url.clone(),
            local_track,
            tidal_playable,
            source: "tidal".to_string(),
        });
    }
    Ok(out)
}

/// Resolve (artist, title) pairs to local Track rows, case-insensitively.
/// Uses a single SQL query with a fold-table; falls back to empty map on error.
fn resolve_chart_pairs_to_local(
    db: &crate::db::Database,
    pairs: &[(String, String)],
) -> anyhow::Result<HashMap<String, crate::db::models::Track>> {
    if pairs.is_empty() {
        return Ok(HashMap::new());
    }
    let mut out = HashMap::new();
    db.with_conn(|conn| {
        // Match by lower(artist_name) + lower(title). Library is small enough
        // that one round-trip per pair is acceptable; a single OR-chained
        // query also works but is harder to map back to pairs.
        let mut stmt = conn.prepare(
            "SELECT t.id, t.title, t.artist_id, a.name as artist_name, t.album_id, al.title as album_title,
                    t.disc_number, t.track_number, t.duration_ms, t.isrc, t.tidal_id, t.ytmusic_id,
                    t.soundcloud_id, t.best_quality, t.best_source, t.fidelity_score, t.is_favorite,
                    t.play_count, t.last_played_at, t.date_added, t.source, t.artwork_url
             FROM tracks t
             LEFT JOIN artists a ON t.artist_id = a.id
             LEFT JOIN albums al ON t.album_id = al.id
             WHERE LOWER(a.name) = LOWER(?1) AND LOWER(t.title) = LOWER(?2)
             LIMIT 1",
        )?;
        for (artist, title) in pairs {
            let mut rows = stmt.query(rusqlite::params![artist, title])?;
            if let Some(row) = rows.next()? {
                let track = crate::db::models::Track {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    artist_id: row.get(2)?,
                    artist_name: row.get(3)?,
                    album_id: row.get(4)?,
                    album_title: row.get(5)?,
                    disc_number: row.get(6)?,
                    track_number: row.get(7)?,
                    duration_ms: row.get(8)?,
                    isrc: row.get(9)?,
                    tidal_id: row.get(10)?,
                    ytmusic_id: row.get(11)?,
                    soundcloud_id: row.get(12)?,
                    best_quality: row.get(13)?,
                    best_source: row.get(14)?,
                    fidelity_score: row.get(15)?,
                    is_favorite: row.get::<_, i64>(16)? != 0,
                    play_count: row.get(17)?,
                    last_played_at: row.get(18)?,
                    date_added: row.get(19)?,
                    source: row.get(20)?,
                    artwork_url: row.get(21)?,
                };
                let key = format!(
                    "{}\u{0001}{}",
                    artist.to_ascii_lowercase(),
                    title.to_ascii_lowercase()
                );
                out.insert(key, track);
            }
        }
        Ok(())
    })?;
    Ok(out)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, schema};
    use axum::{body::Body, http::Request};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tower::ServiceExt;

    #[test]
    fn stream_error_mapping_marks_session_expired_as_unauthorized() {
        let (status, Json(body)) = tidal_playback_error_response(
            42,
            TidalPlaybackError::StreamResolve(tidal_stream::StreamResolveError::SessionExpired {
                message: "expired".to_string(),
            }),
            "fallback",
        );

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["status"], "session_expired");
        assert_eq!(body["track_id"], 42);
    }

    #[test]
    fn stream_error_mapping_marks_manifest_decode_failures_as_bad_gateway() {
        let (status, Json(body)) = tidal_playback_error_response(
            7,
            TidalPlaybackError::StreamResolve(
                tidal_stream::StreamResolveError::ManifestDecodeFailed {
                    message: "bad base64".to_string(),
                },
            ),
            "fallback",
        );

        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(body["status"], "manifest_decode_failed");
        assert_eq!(body["track_id"], 7);
    }

    #[test]
    fn stream_error_mapping_marks_rejected_stream_requests_as_forbidden() {
        let (status, Json(body)) = tidal_playback_error_response(
            11,
            TidalPlaybackError::StreamResolve(tidal_stream::StreamResolveError::StreamRejected {
                message: "rejected".to_string(),
            }),
            "fallback",
        );

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["status"], "stream_rejected");
        assert_eq!(body["track_id"], 11);
    }

    #[test]
    fn extracts_genre_candidates_from_mixed_metadata_shapes() {
        let mut extra = HashMap::new();
        extra.insert("genre".to_string(), json!("trip hop"));
        extra.insert(
            "subGenres".to_string(),
            json!([
                "shoegazee",
                { "name": "Tech House / House" },
                { "title": "Progressive House" }
            ]),
        );

        let genres = crate::genre::builder::collect_clear_genres(
            extract_genre_candidates_from_extra(&extra),
        );

        assert_eq!(
            genres,
            vec![
                "Progressive House".to_string(),
                "Shoegaze".to_string(),
                "Trip-Hop".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn genre_heat_route_defaults_to_ninety_days() {
        let db_path =
            std::env::temp_dir().join(format!("noor-genre-heat-{}.db", uuid::Uuid::new_v4()));
        let db = Database::open(&db_path).expect("db opened");
        db.run_migrations().expect("migrations");
        db.with_conn(|conn| {
            schema::run_migrations(conn)?;
            conn.execute(
                "INSERT INTO genres (id, name, slug, parent_id) VALUES
                    (1, 'Electronic', 'electronic', NULL),
                    (2, 'Ambient', 'ambient', 1)",
                [],
            )?;
            conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Biosphere')", [])?;
            conn.execute(
                "INSERT INTO tracks (
                    id, title, artist_id, duration_ms, tidal_id, best_quality, best_source, fidelity_score, is_favorite, source
                ) VALUES (1, 'Substrata', 1, 360000, 201, 'LOSSLESS', 'tidal', 10, 1, 'tidal')",
                [],
            )?;
            conn.execute(
                "INSERT INTO track_genres (track_id, genre_id, source, confidence)
                 VALUES (1, 2, 'musicbrainz', 1.0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO listen_history (track_id, started_at, duration_listened_ms, completed)
                 VALUES (1, datetime('now', '-10 days'), 180000, 1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO listen_history (track_id, started_at, duration_listened_ms, completed)
                 VALUES (1, datetime('now', '-120 days'), 180000, 1)",
                [],
            )?;
            Ok(())
        })
        .expect("seeded");

        let (event_tx, _) = tokio::sync::broadcast::channel(16);
        let app = api_routes(Arc::new(tokio::sync::RwLock::new(crate::AppState {
            db,
            event_tx,
            http_client: reqwest::Client::new(),
            tidal_tokens: None,
            spotify_tokens: None,
            playback_runtime: None,
            playback_runtime_info: None,
            current_stream_display: None,
            pending_stream_display: None,
            active_listen_session: None,
            external_playback_track: None,
            ephemeral_tidal_track: None,
            tidal_login_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            rss_aggregator: Arc::new(crate::services::rss_feeds::FeedAggregator::new(reqwest::Client::new())),
            acrcloud_client: None,
            analysis_tx: None,
            audio_analysis_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            audio_analysis_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            acrcloud_scan_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            acrcloud_daily_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            spotify_enrich_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            spotify_enrich_total: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            spotify_enrich_processed: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            lastfm_enrich_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            lastfm_enrich_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            lastfm_enrich_total: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            lastfm_enrich_processed: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            lastfm_prefetch_total: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            lastfm_prefetch_done: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            lastfm_enrich_started_at: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            discovery_train_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            server_token: String::new(),
            audio_active: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/genres/heat")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        let payload: Value = serde_json::from_slice(&body).expect("json body");
        let electronic = payload["heat"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["genre_id"] == 1))
            .expect("electronic row");

        assert_eq!(electronic["listen_count"], 1);
        assert_eq!(electronic["total_listened_ms"], 180000);

        let _ = std::fs::remove_file(db_path);
    }

    /// Build a minimal test app backed by a fresh in-memory database.
    async fn build_test_app() -> Router {
        let db_path =
            std::env::temp_dir().join(format!("noor-test-{}.db", uuid::Uuid::new_v4()));
        let db = Database::open(&db_path).expect("db opened");
        db.run_migrations().expect("migrations");
        db.with_conn(|conn| schema::run_migrations(conn)).expect("schema migrations");
        let (event_tx, _) = tokio::sync::broadcast::channel(16);
        api_routes(Arc::new(tokio::sync::RwLock::new(crate::AppState {
            db,
            event_tx,
            http_client: reqwest::Client::new(),
            tidal_tokens: None,
            spotify_tokens: None,
            playback_runtime: None,
            playback_runtime_info: None,
            current_stream_display: None,
            pending_stream_display: None,
            active_listen_session: None,
            external_playback_track: None,
            ephemeral_tidal_track: None,
            tidal_login_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            rss_aggregator: Arc::new(crate::services::rss_feeds::FeedAggregator::new(reqwest::Client::new())),
            acrcloud_client: None,
            analysis_tx: None,
            audio_analysis_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            audio_analysis_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            acrcloud_scan_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            acrcloud_daily_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            spotify_enrich_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            spotify_enrich_total: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            spotify_enrich_processed: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            lastfm_enrich_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            lastfm_enrich_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            lastfm_enrich_total: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            lastfm_enrich_processed: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            lastfm_prefetch_total: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            lastfm_prefetch_done: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            lastfm_enrich_started_at: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            discovery_train_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            server_token: String::new(),
            audio_active: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })))
    }

    #[tokio::test]
    async fn server_info_returns_defaults() {
        let app = build_test_app().await;
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/server/info")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["host_mode"], false);
        assert!(body["bind_address"].as_str().unwrap().contains("3334"));
        assert!(body["version"].is_string());
    }

    #[tokio::test]
    async fn put_host_mode_persists() {
        let app = build_test_app().await;

        // Enable host mode
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/server/host_mode")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"host_mode":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Reading info should now reflect host_mode = true
        let resp2 = app
            .oneshot(
                Request::builder()
                    .uri("/api/server/info")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&axum::body::to_bytes(resp2.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["host_mode"], true);
        assert!(body["bind_address"].as_str().unwrap().starts_with("0.0.0.0"));
    }

    /// Reproducer for the Phase 2b hotfix: `/api/radio/song` must
    /// reject ephemeral Tidal track ids (negative or zero) with a
    /// 400 + actionable error body, not a 500 with no body.
    ///
    /// Pre-fix behaviour: handler accepted any i64, passed it
    /// through to `orchestrate_song` which logged
    /// `WARN "radio_song failed: seed track not found: -85771852"`
    /// and returned 500. Frontend kept the prior queue, producing
    /// the "kitchen-sink" symptom the bug report described.
    #[tokio::test]
    async fn radio_song_rejects_negative_seed_id_with_400() {
        let app = build_test_app().await;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/radio/song")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"seed_track_id": -85771852}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap(),
        )
        .unwrap();
        assert!(
            body["error"]
                .as_str()
                .unwrap_or("")
                .contains("positive library id"),
            "unexpected error body: {body}"
        );
        assert!(
            body["hint"].as_str().unwrap_or("").contains("seed_tidal_id"),
            "expected hint to mention seed_tidal_id: {body}"
        );
    }

    /// Boundary: `seed_track_id == 0` is also rejected. Zero is
    /// neither a valid library id (rowids start at 1) nor an
    /// ephemeral negative — it usually indicates a serialisation
    /// default leaking through, which still shouldn't reach the
    /// orchestrator.
    #[tokio::test]
    async fn radio_song_rejects_zero_seed_id_with_400() {
        let app = build_test_app().await;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/radio/song")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"seed_track_id": 0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
