use crate::db::queries;
use crate::library::duplicates as dup;
use crate::metadata::discogs::DiscogsClient;
use crate::metadata::lastfm::LastFmClient;
use crate::playback::{player, queue, runtime as playback_runtime};
use crate::services::discovery::{
    DiscoveryCandidateSeed, DiscoveryProvider, TidalDiscoveryProvider,
};
use crate::services::discovery_space as ds;
use crate::services::learning as discovery_learning;
use crate::services::spotify;
use crate::services::tidal::{
    auth as tidal_auth,
    client::{TidalClient, TidalSearchTrack, TidalSearchVideo, TidalTrack},
    import as tidal_import, mutations as tidal_mutations, stream as tidal_stream,
};
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
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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
    // Legacy naming: despite "favorite_only", this means "library tracks" =
    // tracks where tracks.is_favorite=1 OR the parent album has albums.is_favorite=1.
    // For a strict "user explicitly liked this track" filter, use `liked_only` instead.
    favorite_only: Option<bool>,
    // Strict filter: tracks where tracks.is_favorite=1 only. Takes precedence
    // over `favorite_only` when both are set.
    liked_only: Option<bool>,
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
    /// Galaxy display filter — see `crate::genre::filter::GalaxyFilterRule`.
    /// Tokens: `all` | `conf05` | `conf07` | `top2` | `top3` | `mb_only` |
    /// `primary`. Unknown / missing → default (`conf05`).
    filter: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GenreHeatParams {
    days: Option<i64>,
    /// See `GenreTrackParams::filter`.
    filter: Option<String>,
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
pub struct AnalyticsSignalsParams {
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

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueueExternalKind {
    Library,
    Tidal,
    External,
}

#[derive(Debug, Deserialize)]
pub struct QueueExternalRequest {
    kind: QueueExternalKind,
    #[serde(default)]
    track_id: Option<i64>,
    #[serde(default)]
    tidal_id: Option<i64>,
    #[serde(default)]
    artist: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct QueueExternalManyRequest {
    items: Vec<QueueExternalRequest>,
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
        .route("/api/artists/{id}", get(get_artist))
        .route("/api/artists/{id}/tracks", get(get_artist_tracks))
        .route("/api/artists/{id}/discography", get(get_artist_discography))
        .route(
            "/api/artists/{id}/spotify-stats",
            get(get_artist_spotify_stats),
        )
        .route("/api/tidal/albums/{id}/tracks", get(get_tidal_album_tracks))
        .route("/api/tidal/albums/{id}/import", post(import_tidal_album))
        .route(
            "/api/tidal/tracks/import",
            post(import_tidal_track_for_radio),
        )
        .route("/api/genres", get(get_genres))
        .route("/api/genres/heat", get(get_genre_heat))
        .route("/api/genres/co-occurrence", get(get_genre_co_occurrence))
        .route("/api/genres/cohorts", get(get_genre_cohorts))
        .route("/api/genres/evolution", get(get_genre_evolution))
        .route("/api/genres/audio-metrics", get(get_genre_audio_metrics))
        .route("/api/genres/{id}/tracks", get(get_genre_tracks))
        .route("/api/playlists", get(get_playlists))
        .route(
            "/api/playlists/{id}/tracks",
            get(get_playlist_tracks).post(add_tracks_to_playlist_route),
        )
        .route(
            "/api/playlists/{id}/favorite",
            patch(toggle_playlist_favorite_route),
        )
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
        .route("/api/analytics/signals", get(get_analytics_signals))
        .route("/api/analytics/listens/recent", get(get_recent_listens))
        .route("/api/discovery/preview", post(preview_discovery))
        .route("/api/discovery/new", post(discover_new_music))
        .route("/api/discovery/save", post(save_discovery_track))
        .route("/api/discovery/play", post(play_discovery_track))
        .route("/api/discovery/connections", post(discover_connected_music))
        .route("/api/discovery/status", get(get_discovery_status))
        .route("/api/discovery/train", post(start_discovery_training))
        .route(
            "/api/discovery/train/status",
            get(get_discovery_training_status),
        )
        .route("/api/discovery/train/stop", post(stop_discovery_training))
        .route(
            "/api/discovery/train/intensity",
            get(get_discovery_intensity).post(set_discovery_intensity),
        )
        .route("/api/discovery/train/safety", get(get_discovery_safety))
        .route("/api/discovery/feedback", post(record_discovery_feedback))
        .route(
            "/api/discovery/presets",
            get(get_discovery_presets).post(create_discovery_preset),
        )
        // Similar Radio
        .route("/api/discovery/radio", post(get_radio_tracks))
        .route(
            "/api/discovery/radio/compute",
            post(compute_radio_similarity),
        )
        // Discovery Sound Space
        .route("/api/discovery/space", post(get_discovery_space))
        // Sportify-based discovery resolver — single, bulk, and cache-only status poll.
        .route("/api/resolve/tidal/track", get(resolve_tidal_track))
        .route("/api/resolve/tidal/bulk", post(resolve_tidal_bulk))
        .route("/api/resolve/tidal/status", get(resolve_tidal_status))
        // Sportify (anonymous Spotify metadata proxy) discovery surface.
        // Sportify is upstream and subject to breakage — every handler is
        // cache-first, every failure surfaces as JSON error or empty list,
        // and nothing here writes to library tables. Worst case for an
        // outage is a degraded /discover; existing library data is never
        // affected.
        .route(
            "/api/discovery/sportify/search",
            get(sportify_discovery_search),
        )
        .route(
            "/api/discovery/sportify/track/{spotify_id}",
            get(sportify_discovery_track),
        )
        .route(
            "/api/discovery/sportify/album/{spotify_id}",
            get(sportify_discovery_album),
        )
        .route(
            "/api/discovery/sportify/playlist/{spotify_id}",
            get(sportify_discovery_playlist),
        )
        .route(
            "/api/discovery/sportify/artist/{spotify_id}",
            get(sportify_discovery_artist),
        )
        .route(
            "/api/discovery/sportify/artist/{spotify_id}/top-tracks",
            get(sportify_discovery_artist_top_tracks),
        )
        .route(
            "/api/discovery/sportify/artist/{spotify_id}/related",
            get(sportify_discovery_artist_related),
        )
        .route(
            "/api/discovery/sportify/album/{spotify_id}/related",
            get(sportify_discovery_album_related),
        )
        .route(
            "/api/discovery/sportify/track/{spotify_id}/related",
            get(sportify_discovery_track_related),
        )
        // Save an ephemeral Spotify-sourced playlist into the user's library.
        // Imports each resolved TIDAL track + creates a noor playlist; rows
        // without a TIDAL match are skipped (counted in the response).
        .route("/api/spotify-playlist/save", post(save_spotify_playlist))
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
        .route("/api/queue/play_next", post(queue_play_next))
        .route("/api/queue/play_next_many", post(queue_play_next_many))
        .route("/api/queue/append", post(queue_append))
        .route("/api/queue/append_many", post(queue_append_many))
        .route(
            "/api/playlists/from-queue",
            post(create_playlist_from_queue),
        )
        // Audio output settings + device enumeration
        .route("/api/audio/devices", get(get_audio_devices))
        .route(
            "/api/audio/settings",
            get(get_audio_settings).put(put_audio_settings),
        )
        .route(
            "/api/audio/exclusive/retry",
            post(post_audio_exclusive_retry),
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
        .route("/api/tidal/sync/cancel", post(tidal_sync_cancel))
        .route("/api/tidal/status", get(tidal_status))
        .route(
            "/api/tidal/backoff",
            axum::routing::get(get_tidal_backoff_status),
        )
        .route("/api/tidal/search", get(tidal_search))
        .route("/api/tidal/videos/search", get(tidal_video_search))
        .route("/api/tidal/videos/{id}/playback", get(tidal_video_playback))
        .route(
            "/api/tidal/video-mixes/{id}/items",
            get(tidal_video_mix_items),
        )
        .route("/api/tidal/playlists/search", get(tidal_playlist_search))
        .route(
            "/api/tidal/playlists/{uuid}/tracks",
            get(tidal_playlist_tracks),
        )
        .route("/api/tidal/play", post(play_tidal_ephemeral))
        .route("/api/tidal/artists/{tidal_id}", get(tidal_artist_profile))
        .route("/api/tidal/logout", post(tidal_logout))
        // Spotify
        .route("/api/spotify/config", post(spotify_save_config))
        .route(
            "/api/spotify/config",
            axum::routing::delete(spotify_clear_config),
        )
        .route("/api/spotify/status", get(spotify_status))
        .route(
            "/api/library/enrich/spotify",
            post(start_spotify_enrichment),
        )
        .route(
            "/api/library/enrich/spotify/status",
            get(get_spotify_enrichment_status),
        )
        .route(
            "/api/library/enrich/spotify/reset",
            post(reset_spotify_enrichment),
        )
        .route(
            "/api/library/tidal-stream/purge",
            post(purge_orphan_tidal_stream_tracks),
        )
        // Last.fm
        .route("/api/lastfm/config", post(lastfm_save_config))
        .route(
            "/api/lastfm/config",
            axum::routing::delete(lastfm_clear_config),
        )
        .route("/api/lastfm/status", get(lastfm_status))
        // Last.fm scrobble auth (server-side flow — `LASTFM_API_SECRET` env required)
        .route("/api/lastfm/auth/start", post(lastfm_auth_start))
        .route("/api/lastfm/auth/complete", post(lastfm_auth_complete))
        .route("/api/lastfm/auth/disconnect", post(lastfm_auth_disconnect))
        .route("/api/library/enrich/lastfm", post(start_lastfm_enrichment))
        .route(
            "/api/library/enrich/lastfm/stop",
            post(stop_lastfm_enrichment),
        )
        .route(
            "/api/library/enrich/lastfm/status",
            get(get_lastfm_enrichment_status),
        )
        .route(
            "/api/library/enrich/lastfm/reset",
            post(reset_lastfm_enrichment),
        )
        // Audio analysis
        .route(
            "/api/library/analyze/audio-features",
            post(start_audio_analysis),
        )
        .route("/api/library/analyze/stop", post(stop_audio_analysis))
        .route(
            "/api/library/analyze/status",
            get(get_audio_analysis_status),
        )
        .route(
            "/api/library/analyze/passive",
            get(get_passive_dsp).put(set_passive_dsp),
        )
        .route(
            "/api/tracks/{id}/audio-features",
            get(get_track_audio_features),
        )
        .route(
            "/api/library/audio-features/stats",
            get(get_audio_features_stats),
        )
        .route(
            "/api/library/audio-features/quality",
            get(get_audio_features_quality),
        )
        .route("/api/library/analytics", get(get_library_analytics))
        .route(
            "/api/library/analyze/reanalyze-stale",
            get(reanalyze_stale_tracks),
        )
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
        // TIDAL "Your Mixes" — drives the home Your Mixes shelf above Trending.
        .route("/api/tidal/mixes", get(get_tidal_mixes))
        .route("/api/tidal/mixes/{id}/tracks", get(get_tidal_mix_tracks))
        .route("/api/tidal/play-mix", post(play_tidal_mix))
        // TIDAL "Personal Radio" — drives the home Personal Radio shelf.
        .route("/api/tidal/radio-stations", get(get_tidal_radio_stations))
        // TIDAL editorial home modules — drives the search-page discover surface.
        .route("/api/tidal/home-modules", get(get_tidal_home_modules))
        // Per-module detail items (View all). Resolves the module's
        // dataApiPath server-side and returns the full item set.
        .route(
            "/api/tidal/discover-modules/{id}/items",
            get(get_tidal_discover_module_items),
        )
        // Trending / charts (Phase 5)
        .route("/api/charts", get(get_charts))
        .route("/api/charts/lastfm/genres", get(list_lastfm_genres))
        .route("/api/charts/lastfm/countries", get(list_lastfm_countries))
        // Server auth management
        .route("/api/server/token", get(get_server_token_handler))
        .route(
            "/api/server/token/regenerate",
            post(regenerate_server_token_handler),
        )
        // Server configuration
        .route("/api/server/info", get(get_server_info))
        .route("/api/server/host_mode", put(put_server_host_mode))
        .with_state(state)
}

async fn get_server_token_handler(State(state): State<SharedState>) -> Json<Value> {
    let s = state.read().await;
    Json(json!({ "token": s.server_token }))
}

async fn regenerate_server_token_handler(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let new_token = {
        let s = state.read().await;
        s.db.with_conn(crate::db::queries::regenerate_server_token)
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

    let bind_address = if host_mode {
        "0.0.0.0:3334"
    } else {
        "127.0.0.1:3334"
    };
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

    let bind_address = if host_mode {
        "0.0.0.0:3334"
    } else {
        "127.0.0.1:3334"
    };
    Ok(Json(
        json!({ "host_mode": host_mode, "bind_address": bind_address }),
    ))
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
    let liked_only = params.liked_only.unwrap_or(false);

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
            let tracks = queries::get_tracks_with_dsp(
                conn,
                sort_by,
                sort_dir,
                limit,
                offset,
                favorite_only,
                liked_only,
                &dsp,
            )?;
            let total = queries::get_track_count(conn, favorite_only, liked_only)?;
            Ok(Json(json!({ "tracks": tracks, "total": total })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_track_count(
    State(state): State<SharedState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Value>, StatusCode> {
    let favorite_only = params.favorite_only.unwrap_or(false);
    let liked_only = params.liked_only.unwrap_or(false);
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let count = queries::get_track_count(conn, favorite_only, liked_only)?;
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
            let tracks = queries::get_artist_library_tracks(conn, artist_id)?;
            Ok(Json(json!({ "tracks": tracks })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_artist(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    let row =
        s.db.with_conn(|conn| queries::get_artist_with_counts(conn, id))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let Some((artist, track_count, album_count)) = row else {
        return Err(StatusCode::NOT_FOUND);
    };

    Ok(Json(json!({
        "id": artist.id,
        "tidal_id": artist.tidal_id,
        "name": artist.name,
        "biography": artist.biography,
        "photo_url": artist.photo_url,
        "track_count": track_count,
        "album_count": album_count,
    })))
}

async fn get_album_tracks(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    // Three-pass approach so the page can render the FULL album (not just
    // library coverage):
    //   1. Pull the local rows + the album's TIDAL id in one DB hit.
    //   2. If TIDAL is connected and the album maps to a TIDAL id, fetch the
    //      full TIDAL track list.
    //   3. Filter TIDAL tracks down to only those NOT already in `tracks`
    //      (deduped by tidal_id) and serialize as `tidal_tracks`.
    //
    // The frontend renders both arrays; the user gets a single coherent track
    // listing where library entries are styled as "owned" and pure-TIDAL
    // entries get a TIDAL pill.
    let (tracks, album_tidal_id) = {
        let s = state.read().await;
        let result = s.db.with_conn(|conn| {
            let tracks = queries::get_album_tracks(conn, id)?;
            let pairs = queries::get_album_tidal_ids(conn, &[id])?;
            let tidal_id = pairs.first().map(|(_, t)| *t);
            Ok::<_, anyhow::Error>((tracks, tidal_id))
        });
        match result {
            Ok((tracks, tidal_id)) => (tracks, tidal_id),
            Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
        }
    };

    // No TIDAL id → can't enrich; return library tracks alone.
    let Some(tidal_album_id) = album_tidal_id else {
        return Ok(Json(json!({
            "tracks": tracks,
            "tidal_tracks": [],
            "album_tidal_id": null,
        })));
    };

    // TIDAL session needed for the catalog fetch — best-effort only.
    let (tokens, tidal_http_client) = {
        let persisted = match load_persisted_tidal_tokens(&state).await {
            Ok(p) => p,
            Err(_) => None,
        };
        let s = state.read().await;
        (
            s.tidal_tokens.clone().or(persisted),
            s.tidal_http_client.clone(),
        )
    };

    let Some(tokens) = tokens else {
        return Ok(Json(json!({
            "tracks": tracks,
            "tidal_tracks": [],
            "album_tidal_id": tidal_album_id,
        })));
    };

    let client = TidalClient::with_http(
        tidal_http_client,
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );

    let tidal_tracks_payload: Vec<Value> = match client.get_album_tracks(tidal_album_id).await {
        Ok(resp) => {
            // Local rows that came from TIDAL carry a `tidal_id`; dedupe so
            // the same track doesn't appear twice (once styled as library,
            // once as TIDAL-only).
            let local_tidal_ids: std::collections::HashSet<i64> =
                tracks.iter().filter_map(|t| t.tidal_id).collect();

            resp.items
                .into_iter()
                .filter(|t| !local_tidal_ids.contains(&t.id))
                .map(|t| {
                    let artwork = t
                        .album
                        .as_ref()
                        .and_then(|al| al.cover.as_ref())
                        .and_then(|c| {
                            crate::services::tidal::client::TidalClient::get_artwork_url(
                                &Some(c.clone()),
                                160,
                            )
                        });
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
                    })
                })
                .collect()
        }
        Err(e) => {
            tracing::warn!(?e, "TIDAL get_album_tracks failed; serving library only");
            Vec::new()
        }
    };

    Ok(Json(json!({
        "tracks": tracks,
        "tidal_tracks": tidal_tracks_payload,
        "album_tidal_id": tidal_album_id,
    })))
}

/// Pages through all entries of a single TIDAL discography filter for one
/// artist. TIDAL's `/artists/{id}/albums` returns at most 50 per call, sorted
/// newest-first; calling once would silently clip anything older than the 50th
/// most-recent release per filter (i.e. anything past page 1). Stops on a
/// short page (TIDAL's "no more" signal) or when the running count reaches
/// `total_number_of_items`. Capped at 1000 entries per filter as a safety net.
async fn fetch_all_artist_albums(
    client: &TidalClient,
    artist_id: i64,
    filter: &str,
) -> anyhow::Result<Vec<crate::services::tidal::client::TidalAlbum>> {
    const PAGE: i32 = 50;
    const MAX_PAGES: i32 = 20;
    let mut out: Vec<crate::services::tidal::client::TidalAlbum> = Vec::new();
    let mut offset: i32 = 0;
    for _ in 0..MAX_PAGES {
        let page = client
            .get_artist_albums(artist_id, PAGE, offset, Some(filter))
            .await?;
        let n = page.items.len() as i32;
        let total = page.total_number_of_items;
        out.extend(page.items);
        if n < PAGE {
            break;
        }
        if let Some(t) = total
            && (out.len() as i64) >= t
        {
            break;
        }
        offset += PAGE;
    }
    Ok(out)
}

async fn get_artist_discography(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let tidal_artist_id = {
        let s = state.read().await;
        s.db.with_conn(|conn| queries::get_artist_tidal_id(conn, id))
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": e.to_string() })),
                )
            })?
    };

    let Some(tidal_artist_id) = tidal_artist_id else {
        return Ok(Json(json!({
            "albums": [],
            "top_tracks": [],
            "available": false,
            "reason": "artist_not_on_tidal"
        })));
    };

    let (tokens, tidal_http_client) = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;
        let s = state.read().await;
        (
            s.tidal_tokens.clone().or(persisted),
            s.tidal_http_client.clone(),
        )
    };

    let Some(tokens) = tokens else {
        return Ok(Json(json!({
            "albums": [],
            "top_tracks": [],
            "available": false,
            "reason": "tidal_not_connected"
        })));
    };

    let client = TidalClient::with_http(
        tidal_http_client,
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );

    // Each filter is paginated separately; previously we fetched only the first
    // page (50 newest), which clipped any artist with a long catalog (e.g. a
    // 50+ year discography returned only modern compilations).
    let albums_fut = fetch_all_artist_albums(&client, tidal_artist_id, "ALBUMS");
    let eps_fut = fetch_all_artist_albums(&client, tidal_artist_id, "EPSANDSINGLES");
    let compilations_fut = fetch_all_artist_albums(&client, tidal_artist_id, "COMPILATIONS");
    let live_fut = fetch_all_artist_albums(&client, tidal_artist_id, "LIVE");
    // Top tracks raised from 10 → 50 so the merged Top Tracks list on the
    // artist page surfaces a meaningful catalog even when the user has zero
    // library matches; 50 is TIDAL's per-page max.
    let top_fut = client.get_artist_top_tracks(tidal_artist_id, 50, 0);
    let videos_fut = client.get_artist_videos(tidal_artist_id, 50, 0);
    let similar_fut = client.get_artist_similar(tidal_artist_id, 20, 0);
    let bio_fut = client.get_artist_bio(tidal_artist_id);
    // Profile fetch in the same parallel batch — gives us the artist's
    // canonical `picture` URL so the page hero can fall back to TIDAL
    // when the local row has no `photo_url`.
    let profile_fut = client.get_artist(tidal_artist_id);

    let (
        albums_res,
        eps_res,
        comps_res,
        live_res,
        top_res,
        videos_res,
        similar_res,
        bio_res,
        profile_res,
    ) = tokio::join!(
        albums_fut,
        eps_fut,
        compilations_fut,
        live_fut,
        top_fut,
        videos_fut,
        similar_fut,
        bio_fut,
        profile_fut
    );

    // Picture URL fallback chain. TIDAL's `/artists/{id}` record is the
    // canonical source, but it ships `picture: null` for many artists.
    // We then try the artist's own `picture` as embedded in their top
    // tracks, then finally fall back to an album cover — same trick the
    // library Recently Played Artists rail uses to keep tiles populated
    // when no artist photo exists. Extracted *before* the result-bearing
    // _res values are consumed by the payload builders below.
    let direct_picture_id = profile_res.as_ref().ok().and_then(|a| a.picture.clone());
    let top_track_picture_id = top_res.as_ref().ok().and_then(|tr| {
        tr.items
            .iter()
            .filter(|t| t.artist.id == tidal_artist_id)
            .find_map(|t| t.artist.picture.clone())
    });
    let album_cover_picture_id = [&albums_res, &eps_res, &comps_res, &live_res]
        .iter()
        .filter_map(|res| res.as_ref().ok())
        .flat_map(|list| list.iter())
        .find_map(|a| a.cover.clone());

    let direct_some = direct_picture_id.is_some();
    let top_track_some = top_track_picture_id.is_some();
    let album_cover_some = album_cover_picture_id.is_some();
    // TIDAL's CDN ships `640x640.jpg` reliably for album covers but not
    // for artist pictures — many artist images are stored at 320 max.
    // Pick the size that matches whichever tier resolved.
    let (resolved_picture_id, picture_size) = if let Some(id) = direct_picture_id {
        (Some(id), 320)
    } else if let Some(id) = top_track_picture_id {
        (Some(id), 320)
    } else if let Some(id) = album_cover_picture_id {
        (Some(id), 640)
    } else {
        (None, 320)
    };
    let picture_url = TidalClient::get_artwork_url(&resolved_picture_id, picture_size);
    if let Err(e) = profile_res.as_ref() {
        tracing::debug!(
            "TIDAL artist {} profile fetch failed: {}",
            tidal_artist_id,
            e
        );
    }
    tracing::warn!(
        "TIDAL artist {} picture resolution: direct={}, top_track={}, album_cover={}, resolved={}",
        tidal_artist_id,
        direct_some,
        top_track_some,
        album_cover_some,
        picture_url.is_some()
    );

    // TIDAL can return the same release under multiple filters (e.g. an album
    // re-issue tagged both ALBUMS and COMPILATIONS). Dedupe by tidal_id while
    // preserving the order of first appearance.
    let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut all_albums: Vec<crate::services::tidal::client::TidalAlbum> = Vec::new();
    for r in [albums_res, eps_res, comps_res, live_res]
        .into_iter()
        .flatten()
    {
        for item in r {
            if seen.insert(item.id) {
                all_albums.push(item);
            }
        }
    }

    let tidal_album_ids: Vec<i64> = all_albums.iter().map(|a| a.id).collect();
    let known_map = {
        let s = state.read().await;
        s.db.with_conn(|conn| queries::get_known_album_tidal_ids(conn, &tidal_album_ids))
            .unwrap_or_default()
    };

    let albums_payload: Vec<Value> = all_albums
        .into_iter()
        .map(|a| {
            let artwork =
                crate::services::tidal::client::TidalClient::get_artwork_url(&a.cover, 320);
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
                        .and_then(|c| {
                            crate::services::tidal::client::TidalClient::get_artwork_url(
                                &Some(c.clone()),
                                160,
                            )
                        });
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

    let videos_payload: Vec<Value> = videos_res
        .map(|r| {
            r.items
                .into_iter()
                .map(|v| {
                    let artwork = crate::services::tidal::client::TidalClient::get_artwork_url(
                        &v.image_id,
                        320,
                    );
                    let artist_name = v.artist.map(|a| a.name);
                    json!({
                        "tidal_id": v.id,
                        "title": v.title,
                        "duration_ms": v.duration * 1000,
                        "artwork_url": artwork,
                        "artist_name": artist_name,
                        "album_tidal_id": v.album.as_ref().map(|al| al.id),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // Resolve `local_id` per similar artist via the same lookup pattern used
    // for albums above — lets the frontend route /artists/[local_id] when
    // present (preserving library-affordances) and /tidal/artists/[id] otherwise.
    let similar_items: Vec<crate::services::tidal::client::TidalArtist> =
        similar_res.map(|r| r.items).unwrap_or_default();
    let similar_tidal_ids: Vec<i64> = similar_items.iter().map(|a| a.id).collect();
    let similar_known_map = {
        let s = state.read().await;
        s.db.with_conn(|conn| queries::get_known_artist_tidal_ids(conn, &similar_tidal_ids))
            .unwrap_or_default()
    };
    let similar_artists_payload: Vec<Value> = similar_items
        .into_iter()
        .map(|a| {
            let artwork =
                crate::services::tidal::client::TidalClient::get_artwork_url(&a.picture, 320);
            let local_id = similar_known_map.get(&a.id).copied();
            json!({
                "tidal_id": a.id,
                "local_id": local_id,
                "name": a.name,
                "artwork_url": artwork,
                "in_library": local_id.is_some(),
            })
        })
        .collect();

    let bio_payload = bio_res.ok().map(|b| {
        json!({
            "summary": b.summary,
            "text": b.text,
            "source": b.source,
        })
    });

    // Best-effort persistence of bio text to the local artists row so the
    // page can render it offline next time. Only writes when the local row
    // had no biography of its own.
    if let Some(b) = bio_payload.as_ref() {
        let bio_str = b
            .get("text")
            .and_then(|v| v.as_str())
            .or_else(|| b.get("summary").and_then(|v| v.as_str()))
            .map(|s| s.to_string());
        if let Some(text) = bio_str {
            let s = state.read().await;
            let _ = s.db.with_conn(|conn| {
                conn.execute(
                    "UPDATE artists SET biography = ?1
                     WHERE id = ?2 AND (biography IS NULL OR biography = '')",
                    rusqlite::params![text, id],
                )?;
                Ok(())
            });
        }
    }

    // Backfill the local artists row's `photo_url` so other surfaces in the
    // app (Library Recently Played Artists, search results, etc.) get the
    // working URL too. Older sync runs sometimes stored sizes TIDAL no
    // longer serves (e.g. 640x640 returning AccessDenied for some artists);
    // overwriting whenever the resolved URL differs keeps the cache fresh.
    if let Some(url) = picture_url.as_deref() {
        let s = state.read().await;
        let _ = s.db.with_conn(|conn| {
            conn.execute(
                "UPDATE artists SET photo_url = ?1
                 WHERE id = ?2 AND (photo_url IS NULL OR photo_url != ?1)",
                rusqlite::params![url, id],
            )?;
            Ok(())
        });
    }

    Ok(Json(json!({
        "albums": albums_payload,
        "top_tracks": top_tracks_payload,
        "videos": videos_payload,
        "similar_artists": similar_artists_payload,
        "bio": bio_payload,
        "picture_url": picture_url,
        "available": true
    })))
}

async fn get_artist_spotify_stats(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Json<Value> {
    let (enabled, artist_name, isrc_pairs) = {
        let s = state.read().await;
        let enabled = s.spotify_public_stats_enabled;
        if !enabled {
            return Json(json!({ "monthly_listeners": null, "tracks": [] }));
        }
        let pairs =
            s.db.with_conn(|conn| queries::get_artist_tracks(conn, id))
                .map(|tracks| {
                    let mut sorted = tracks
                        .into_iter()
                        .filter(|t| t.isrc.as_deref().is_some_and(|s| !s.is_empty()))
                        .collect::<Vec<_>>();
                    sorted.sort_by(|a, b| b.play_count.cmp(&a.play_count));
                    sorted.truncate(10);
                    sorted
                        .into_iter()
                        .map(|t| (t.isrc.unwrap_or_default(), t.title))
                        .collect::<Vec<(String, String)>>()
                })
                .unwrap_or_default();
        let artist_name = pairs.first().map(|_| String::new()).unwrap_or_default();
        // Re-look up the artist's display name (any track's artist_name works
        // — they all share artist_id=id by construction).
        let name =
            s.db.with_conn(|conn| queries::get_artist_tracks(conn, id))
                .ok()
                .and_then(|ts| ts.first().and_then(|t| t.artist_name.clone()))
                .unwrap_or(artist_name);
        (enabled, name, pairs)
    };

    let result =
        crate::services::spotify_public::fetch_artist_stats(enabled, &artist_name, &isrc_pairs)
            .await;

    Json(json!({
        "monthly_listeners": result.monthly_listeners,
        "tracks": result.tracks,
    }))
}

async fn get_tidal_album_tracks(
    State(state): State<SharedState>,
    Path(tidal_album_id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (tokens, tidal_http_client) = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;
        let s = state.read().await;
        (
            s.tidal_tokens.clone().or(persisted),
            s.tidal_http_client.clone(),
        )
    };

    let Some(tokens) = tokens else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let client = TidalClient::with_http(
        tidal_http_client,
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let result = client.get_album_tracks(tidal_album_id).await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": e.to_string() })),
        )
    })?;

    let tracks: Vec<Value> = result
        .items
        .into_iter()
        .map(|t| {
            let artwork = t
                .album
                .as_ref()
                .and_then(|al| al.cover.as_ref())
                .and_then(|c| {
                    crate::services::tidal::client::TidalClient::get_artwork_url(
                        &Some(c.clone()),
                        160,
                    )
                });
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
            })
        })
        .collect();

    Ok(Json(json!({ "tracks": tracks })))
}

async fn import_tidal_album(
    State(state): State<SharedState>,
    Path(tidal_album_id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (tokens, db, tidal_http_client) = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;
        let s = state.read().await;
        (
            s.tidal_tokens.clone().or(persisted),
            s.db.clone(),
            s.tidal_http_client.clone(),
        )
    };

    let Some(tokens) = tokens else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let client = TidalClient::with_http(
        tidal_http_client,
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let imported = tidal_import::import_album(&db, &client, tidal_album_id)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": e.to_string() })),
            )
        })?;

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
    album_tidal_id: Option<i64>,
    artwork_url: Option<String>,
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
        tidal_import::ImportTrackMetadata {
            tidal_id: body.tidal_id,
            title: body.title,
            artist_name: body.artist_name,
            artist_tidal_id: body.artist_tidal_id,
            artist_picture: None,
            album_title: body.album_title,
            album_tidal_id: body.album_tidal_id,
            album_artwork_url: body.artwork_url,
            duration_ms: body.duration_ms,
        },
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

#[derive(Debug, Deserialize)]
struct GenreListParams {
    /// See `GenreTrackParams::filter`.
    filter: Option<String>,
}

async fn get_genres(
    State(state): State<SharedState>,
    Query(params): Query<GenreListParams>,
) -> Result<Json<Value>, StatusCode> {
    let filter = crate::genre::filter::GalaxyFilterRule::from_query(params.filter.as_deref());
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let genres = queries::get_genre_tree_filtered(conn, filter)?;
            Ok(Json(json!({
                "genres": genres,
                "filter": filter.label().as_ref(),
            })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_genre_heat(
    State(state): State<SharedState>,
    Query(params): Query<GenreHeatParams>,
) -> Result<Json<Value>, StatusCode> {
    let days = params.days.unwrap_or(90).max(1);
    let filter = crate::genre::filter::GalaxyFilterRule::from_query(params.filter.as_deref());
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let heat = queries::get_genre_heat_filtered(conn, days, filter)?;
            Ok(Json(json!({
                "heat": heat,
                "filter": filter.label().as_ref(),
            })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[derive(Debug, Deserialize)]
struct GenreCoOccurrenceParams {
    days: Option<i64>,
    window_minutes: Option<i64>,
    min_count: Option<i64>,
    /// See `GenreTrackParams::filter`.
    filter: Option<String>,
}

async fn get_genre_co_occurrence(
    State(state): State<SharedState>,
    Query(params): Query<GenreCoOccurrenceParams>,
) -> Result<Json<Value>, StatusCode> {
    let days = params.days.unwrap_or(90).max(1);
    let window = params.window_minutes.unwrap_or(30).max(5);
    let min = params.min_count.unwrap_or(3).max(1);
    let filter = crate::genre::filter::GalaxyFilterRule::from_query(params.filter.as_deref());
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let pairs = queries::get_genre_co_occurrence_filtered(conn, days, window, min, filter)
                .map_err(|e| {
                    tracing::error!("co-occurrence query failed: {e:#}");
                    anyhow::anyhow!("co-occurrence query failed: {e:#}")
                })?;
            Ok(Json(json!({
                "pairs": pairs,
                "filter": filter.label().as_ref(),
            })))
        })
        .map_err(|e| {
            tracing::error!("co-occurrence handler error: {e:#}");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

#[derive(Debug, Deserialize)]
struct GenreCohortParams {
    days: Option<i64>,
    /// See `GenreTrackParams::filter`.
    filter: Option<String>,
}

async fn get_genre_cohorts(
    State(state): State<SharedState>,
    Query(params): Query<GenreCohortParams>,
) -> Result<Json<Value>, StatusCode> {
    let days = params.days.unwrap_or(90).max(1);
    let filter = crate::genre::filter::GalaxyFilterRule::from_query(params.filter.as_deref());
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            // HTTP endpoint preserves strict semantics — `?filter` controls
            // exactly what's matched. Fallback rescue is internal-only.
            let cohorts = queries::get_genre_cohorts_filtered(conn, days, filter, false)?;
            Ok(Json(json!({
                "cohorts": cohorts,
                "filter": filter.label().as_ref(),
            })))
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

async fn get_genre_audio_metrics(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let metrics = queries::get_genre_audio_metrics(conn)?;
            Ok(Json(json!({ "metrics": metrics })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn get_genre_tracks(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Query(params): Query<GenreTrackParams>,
) -> Result<Json<Value>, StatusCode> {
    let include_descendants = params.include_descendants.unwrap_or(true);
    let filter = crate::genre::filter::GalaxyFilterRule::from_query(params.filter.as_deref());
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let tracks =
                queries::get_tracks_by_genre_filtered(conn, id, include_descendants, filter)?;
            Ok(Json(json!({
                "tracks": tracks,
                "filter": filter.label().as_ref(),
            })))
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
    // Relaxed from 90 → 36500 so the new analytics page's "All" pill can pass through to
    // the legacy dashboard endpoint without silent truncation. Existing callers passing
    // ≤90 are unaffected.
    let days = params.days.unwrap_or(14).clamp(1, 36500);

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

async fn get_analytics_signals(
    State(state): State<SharedState>,
    Query(params): Query<AnalyticsSignalsParams>,
) -> Result<Json<Value>, StatusCode> {
    let days = params.days.unwrap_or(30).clamp(1, 36500);

    let state = state.read().await;
    let signals = state
        .db
        .with_conn(|conn| queries::get_analytics_signals(conn, days))
        .map_err(|err| {
            tracing::error!(?err, days, "get_analytics_signals failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(json!({ "signals": signals })))
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
                track_genres: queries::get_track_genre_paths_with_fallback(conn)?
                    .into_iter()
                    .map(|(id, rows)| (id, queries::ResolvedGenre::paths_only(&rows)))
                    .collect(),
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
    feed.results
        .sort_by(|left, right| right.score.cmp(&left.score));

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
    let playback_generation = bump_playback_generation(&state).await;
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
    let job =
        player::build_playback_preparation(&track, Some(&stream_info), crossfade_ms, user_quality)
            .with_generation(playback_generation);
    align_device_to_stream_rate(&state, &runtime_handle, &stream_info).await;
    runtime_handle.play(job).map_err(|error| {
        let message = format!("Failed to start host audio playback: {error}");
        report_playback_failure(&state, &message);
        if let Ok(state_guard) = state.try_read() {
            let _ = state_guard.db.with_conn(player::pause);
        }
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
    feed.results
        .sort_by(|left, right| right.score.cmp(&left.score));

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
        "corpus",
        "behavioral",
        "audio",
        "fusion",
        "neighbors",
        "evaluate",
    ];
    const STAGE_THRESHOLDS: &[f64] = &[0.05, 0.2, 0.55, 0.72, 0.88, 0.96];

    let stages: Vec<Value> = if let Some(ref r) = run {
        let current_stage_idx = STAGE_ORDER.iter().position(|&s| s == r.stage).unwrap_or(0);
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
        .with_conn(queries::get_latest_training_run)
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

async fn get_discovery_intensity(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::learning::{DiscoveryIntensity, load_discovery_intensity};
    let s = state.read().await;
    let intensity = load_discovery_intensity(&s.db);
    let params = intensity.params();
    Ok(Json(json!({
        "intensity": intensity.as_str(),
        "dimension": params.dimension,
        "top_k": params.top_k,
        "window_size": params.window_size,
        "include_audio_proxy": params.include_audio_proxy,
        "available": [
            DiscoveryIntensity::Max.as_str(),
            DiscoveryIntensity::Medium.as_str(),
            DiscoveryIntensity::Low.as_str(),
        ],
    })))
}

#[derive(Debug, Deserialize)]
struct IntensityRequest {
    intensity: String,
}

async fn set_discovery_intensity(
    State(state): State<SharedState>,
    Json(payload): Json<IntensityRequest>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::learning::{
        DiscoveryIntensity, set_discovery_intensity as save_intensity,
    };
    let s = state.read().await;
    let parsed = DiscoveryIntensity::parse(&payload.intensity);
    save_intensity(&s.db, parsed).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "intensity": parsed.as_str() })))
}

// Safety estimate: tells the UI how long training is expected to take and
// how much memory it'll claim, derived from the current track count, the
// active intensity tier, and the duration of the most recent successful run
// (if any). Frontend uses this to gate the user with a "this'll take ~X min"
// preview before they hit Start.
async fn get_discovery_safety(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    use crate::services::learning::load_discovery_intensity;
    let s = state.read().await;

    let (track_count, last_run_seconds): (i64, Option<f64>) =
        s.db.with_conn(|conn| {
            let tracks: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM tracks WHERE source IS NOT NULL",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            // Most recent finished run, in seconds. SQLite's strftime epoch
            // gives integer seconds; subtraction is the wall-clock duration.
            let last: Option<f64> = conn
                .query_row(
                    "SELECT (julianday(finished_at) - julianday(started_at)) * 86400.0
                     FROM training_runs
                     WHERE finished_at IS NOT NULL AND status = 'completed'
                     ORDER BY id DESC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .ok();
            Ok((tracks, last))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let intensity = load_discovery_intensity(&s.db);
    let params = intensity.params();

    // Cost model: similarity_neighbors is O(n²) on track count. Constant
    // factor scales roughly with `dim × top_k`. Calibrated against the
    // observed Max-tier baseline of ~12 minutes for 30,000 tracks (~0.8μs
    // per pair on a typical laptop). Final fudge factor includes I/O,
    // co-occurrence build, and audio-proxy overhead.
    let n = track_count as f64;
    let pair_cost_ns = 800.0 * (params.dimension as f64 / 96.0) * (params.top_k as f64 / 64.0);
    let neighbors_seconds = (n * n * pair_cost_ns) / 1.0e9;
    let audio_seconds = if params.include_audio_proxy {
        n * 0.0008
    } else {
        0.0
    };
    let behavioral_seconds = n * 0.001;
    let estimated_seconds_model = neighbors_seconds + audio_seconds + behavioral_seconds;

    // Prefer the actual last-run duration if we have one — it captures the
    // user's real machine and library. Blend 70/30 with the model so we
    // don't anchor too hard on a single noisy datapoint.
    let estimated_seconds = match last_run_seconds {
        Some(observed) if observed > 5.0 => 0.3 * estimated_seconds_model + 0.7 * observed,
        _ => estimated_seconds_model,
    };

    // Peak RAM rough estimate: dim × N × 8 bytes for behavioral vectors,
    // doubled for audio + fusion, plus the neighbor graph (top_k × N × 32).
    let ram_mb = ((params.dimension as f64 * n * 8.0 * 3.0) + (params.top_k as f64 * n * 32.0))
        / (1024.0 * 1024.0);

    // Safety classification: Green when the run is short or matches a known
    // baseline. Yellow when we expect 5-20 min on a non-trivial library.
    // Red when we predict over 20 min or RAM crosses 1.5 GB — these are the
    // cases where the user should consider dropping intensity.
    let recommendation = if estimated_seconds > 1200.0 || ram_mb > 1500.0 {
        "high_cost"
    } else if estimated_seconds > 300.0 {
        "moderate"
    } else {
        "safe"
    };

    Ok(Json(json!({
        "track_count": track_count,
        "intensity": intensity.as_str(),
        "estimated_seconds": estimated_seconds.round() as i64,
        "estimated_minutes": (estimated_seconds / 60.0 * 10.0).round() / 10.0,
        "estimated_ram_mb": ram_mb.round() as i64,
        "last_run_seconds": last_run_seconds.map(|s| s.round() as i64),
        "recommendation": recommendation,
        "params": {
            "dimension": params.dimension,
            "top_k": params.top_k,
            "window_size": params.window_size,
            "include_audio_proxy": params.include_audio_proxy,
        },
    })))
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
    seed_tidal_id: Option<i64>, // resolve to local library track when seed_track_id <= 0
    creativity: Option<f64>,    // 0.0 (tight) to 1.0 (adventurous), default 0.3
    context_window: Option<i64>, // number of recent tracks to influence, default 5
    limit: Option<i64>,         // results to return, default 20
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
                Ok(conn
                    .query_row(
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
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    Json(json!({"error": "No local track matches that Tidal ID"})),
                )
            })?
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
    })? {
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
        .with_conn(|conn| queries::get_similar_tracks(conn, seed_track_id, limit * 3, &exclude_ids))
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
        .with_conn(queries::get_similarity_computed_at)
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
        match db.with_conn(queries::compute_track_similarity) {
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

/// Stable synthetic id for an external (Last.fm) candidate that has no resolved
/// Tidal id. Negative i64 keyed off `artist|title` so multiple unresolved hits
/// don't all collapse onto the same `track-0` node on the canvas. Hash collisions
/// are negligible at the ~60-candidate scale of a single radio request.
///
/// TODO(option 2): Replace this with real Tidal-search resolution in `radio.rs`
/// before the candidate leaves the orchestrator — that would also let
/// `DiscoverSidePanel.resolveExternalPlayable` go away. Needs an artist+title →
/// tidal_id cache (in-memory or a small SQLite table) to avoid hammering the
/// Tidal API on every discovery request.
fn synthetic_external_track_id(artist: &str, title: &str) -> i64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    artist.hash(&mut h);
    "|".hash(&mut h);
    title.hash(&mut h);
    // Force into the negative i64 range so it can't collide with library ids
    // (always positive) or with the legacy `0` placeholder.
    -(((h.finish() & 0x7FFF_FFFF_FFFF_FFFF) | 1) as i64)
}

#[derive(Debug, Deserialize)]
struct DiscoverySpaceRequest {
    mode: Option<String>,
    seed_track_id: Option<i64>,
    prompt: Option<String>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ResolveTidalTrackQuery {
    spotify_id: String,
    /// When true, ignore any cached resolution and re-run the matcher.
    #[serde(default)]
    refresh: bool,
}

/// Resolve one Spotify (Sportify) track to TIDAL for playback. Reads the
/// Spotify→TIDAL map cache first; on miss, fetches Sportify metadata and
/// runs the title/artist/duration matcher against TIDAL search.
///
/// Response shape mirrors the `tidal: {...}` block on the normalized
/// DiscoveryTrack so the frontend can drop it straight in.
async fn resolve_tidal_track(
    State(state): State<SharedState>,
    Query(params): Query<ResolveTidalTrackQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use crate::services::sportify::{cache as sp_cache, resolver};

    let spotify_id = params.spotify_id.trim();
    if spotify_id.is_empty() {
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

    if !params.refresh {
        let cached = db
            .with_conn(|conn| sp_cache::get_tidal_resolution(conn, &cache_cfg, spotify_id))
            .map_err(internal)?;
        if let Some(hit) = cached {
            return Ok(Json(json!({
                "spotify_id": spotify_id,
                "tidal": {
                    "status": resolver::classify(hit.confidence).as_str(),
                    "id": hit.tidal_track_id,
                    "confidence": hit.confidence,
                    "match_reason": hit.match_reason,
                    "resolved_at": hit.resolved_at,
                    "from_cache": true,
                }
            })));
        }
        let unresolved = db
            .with_conn(|conn| sp_cache::get_unresolved(conn, spotify_id))
            .map_err(internal)?;
        if let Some(record) = unresolved.as_ref()
            && sp_cache::unresolved_is_cold(record, &cache_cfg)
        {
            return Ok(Json(json!({
                "spotify_id": spotify_id,
                "tidal": {
                    "status": "unresolved",
                    "id": null,
                    "confidence": 0.0,
                    "match_reason": record.reason,
                    "last_attempt_at": record.last_attempt_at,
                    "attempts": record.attempts,
                    "from_cache": true,
                }
            })));
        }
    }

    let sportify_track = match db
        .with_conn(|conn| sp_cache::get_track_meta(conn, &cache_cfg, spotify_id))
        .map_err(internal)?
    {
        Some(t) => t,
        None => {
            let fetched = sportify_client.track(spotify_id).await.map_err(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": format!("sportify_track_fetch: {e}") })),
                )
            })?;
            db.with_conn(|conn| {
                sp_cache::put_track_meta(conn, spotify_id, &fetched)?;
                crate::services::sportify::stats::write_track_playcount(conn, &fetched);
                Ok::<_, anyhow::Error>(())
            })
            .map_err(internal)?;
            fetched
        }
    };

    let (tokens, tidal_http_client) = {
        let persisted = load_persisted_tidal_tokens(&state)
            .await
            .map_err(internal)?;
        let s = state.read().await;
        (
            s.tidal_tokens.clone().or(persisted),
            s.tidal_http_client.clone(),
        )
    };
    let Some(tokens) = tokens else {
        return Ok(Json(json!({
            "spotify_id": spotify_id,
            "tidal": {
                "status": "error",
                "id": null,
                "confidence": 0.0,
                "match_reason": "tidal_not_connected",
            }
        })));
    };

    let tidal_client = TidalClient::with_http(
        tidal_http_client,
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let outcome = resolver::resolve_track(&tidal_client, &sportify_track)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("resolve: {e}") })),
            )
        })?;

    match outcome.status {
        resolver::ResolutionStatus::Resolved | resolver::ResolutionStatus::LowConfidence => {
            if let Some(tidal_id) = outcome.tidal_track_id {
                let reason = outcome.reason.clone();
                db.with_conn(|conn| {
                    sp_cache::put_tidal_resolution(
                        conn,
                        spotify_id,
                        tidal_id,
                        outcome.confidence,
                        Some(&reason),
                    )
                })
                .map_err(internal)?;
            }
        }
        resolver::ResolutionStatus::Unresolved => {
            let reason = outcome.reason.clone();
            db.with_conn(|conn| sp_cache::put_unresolved(conn, spotify_id, Some(&reason)))
                .map_err(internal)?;
        }
    }

    Ok(Json(json!({
        "spotify_id": spotify_id,
        "tidal": {
            "status": outcome.status.as_str(),
            "id": outcome.tidal_track_id,
            "confidence": outcome.confidence,
            "match_reason": outcome.reason,
            "from_cache": false,
        }
    })))
}

// ─── Sportify bulk + status resolution endpoints ────────────

#[derive(Debug, Deserialize)]
struct ResolveTidalBulkBody {
    spotify_ids: Vec<String>,
    /// When true, ignore any cached resolution for the given ids.
    #[serde(default)]
    refresh: bool,
}

/// Resolve a batch of Spotify ids in one request. Returns the cached state
/// for any ids that already had resolutions, plus fresh resolutions for the
/// rest. Caller may use this for a "resolve everything before opening" flow
/// or to force a refresh after a TIDAL session change.
async fn resolve_tidal_bulk(
    State(state): State<SharedState>,
    Json(body): Json<ResolveTidalBulkBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use crate::services::sportify::{cache as sp_cache, resolver};

    let ids: Vec<String> = body
        .spotify_ids
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if ids.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "spotify_ids required" })),
        ));
    }

    let (sportify_client, cache_cfg, resolve_cfg, db, tokens_in_state, tidal_http_client) = {
        let s = state.read().await;
        (
            s.sportify_client.clone(),
            s.sportify_cache_config,
            s.sportify_resolve_config,
            s.db.clone(),
            s.tidal_tokens.clone(),
            s.tidal_http_client.clone(),
        )
    };
    let Some(sportify_client) = sportify_client else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "sportify_unavailable" })),
        ));
    };

    // Partition: which ids do we need to actually resolve vs. read from cache.
    let mut resolved: Vec<Value> = Vec::new();
    let mut unresolved_payload: Vec<Value> = Vec::new();
    let mut needs_fetch: Vec<String> = Vec::new();
    db.with_conn(|conn| {
        for id in &ids {
            if !body.refresh {
                if let Some(hit) = sp_cache::get_tidal_resolution(conn, &cache_cfg, id)? {
                    resolved.push(json!({
                        "spotifyId": id,
                        "tidal": {
                            "status": resolver::classify(hit.confidence).as_str(),
                            "id": hit.tidal_track_id,
                            "confidence": hit.confidence,
                            "matchReason": hit.match_reason,
                            "fromCache": true,
                        }
                    }));
                    continue;
                }
                if let Some(record) = sp_cache::get_unresolved(conn, id)?
                    && sp_cache::unresolved_is_cold(&record, &cache_cfg)
                {
                    unresolved_payload.push(json!({
                        "spotifyId": id,
                        "tidal": {
                            "status": "unresolved",
                            "id": null,
                            "confidence": 0.0,
                            "matchReason": record.reason,
                            "attempts": record.attempts,
                            "fromCache": true,
                        }
                    }));
                    continue;
                }
            }
            needs_fetch.push(id.clone());
        }
        Ok::<_, anyhow::Error>(())
    })
    .map_err(internal)?;

    if needs_fetch.is_empty() {
        return Ok(Json(json!({
            "resolved": resolved,
            "unresolved": unresolved_payload,
        })));
    }

    // Fetch Sportify metadata for everything we need to resolve. Cache miss
    // → upstream call; failures fall through as `unresolved` rather than
    // failing the whole batch.
    let mut to_resolve: Vec<(String, crate::services::sportify::models::SportifyTrack)> =
        Vec::with_capacity(needs_fetch.len());
    for id in &needs_fetch {
        let cached = db
            .with_conn(|conn| sp_cache::get_track_meta(conn, &cache_cfg, id))
            .map_err(internal)?;
        let track = match cached {
            Some(t) => t,
            None => match sportify_client.track(id).await {
                Ok(t) => {
                    let _ = db.with_conn(|conn| {
                        sp_cache::put_track_meta(conn, id, &t)?;
                        crate::services::sportify::stats::write_track_playcount(conn, &t);
                        Ok::<_, anyhow::Error>(())
                    });
                    t
                }
                Err(e) => {
                    unresolved_payload.push(json!({
                        "spotifyId": id,
                        "tidal": {
                            "status": "error",
                            "id": null,
                            "confidence": 0.0,
                            "matchReason": format!("sportify_fetch:{e}"),
                            "fromCache": false,
                        }
                    }));
                    continue;
                }
            },
        };
        to_resolve.push((id.clone(), track));
    }

    let tokens = match tokens_in_state {
        Some(t) => Some(t),
        None => load_persisted_tidal_tokens(&state)
            .await
            .map_err(internal)?,
    };
    let Some(tokens) = tokens else {
        for (id, _) in to_resolve {
            unresolved_payload.push(json!({
                "spotifyId": id,
                "tidal": {
                    "status": "error",
                    "id": null,
                    "confidence": 0.0,
                    "matchReason": "tidal_not_connected",
                    "fromCache": false,
                }
            }));
        }
        return Ok(Json(json!({
            "resolved": resolved,
            "unresolved": unresolved_payload,
        })));
    };

    let client = TidalClient::with_http(
        tidal_http_client,
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let outcomes = resolver::resolve_many(&client, &to_resolve, resolve_cfg.bulk_concurrency).await;

    db.with_conn(|conn| {
        for (id, outcome) in &outcomes {
            persist_outcome(conn, id, outcome);
        }
        Ok::<_, anyhow::Error>(())
    })
    .map_err(internal)?;

    for (id, outcome) in outcomes {
        let row = json!({
            "spotifyId": id,
            "tidal": {
                "status": outcome.status.as_str(),
                "id": outcome.tidal_track_id,
                "confidence": outcome.confidence,
                "matchReason": outcome.reason,
                "fromCache": false,
            }
        });
        match outcome.status {
            resolver::ResolutionStatus::Resolved | resolver::ResolutionStatus::LowConfidence => {
                resolved.push(row);
            }
            resolver::ResolutionStatus::Unresolved => unresolved_payload.push(row),
        }
    }

    Ok(Json(json!({
        "resolved": resolved,
        "unresolved": unresolved_payload,
    })))
}

#[derive(Debug, Deserialize)]
struct ResolveTidalStatusQuery {
    /// Comma-separated list of Spotify track ids.
    spotify_ids: String,
}

/// Cheap polling endpoint: reads the cache only, never hits TIDAL or
/// Sportify. The frontend's lazy-tail poller calls this every ~1.5s after
/// opening a list endpoint until everything is non-pending or it times out.
async fn resolve_tidal_status(
    State(state): State<SharedState>,
    Query(params): Query<ResolveTidalStatusQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use crate::services::sportify::{cache as sp_cache, resolver};

    let ids: Vec<String> = params
        .spotify_ids
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if ids.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "spotify_ids required" })),
        ));
    }

    let (cache_cfg, db) = {
        let s = state.read().await;
        (s.sportify_cache_config, s.db.clone())
    };

    let entries = db
        .with_conn(|conn| {
            let mut out: Vec<Value> = Vec::with_capacity(ids.len());
            for id in &ids {
                if let Some(hit) = sp_cache::get_tidal_resolution(conn, &cache_cfg, id)? {
                    out.push(json!({
                        "spotifyId": id,
                        "tidal": {
                            "status": resolver::classify(hit.confidence).as_str(),
                            "id": hit.tidal_track_id,
                            "confidence": hit.confidence,
                            "matchReason": hit.match_reason,
                            "fromCache": true,
                        }
                    }));
                    continue;
                }
                if let Some(record) = sp_cache::get_unresolved(conn, id)?
                    && sp_cache::unresolved_is_cold(&record, &cache_cfg)
                {
                    out.push(json!({
                        "spotifyId": id,
                        "tidal": {
                            "status": "unresolved",
                            "id": null,
                            "confidence": 0.0,
                            "matchReason": record.reason,
                            "fromCache": true,
                        }
                    }));
                    continue;
                }
                out.push(json!({
                    "spotifyId": id,
                    "tidal": {
                        "status": "pending",
                        "id": null,
                        "confidence": 0.0,
                        "fromCache": false,
                    }
                }));
            }
            Ok::<_, anyhow::Error>(out)
        })
        .map_err(internal)?;

    Ok(Json(json!({ "entries": entries })))
}

// ─── Sportify discovery read endpoints ──────────────────────

/// Resolve the first `eager_n` Spotify tracks against TIDAL inline (so the
/// top of the response is instantly playable) and spawn a background task
/// for the remainder. Both paths persist into `sportify_track_map` /
/// `sportify_unresolved`, so a follow-up `enrich_tracks_with_tidal_cache`
/// call reflects the inline resolutions in the response.
///
/// Returns the list of spotify_ids spawned for lazy resolution — surfaced in
/// the response so the frontend's status poller knows what to watch.
async fn eager_and_lazy_resolve_for_list(
    state: &SharedState,
    sportify_tracks: &[crate::services::sportify::models::SportifyTrack],
) -> Vec<String> {
    use crate::services::sportify::{cache as sp_cache, resolver};

    let (cache_cfg, resolve_cfg, db, tidal_tokens_in_state, tidal_http_client) = {
        let s = state.read().await;
        (
            s.sportify_cache_config,
            s.sportify_resolve_config,
            s.db.clone(),
            s.tidal_tokens.clone(),
            s.tidal_http_client.clone(),
        )
    };

    // Filter to entries that need resolution: they have a spotify_id, no
    // cached resolution, and aren't sitting in a fresh negative-cache row.
    let needs_resolve: Vec<(String, crate::services::sportify::models::SportifyTrack)> = {
        let pairs: Vec<(String, crate::services::sportify::models::SportifyTrack)> =
            sportify_tracks
                .iter()
                .filter_map(|t| t.id.clone().map(|id| (id, t.clone())))
                .collect();
        let cache_check = db.with_conn(|conn| {
            let mut keep = Vec::with_capacity(pairs.len());
            for (id, track) in pairs.iter() {
                if sp_cache::get_tidal_resolution(conn, &cache_cfg, id)?.is_some() {
                    continue;
                }
                if let Some(record) = sp_cache::get_unresolved(conn, id)?
                    && sp_cache::unresolved_is_cold(&record, &cache_cfg)
                {
                    continue;
                }
                keep.push((id.clone(), track.clone()));
            }
            Ok::<_, anyhow::Error>(keep)
        });
        match cache_check {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("eager_and_lazy_resolve cache check failed: {}", e);
                return Vec::new();
            }
        }
    };

    if needs_resolve.is_empty() {
        return Vec::new();
    }

    // Without TIDAL credentials we can't resolve anything. Leave rows pending
    // so the UI shows them as such — better than persisting bogus failures.
    let tokens = match tidal_tokens_in_state {
        Some(t) => Some(t),
        None => match load_persisted_tidal_tokens(state).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("eager_and_lazy_resolve token load failed: {}", e);
                None
            }
        },
    };
    let Some(tokens) = tokens else {
        return Vec::new();
    };
    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );

    let eager_count = needs_resolve.len().min(resolve_cfg.eager_n);
    let (eager, lazy) = needs_resolve.split_at(eager_count);

    // Eager pass: resolve inline and persist before returning.
    if !eager.is_empty() {
        let outcomes = resolver::resolve_many(&client, eager, resolve_cfg.bulk_concurrency).await;
        let _ = db.with_conn(|conn| {
            for (id, outcome) in &outcomes {
                persist_outcome(conn, id, outcome);
            }
            Ok::<_, anyhow::Error>(())
        });
    }

    // Lazy pass: spawn detached task. The cache writes show up in the next
    // status-poll round trip from the frontend.
    let lazy_ids: Vec<String> = lazy.iter().map(|(id, _)| id.clone()).collect();
    if !lazy.is_empty() {
        let lazy_owned = lazy.to_vec();
        let db_lazy = db.clone();
        let concurrency = resolve_cfg.bulk_concurrency;
        tokio::spawn(async move {
            let outcomes = resolver::resolve_many(&client, &lazy_owned, concurrency).await;
            let _ = db_lazy.with_conn(|conn| {
                for (id, outcome) in &outcomes {
                    persist_outcome(conn, id, outcome);
                }
                Ok::<_, anyhow::Error>(())
            });
        });
    }

    lazy_ids
}

/// Playlist pages need a fast first paint: return cache-only rows immediately
/// and resolve every remaining Spotify track in the background.
async fn spawn_background_resolve_for_list(
    state: &SharedState,
    sportify_tracks: &[crate::services::sportify::models::SportifyTrack],
) -> Vec<String> {
    use crate::services::sportify::{cache as sp_cache, resolver};

    let (cache_cfg, resolve_cfg, db, tidal_tokens_in_state, tidal_http_client) = {
        let s = state.read().await;
        (
            s.sportify_cache_config,
            s.sportify_resolve_config,
            s.db.clone(),
            s.tidal_tokens.clone(),
            s.tidal_http_client.clone(),
        )
    };

    let needs_resolve: Vec<(String, crate::services::sportify::models::SportifyTrack)> = {
        let pairs: Vec<(String, crate::services::sportify::models::SportifyTrack)> =
            sportify_tracks
                .iter()
                .filter_map(|t| t.id.clone().map(|id| (id, t.clone())))
                .collect();
        match db.with_conn(|conn| {
            let mut keep = Vec::with_capacity(pairs.len());
            for (id, track) in pairs.iter() {
                if sp_cache::get_tidal_resolution(conn, &cache_cfg, id)?.is_some() {
                    continue;
                }
                if let Some(record) = sp_cache::get_unresolved(conn, id)?
                    && sp_cache::unresolved_is_cold(&record, &cache_cfg)
                {
                    continue;
                }
                keep.push((id.clone(), track.clone()));
            }
            Ok::<_, anyhow::Error>(keep)
        }) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("background Sportify resolve cache check failed: {}", e);
                return Vec::new();
            }
        }
    };

    if needs_resolve.is_empty() {
        return Vec::new();
    }

    let pending_ids: Vec<String> = needs_resolve.iter().map(|(id, _)| id.clone()).collect();
    let tokens = match tidal_tokens_in_state {
        Some(t) => Some(t),
        None => match load_persisted_tidal_tokens(state).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("background Sportify resolve token load failed: {}", e);
                None
            }
        },
    };
    let Some(tokens) = tokens else {
        return Vec::new();
    };

    tokio::spawn(async move {
        let client = TidalClient::with_http(
            tidal_http_client,
            tokens.access_token.clone(),
            tokens.country_code.clone(),
        );
        let outcomes =
            resolver::resolve_many(&client, &needs_resolve, resolve_cfg.bulk_concurrency).await;
        let _ = db.with_conn(|conn| {
            for (id, outcome) in &outcomes {
                persist_outcome(conn, id, outcome);
            }
            Ok::<_, anyhow::Error>(())
        });
    });

    pending_ids
}

fn persist_outcome(
    conn: &rusqlite::Connection,
    spotify_id: &str,
    outcome: &crate::services::sportify::resolver::ResolutionOutcome,
) {
    use crate::services::sportify::{cache as sp_cache, resolver::ResolutionStatus};
    match outcome.status {
        ResolutionStatus::Resolved | ResolutionStatus::LowConfidence => {
            if let Some(tidal_id) = outcome.tidal_track_id {
                let _ = sp_cache::put_tidal_resolution(
                    conn,
                    spotify_id,
                    tidal_id,
                    outcome.confidence,
                    Some(&outcome.reason),
                );
            }
        }
        ResolutionStatus::Unresolved => {
            let _ = sp_cache::put_unresolved(conn, spotify_id, Some(&outcome.reason));
        }
    }
}

#[derive(Debug, Deserialize)]
struct SportifySearchQuery {
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
        // Default to track when missing or unknown — matches Sportify's own
        // most-common usage.
        _ => SportifySearchKind::Track,
    }
}

async fn sportify_discovery_search(
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
        .map_err(internal)?;

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
                .map_err(internal)?;
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
    .map_err(internal)?;

    Ok(Json(serde_json::to_value(normalized).unwrap_or(json!({}))))
}

async fn sportify_discovery_track(
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
        .map_err(internal)?
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
            .map_err(internal)?;
            fetched
        }
    };

    let mut row = normalize::track_from_sportify(&track, "sportify_track");
    db.with_conn(|conn| {
        normalize::enrich_tracks_with_tidal_cache(conn, &cache_cfg, std::slice::from_mut(&mut row))
    })
    .map_err(internal)?;

    Ok(Json(serde_json::to_value(row).unwrap_or(json!({}))))
}

async fn sportify_discovery_album(
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
        .map_err(internal)?
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
            .map_err(internal)?;
            fetched
        }
    };

    let mut row = normalize::album_from_sportify(&album, "sportify_album");
    let pending_ids = eager_and_lazy_resolve_for_list(&state, &album.tracks).await;
    db.with_conn(|conn| {
        normalize::enrich_tracks_with_tidal_cache(conn, &cache_cfg, &mut row.tracks)
    })
    .map_err(internal)?;

    Ok(Json(json!({
        "album": serde_json::to_value(row).unwrap_or(json!({})),
        "pendingSpotifyIds": pending_ids,
    })))
}

async fn sportify_discovery_playlist(
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
        .map_err(internal)?
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
            .map_err(internal)?;
            fetched
        }
    };

    let mut row = normalize::playlist_from_sportify(&playlist, "sportify_playlist");
    db.with_conn(|conn| {
        normalize::enrich_tracks_with_tidal_cache(conn, &cache_cfg, &mut row.tracks)
    })
    .map_err(internal)?;
    let pending_ids = spawn_background_resolve_for_list(&state, &playlist.tracks).await;

    Ok(Json(json!({
        "playlist": serde_json::to_value(row).unwrap_or(json!({})),
        "pendingSpotifyIds": pending_ids,
    })))
}

async fn sportify_discovery_artist(
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
        .map_err(internal)?
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
            .map_err(internal)?;
            fetched
        }
    };

    let row = normalize::artist_from_sportify(&artist);
    Ok(Json(serde_json::to_value(row).unwrap_or(json!({}))))
}

async fn sportify_discovery_artist_top_tracks(
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
    .map_err(internal)?;

    let mut rows: Vec<_> = tracks
        .iter()
        .map(|t| normalize::track_from_sportify(t, "sportify_artist_top_tracks"))
        .collect();
    let pending_ids = eager_and_lazy_resolve_for_list(&state, &tracks).await;
    db.with_conn(|conn| normalize::enrich_tracks_with_tidal_cache(conn, &cache_cfg, &mut rows))
        .map_err(internal)?;

    Ok(Json(json!({
        "spotifyId": id,
        "tracks": rows,
        "pendingSpotifyIds": pending_ids,
    })))
}

async fn sportify_discovery_artist_related(
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
    pending.extend(eager_and_lazy_resolve_for_list(&state, &related.top_tracks).await);
    pending.extend(eager_and_lazy_resolve_for_list(&state, &related.deep_cuts).await);

    db.with_conn(|conn| {
        normalize::enrich_tracks_with_tidal_cache(conn, &cache_cfg, &mut top_rows)?;
        normalize::enrich_tracks_with_tidal_cache(conn, &cache_cfg, &mut deep_rows)?;
        Ok::<_, anyhow::Error>(())
    })
    .map_err(internal)?;

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

async fn sportify_discovery_album_related(
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
    let pending = eager_and_lazy_resolve_for_list(&state, &related.more_from_artist).await;

    db.with_conn(|conn| {
        normalize::enrich_tracks_with_tidal_cache(conn, &cache_cfg, &mut more_from_artist)
    })
    .map_err(internal)?;

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

async fn sportify_discovery_track_related(
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
    pending.extend(eager_and_lazy_resolve_for_list(&state, &related.more_from_album).await);
    pending.extend(eager_and_lazy_resolve_for_list(&state, &related.more_from_artist).await);

    db.with_conn(|conn| {
        normalize::enrich_tracks_with_tidal_cache(conn, &cache_cfg, &mut more_from_album)?;
        normalize::enrich_tracks_with_tidal_cache(conn, &cache_cfg, &mut more_from_artist)?;
        Ok::<_, anyhow::Error>(())
    })
    .map_err(internal)?;

    Ok(Json(json!({
        "spotifyId": id,
        "moreFromAlbum": more_from_album,
        "moreFromArtist": more_from_artist,
        "pendingSpotifyIds": pending,
    })))
}

#[derive(Debug, Deserialize)]
struct SaveSpotifyPlaylistBody {
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
/// resolution are skipped — we never invent a placeholder TIDAL id.
async fn save_spotify_playlist(
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
    // a cached hit are skipped here — the frontend should bulk-resolve before
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
        .map_err(internal)?;

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
        .map_err(internal)?;

    Ok(Json(json!({
        "playlist": result.0,
        "added": result.1,
        "totalTracks": total_tracks,
        "resolvedCount": resolved_count,
        "unresolvedCount": unresolved_count,
        "importFailures": import_failures,
    })))
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": e.to_string() })),
    )
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
        radio_source: Option<String>, // "library" | "lastfm" | "engine"
        radio_reason: Option<String>,
        // v1.5 fields
        confidence: f64,
        support_count: i64,
        primary_reason: String,
        reason_tags: Vec<String>,
        genres: Vec<String>,
        in_degree_pctile: f64,
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
        state_guard
            .db
            .with_conn(move |conn| {
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
                    track_genres: queries::get_track_genre_paths_with_fallback(conn)?
                        .into_iter()
                        .map(|(id, rows)| (id, queries::ResolvedGenre::paths_only(&rows)))
                        .collect(),
                };
                let candidates = queries::get_discovery_candidate_tracks(conn, lim * 4)?;
                let preview = discovery_engine::build_preview(&request, &context, &candidates);
                Ok(preview
                    .results
                    .into_iter()
                    .map(|r| SpaceTrack {
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
                        confidence: 1.0,
                        support_count: 0,
                        primary_reason: "unknown".to_string(),
                        reason_tags: vec![],
                        genres: vec![],
                        in_degree_pctile: 0.5,
                    })
                    .collect::<Vec<_>>())
            })
            .unwrap_or_default()
    } else if seed_id > 0 {
        let db = state_guard.db.clone();
        let lastfm = crate::metadata::lastfm::LastFmClient::load(
            state_guard.http_client.clone(),
            &state_guard.db,
        );
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
                    track_id: if c.is_in_library {
                        c.track_id
                    } else {
                        c.tidal_track_id.unwrap_or_else(|| {
                            synthetic_external_track_id(&c.artist_name, &c.title)
                        })
                    },
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
                    confidence: c
                        .confidence
                        .unwrap_or(if c.is_in_library { 1.0 } else { 0.5 }),
                    support_count: c.support_count.unwrap_or(0),
                    primary_reason: ds::normalize_reason(c.primary_reason.as_deref().unwrap_or(""))
                        .to_string(),
                    reason_tags: c
                        .primary_reason
                        .as_deref()
                        .map(|r| vec![ds::normalize_reason(r).to_string()])
                        .unwrap_or_default(),
                    genres: vec![],
                    in_degree_pctile: c.candidate_in_degree_percentile.unwrap_or(0.5),
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
                space_tracks.insert(
                    0,
                    SpaceTrack {
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
                        confidence: 1.0,
                        support_count: 0,
                        primary_reason: "unknown".to_string(),
                        reason_tags: vec![],
                        genres: vec![],
                        in_degree_pctile: 0.5,
                    },
                );
            }
        }
    }

    // ── 2. Fill remainder from most-played library tracks ────────────────────
    // Only fill when browsing without a seed. In seed mode the radio candidates
    // ARE the map — padding with unrelated most-played tracks creates a cloud of
    // disconnected blue dots with no edges and falsely-cold-start labels.
    let seeded_ids: HashSet<i64> = space_tracks.iter().map(|t| t.track_id).collect();
    let remaining = limit - space_tracks.len() as i64;
    if remaining > 0 && seed_id == 0 {
        let fallback = state_guard
            .db
            .with_conn(|conn| {
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
                for r in rows {
                    result.push(r?);
                }
                Ok(result)
            })
            .unwrap_or_default();

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
                    confidence: 1.0,
                    support_count: 0,
                    primary_reason: "unknown".to_string(),
                    reason_tags: vec![],
                    genres: vec![],
                    in_degree_pctile: 0.5,
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
                Option<f64>,    // energy
                Option<f64>,    // danceability
                Option<f64>,    // bpm
                Option<String>, // key_signature
                Option<String>, // camelot_key
                Option<i64>,    // is_instrumental (0/1)
                Option<f64>,    // loudness_lufs
            );
            let dsp_map: std::collections::HashMap<i64, DspRow> = state_guard
                .db
                .with_conn(|conn| {
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
                })
                .unwrap_or_default();

            for t in &mut space_tracks {
                if let Some((energy, dance, bpm, key, camelot, instr, lufs)) =
                    dsp_map.get(&t.track_id)
                {
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
            let track_meta: std::collections::HashMap<i64, (Option<String>, i64)> = state_guard
                .db
                .with_conn(|conn| {
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
                })
                .unwrap_or_default();

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
            // genre_map: track_id → (top_name, top_source, top_conf, all_names)
            type GenreEntry = (String, Option<String>, Option<f64>, Vec<String>);
            let genre_map: std::collections::HashMap<i64, GenreEntry> = state_guard
                .db
                .with_conn(|conn| {
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
                    let mut map: std::collections::HashMap<i64, GenreEntry> =
                        std::collections::HashMap::new();
                    for r in rows {
                        let (id, name, source, conf) = r?;
                        let entry = map
                            .entry(id)
                            .or_insert_with(|| (name.clone(), source.clone(), conf, vec![]));
                        entry.3.push(name);
                    }
                    Ok(map)
                })
                .unwrap_or_default();

            for t in &mut space_tracks {
                if let Some((name, source, conf, all_genres)) = genre_map.get(&t.track_id) {
                    t.top_genre = Some(name.clone());
                    t.top_genre_source = source.clone();
                    t.top_genre_confidence = *conf;
                    t.genres = all_genres.clone();
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
            let cohort_map: std::collections::HashMap<i64, (String, String)> = state_guard
                .db
                .with_conn(|conn| queries::get_track_cohort_assignments(conn, &track_ids, 90))
                .unwrap_or_default();

            for t in &mut space_tracks {
                if let Some((id, label)) = cohort_map.get(&t.track_id) {
                    t.cohort_id = Some(id.clone());
                    t.cohort_label = Some(label.clone());
                }
            }
        }
    }

    // ── 4. Build typed edges (v1.5) ──────────────────────────────────────────
    // Typed to feed the pruner and serialized after pruning. Old callers receive
    // extra fields they can ignore; all existing fields are preserved.
    struct FullEdge {
        from_track_id: i64,
        to_track_id: i64,
        weight: f64,
        confidence: f64,
        primary_reason: String,
        reason_tags: Vec<String>,
        source: String,
        support_count: Option<i64>,
        behavioral_score: f64,
        audio_score: f64,
        metadata_score: f64,
    }

    // Library↔library edges come from `track_neighbors`. We always run this
    // query when there's more than one library track in the result set so the
    // map shows the full neighbor graph, regardless of whether external tracks
    // are present.
    let mut typed_edges: Vec<FullEdge> = {
        let track_id_set: HashSet<i64> = space_tracks
            .iter()
            .filter(|t| t.is_in_library)
            .map(|t| t.track_id)
            .collect();
        if track_id_set.len() > 1 {
            let ids_csv: String = track_id_set
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            state_guard
                .db
                .with_conn(|conn| {
                    let sql = format!(
                        "SELECT n.track_id, n.neighbor_track_id, n.score,
                            n.behavioral_score, n.audio_score, n.metadata_score,
                            n.reason_json, n.confidence, n.support_count
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
                            row.get::<_, Option<f64>>(7)?,
                            row.get::<_, Option<i64>>(8)?,
                        ))
                    })?;
                    let mut result = Vec::new();
                    for r in rows {
                        result.push(r?);
                    }
                    Ok(result)
                })
                .unwrap_or_default()
                .into_iter()
                .map(
                    |(
                        from_id,
                        to_id,
                        score,
                        behavioral,
                        audio,
                        metadata,
                        reason_json,
                        confidence,
                        support_count,
                    )| {
                        let parsed: Vec<Value> = reason_json
                            .as_deref()
                            .and_then(|s| serde_json::from_str::<Vec<Value>>(s).ok())
                            .unwrap_or_default();
                        let raw_tags: Vec<String> = parsed
                            .iter()
                            .filter_map(|v| {
                                v.get("key")
                                    .and_then(|k| k.as_str())
                                    .or_else(|| v.get("label").and_then(|l| l.as_str()))
                                    .map(|s| s.to_string())
                            })
                            .collect();
                        let reason_tags = ds::normalize_reason_tags(&raw_tags);
                        let primary_reason = reason_tags
                            .first()
                            .cloned()
                            .unwrap_or_else(|| "unknown".to_string());
                        FullEdge {
                            from_track_id: from_id,
                            to_track_id: to_id,
                            weight: score.clamp(0.0, 1.0),
                            confidence: confidence.unwrap_or(0.5),
                            primary_reason,
                            reason_tags,
                            source: "library".to_string(),
                            support_count,
                            behavioral_score: behavioral,
                            audio_score: audio,
                            metadata_score: metadata,
                        }
                    },
                )
                .collect()
        } else {
            vec![]
        }
    };

    // External (non-library) tracks aren't in `track_neighbors`, so synthesize
    // a seed→external edge per external track. This runs alongside the library
    // edges above so users see both their library graph and the external links.
    if seed_id > 0 && prompt.is_empty() {
        for t in space_tracks.iter().filter(|t| !t.is_in_library) {
            let reason = ds::normalize_reason("external_match");
            typed_edges.push(FullEdge {
                from_track_id: seed_id,
                to_track_id: t.track_id,
                weight: t.similarity_score,
                confidence: t.confidence,
                primary_reason: reason.to_string(),
                reason_tags: vec![reason.to_string()],
                source: ds::normalize_source(&t.source).to_string(),
                support_count: Some(t.support_count),
                behavioral_score: 0.0,
                audio_score: 0.0,
                metadata_score: t.similarity_score,
            });
        }
    }

    // ── 5. Score normalization (per source group) ─────────────────────────────
    let score_candidates: Vec<ds::ScoreCandidate> = space_tracks
        .iter()
        .map(|t| ds::ScoreCandidate {
            track_id: t.track_id,
            raw_score: t.similarity_score,
            source: ds::normalize_source(&t.source).to_string(),
        })
        .collect();
    let norm_scores = ds::normalize_scores_by_source(&score_candidates);

    // ── 6. Within-set in-degree stats ────────────────────────────────────────
    let prune_edges: Vec<ds::PruneEdge> = typed_edges
        .iter()
        .map(|e| ds::PruneEdge {
            from_track_id: e.from_track_id,
            to_track_id: e.to_track_id,
            weight: e.weight,
            confidence: e.confidence,
        })
        .collect();
    let track_ids_for_deg: Vec<i64> = space_tracks.iter().map(|t| t.track_id).collect();
    let in_deg_stats = ds::compute_in_degree_stats(&track_ids_for_deg, &prune_edges);
    for t in &mut space_tracks {
        if let Some((_, pctile)) = in_deg_stats.get(&t.track_id) {
            t.in_degree_pctile = *pctile;
        }
    }

    // ── 7. Graph pruning ──────────────────────────────────────────────────────
    let prune_nodes: Vec<ds::PruneNode> = space_tracks
        .iter()
        .map(|t| ds::PruneNode {
            track_id: t.track_id,
            score: norm_scores
                .get(&t.track_id)
                .copied()
                .unwrap_or_else(|| t.similarity_score.clamp(0.0, 1.0)),
            is_seed: t.track_id == seed_id,
            primary_reason: t.primary_reason.clone(),
            in_degree_pctile: t.in_degree_pctile,
        })
        .collect();
    let prune_result = ds::prune_graph(
        prune_nodes,
        prune_edges,
        seed_id,
        &ds::PruneConfig::default(),
    );
    let surviving_ids: HashSet<i64> = prune_result.node_ids.iter().copied().collect();

    // Filter space_tracks to survivors; preserve original order.
    space_tracks.retain(|t| surviving_ids.contains(&t.track_id));

    // ── 8. Serialize nodes with v1.5 fields ──────────────────────────────────
    let total = space_tracks.len().max(1);
    let track_nodes: Vec<Value> = space_tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let norm_score = norm_scores
                .get(&t.track_id)
                .copied()
                .unwrap_or_else(|| t.similarity_score.clamp(0.0, 1.0));
            // Library tracks are only truly cold-start if confidence is very low —
            // support_count may be 0 simply because the neighbor table hasn't been
            // calculated yet, which doesn't mean there's no behavioral data.
            let is_cold_start = !t.is_in_library && (t.support_count == 0 || t.confidence < 0.3);
            let normalized_source = ds::normalize_source(&t.source);
            let cluster_key = t
                .genres
                .first()
                .or(t.top_genre.as_ref())
                .map(|s| s.as_str())
                .unwrap_or("unknown");

            let (x, y) = match mode.as_str() {
                "energy_arc" => {
                    let energy = t.energy.unwrap_or(0.5);
                    let jitter_x = (i as f64 * 17.3).sin() * 60.0;
                    let jitter_y = (i as f64 * 31.7).cos() * 200.0;
                    ((energy - 0.5) * 800.0 + jitter_x, jitter_y)
                }
                "harmonic" => {
                    if let Some(ref ck) = t.camelot_key {
                        let num = ck
                            .chars()
                            .take_while(|c| c.is_ascii_digit())
                            .collect::<String>()
                            .parse::<f64>()
                            .unwrap_or(1.0);
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
                    let r =
                        80.0 + (1.0 - t.similarity_score) * 300.0 + (i as f64 * 37.0).sin() * 50.0;
                    (angle.cos() * r, angle.sin() * r)
                }
            };
            let node_radius = 5.0 + t.similarity_score * 20.0 + t.energy.unwrap_or(0.5) * 5.0;
            let in_deg = in_deg_stats
                .get(&t.track_id)
                .map(|(d, _)| *d as i64)
                .unwrap_or(0);
            let layout_obj = json!({
                "x": x, "y": y,
                "radius_hint": node_radius,
                "cluster_key": cluster_key,
                "distance_from_seed": (1.0 - norm_score).clamp(0.0, 1.0),
            });
            // Build node object in two halves to avoid json! macro recursion limit.
            let mut node_obj = json!({
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
                "source": normalized_source,
                "radio_source": t.radio_source,
                "radio_reason": t.radio_reason,
                "x": x, "y": y, "vx": 0.0, "vy": 0.0,
                "radius": node_radius,
                "opacity": 0.0,
            });
            let v15 = json!({
                "id": format!("track-{}", t.track_id),
                "score": norm_score,
                "raw_score": t.similarity_score,
                "confidence": t.confidence,
                "support_count": t.support_count,
                "is_cold_start": is_cold_start,
                "primary_reason": t.primary_reason,
                "reason_tags": t.reason_tags,
                "genres": t.genres,
                "is_seed": t.track_id == seed_id,
                "candidate_in_degree": in_deg,
                "candidate_in_degree_percentile": t.in_degree_pctile,
                "layout": layout_obj,
            });
            if let (Some(obj), Some(ext)) = (node_obj.as_object_mut(), v15.as_object()) {
                obj.extend(ext.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
            node_obj
        })
        .collect();

    // ── 9. Serialize edges with v1.5 fields ──────────────────────────────────
    let edge_nodes: Vec<Value> = typed_edges
        .iter()
        .filter(|e| {
            surviving_ids.contains(&e.from_track_id) && surviving_ids.contains(&e.to_track_id)
        })
        .map(|e| {
            let edge_id = format!("{}-{}-{}", e.from_track_id, e.to_track_id, e.primary_reason);
            json!({
                // ── Existing fields ──
                "from_id": e.from_track_id,
                "to_id": e.to_track_id,
                "type": &e.primary_reason,
                "weight": e.weight,
                "reason_tags": &e.reason_tags,
                "behavioral_score": e.behavioral_score,
                "audio_score": e.audio_score,
                "metadata_score": e.metadata_score,
                // ── v1.5 fields ──
                "id": edge_id,
                "from_track_id": e.from_track_id,
                "to_track_id": e.to_track_id,
                "reason": &e.primary_reason,
                "primary_reason": &e.primary_reason,
                "confidence": e.confidence,
                "source": &e.source,
                "support_count": e.support_count,
            })
        })
        .collect();

    // ── 10. Diagnostics ───────────────────────────────────────────────────────
    let mut source_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut reason_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut conf_sum = 0.0f64;
    let mut pctile_sum = 0.0f64;
    for node in &track_nodes {
        let src = node["source"].as_str().unwrap_or("engine").to_string();
        *source_counts.entry(src).or_insert(0) += 1;
        let reason = node["primary_reason"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();
        *reason_counts.entry(reason).or_insert(0) += 1;
        conf_sum += node["confidence"].as_f64().unwrap_or(0.5);
        pctile_sum += node["candidate_in_degree_percentile"]
            .as_f64()
            .unwrap_or(0.0);
    }
    let n_nodes = track_nodes.len().max(1) as f64;
    let diagnostics = json!({
        "node_count": track_nodes.len(),
        "edge_count": edge_nodes.len(),
        "source_counts": source_counts,
        "reason_counts": reason_counts,
        "avg_confidence": conf_sum / n_nodes,
        "avg_in_degree_percentile": pctile_sum / n_nodes,
        "raw_candidate_count": prune_result.raw_node_count,
        "raw_edge_count": prune_result.raw_edge_count,
        "pruned_node_count": prune_result.pruned_node_count,
        "pruned_edge_count": prune_result.pruned_edge_count,
        "hub_suppressed_count": prune_result.hub_suppressed_count,
        "low_confidence_edge_dropped_count": prune_result.low_confidence_edge_dropped_count,
    });

    // ── 11. Background seed-neighbor refresh (DiscoverSpace only) ────────────
    // Fire-and-forget: computes embedding similarity for this seed, writes to
    // track_neighbors, then sends DiscoverySpaceRefreshed so the map auto-reloads.
    // `refreshed_seeds` is a TTL'd map keyed by (seed_id → model_id, instant) so
    // entries expire and re-training invalidates them automatically.
    if seed_id > 0 && prompt.is_empty() {
        let guard = state.read().await;
        // Best-effort: read current model_id outside the spawned task so we can
        // skip the spawn entirely when this seed is fresh under the same model.
        let active_model_id: Option<i64> = guard
            .db
            .with_conn(|conn| {
                Ok(crate::db::queries::get_active_embedding_model(conn)?.map(|m| m.id))
            })
            .unwrap_or(None);
        let already_fresh = match active_model_id {
            Some(mid) => crate::services::neighbor_refresh::is_seed_fresh(
                &guard.refreshed_seeds,
                seed_id,
                mid,
            ),
            None => true, // no model → nothing to do anyway
        };
        if !already_fresh {
            let db2 = guard.db.clone();
            let tx = guard.event_tx.clone();
            let refreshed = Arc::clone(&guard.refreshed_seeds);
            let cache = Arc::clone(&guard.embedding_cache);
            drop(guard);
            tokio::spawn(crate::services::neighbor_refresh::refresh_seed_neighbors(
                db2, tx, seed_id, refreshed, cache,
            ));
        }
    }

    Ok(Json(json!({
        "tracks": track_nodes,
        "edges": edge_nodes,
        "artists": [],
        "diagnostics": diagnostics,
        "seed_track_id": if seed_id > 0 { Some(seed_id) } else { None },
        "generated_at": chrono::Utc::now().to_rfc3339(),
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
        g.user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
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

    let (first_playable, pending_count) = build_radio_queue_and_spawn_resolvers(
        &state,
        &db,
        Some(payload.seed_track_id),
        queue.tracks.clone(),
        "radio_song",
    )
    .await?;
    let snapshot = start_first_radio_queue_item(&state).await?;
    let mut body = serde_json::to_value(queue).unwrap_or(json!({}));
    body["first_playable"] = first_playable;
    body["pending_count"] = json!(pending_count);
    body["state"] = json!(snapshot.state);
    body["queue"] = json!(snapshot.queue);
    Ok(Json(body))
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

async fn build_radio_queue_and_spawn_resolvers(
    state: &SharedState,
    db: &crate::db::Database,
    seed_track_id: Option<i64>,
    tracks: Vec<crate::services::radio::RadioCandidate>,
    context: &'static str,
) -> Result<(Value, usize), (StatusCode, Json<Value>)> {
    let build = db
        .with_conn(move |conn| {
            Ok(
                crate::server::radio_pipeline::build_radio_queue_from_candidates_with_seed(
                    conn,
                    seed_track_id,
                    tracks,
                )?,
            )
        })
        .map_err(|e| {
            tracing::error!("{context}: queue build failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to build queue" })),
            )
        })?;
    let first_item = build.first_item;
    let pending_item_ids = build.pending_item_ids;
    let pending_count = pending_item_ids.len();

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

    if !pending_item_ids.is_empty() {
        let tokens_opt: Option<crate::services::tidal::auth::TidalTokens> = {
            let s = state.read().await;
            if let Some(t) = s.tidal_tokens.clone() {
                Some(t)
            } else {
                drop(s);
                load_persisted_tidal_tokens(state).await.ok().flatten()
            }
        };

        if let Some(tokens) = tokens_opt {
            let semaphore = Arc::new(tokio::sync::Semaphore::new(RESOLVER_POOL_SIZE));
            let (event_tx, tidal_http_client) = {
                let s = state.read().await;
                (s.event_tx.clone(), s.tidal_http_client.clone())
            };
            for item_id in pending_item_ids {
                let sem = semaphore.clone();
                let db_bg = db.clone();
                let tok = tokens.clone();
                let tx = event_tx.clone();
                let http = tidal_http_client.clone();
                tokio::spawn(async move {
                    let _permit = sem.acquire_owned().await.ok();
                    resolve_pending_row(db_bg, tok, item_id, tx, http).await;
                });
            }
        } else {
            tracing::warn!(
                "{context}: Tidal tokens unavailable - pending rows will rely on lazy resolution"
            );
        }
    }

    {
        let s = state.read().await;
        let _ = s.event_tx.send(AppEvent::QueueUpdated);
    }

    Ok((first_playable, pending_count))
}

async fn start_first_radio_queue_item(
    state: &SharedState,
) -> Result<player::PlaybackSnapshot, (StatusCode, Json<Value>)> {
    let playback_generation = bump_playback_generation(state).await;
    clear_ephemeral_playback_markers(state, true).await;

    let previous_track_id = current_playback_track_id(state).await;
    let mut snapshot = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(|conn| player::start_queue_from_beginning(conn, false))
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "status": "playback_state_update_failed",
                        "message": "Failed to start the radio queue.",
                    })),
                )
            })?
    };

    let mut play_track = snapshot.state.current_track.clone();
    if play_track.is_none()
        && let Some(resolved) = resolve_pending_current_queue_item(state).await
    {
        play_track = Some(resolved);
        let state_guard = state.read().await;
        if let Ok(reloaded) = state_guard.db.with_conn(player::load_snapshot) {
            snapshot = reloaded;
        }
    }

    let end_reason = if play_track.is_some() {
        Some(player::ListenSessionEndReason::Replaced)
    } else {
        Some(player::ListenSessionEndReason::QueueEnded)
    };
    sync_session_after_snapshot(state, &snapshot, end_reason).await;

    if let Some(track) = play_track
        .as_ref()
        .or(snapshot.state.current_track.as_ref())
    {
        let user_quality = current_user_audio_quality(state).await;
        let stream_request = match player::build_tidal_stream_request(track, user_quality.clone()) {
            Some(request) => request,
            None => {
                let paused_snapshot = {
                    let state_guard = state.read().await;
                    state_guard.db.with_conn(player::pause).ok()
                };
                // sync_session_after_snapshot above already opened a session
                // for this (local) track; flush+drop it so we don't bill a
                // bogus multi-minute listen the next time the user plays.
                if let Some(snap) = paused_snapshot {
                    sync_session_after_snapshot(
                        state,
                        &snap,
                        Some(player::ListenSessionEndReason::Stopped),
                    )
                    .await;
                }
                return Err((
                    StatusCode::NOT_IMPLEMENTED,
                    Json(json!({
                        "status": "local_playback_not_supported",
                        "message": "Local-library playback is not wired into the host audio runtime yet.",
                        "track_id": track.id,
                    })),
                ));
            }
        };
        let stream_info = match resolve_tidal_playback_stream(state, track, &stream_request).await {
            Ok(info) => info,
            Err(error) => {
                let state_guard = state.read().await;
                let _ = state_guard.db.with_conn(player::pause);
                return Err(tidal_playback_error_response(
                    track.id,
                    error,
                    "TIDAL stream could not be resolved while starting radio.",
                ));
            }
        };
        let runtime_handle = match ensure_playback_runtime_for_track(state, track).await {
            Ok(handle) => handle,
            Err(_) => {
                let state_guard = state.read().await;
                let _ = state_guard.db.with_conn(player::pause);
                return Err((
                    StatusCode::BAD_GATEWAY,
                    Json(json!({
                        "status": "playback_runtime_unavailable",
                        "message": "Playback runtime was not available for starting radio.",
                        "track_id": track.id,
                    })),
                ));
            }
        };
        let job = player::build_playback_preparation(
            track,
            Some(&stream_info),
            effective_crossfade_ms(state, snapshot.state.crossfade_ms).await,
            user_quality,
        )
        .with_generation(playback_generation);
        align_device_to_stream_rate(state, &runtime_handle, &stream_info).await;
        runtime_handle.play(job).map_err(|error| {
            let message = format!("Failed to start host audio playback: {error}");
            report_playback_failure(state, &message);
            if let Ok(state_guard) = state.try_read() {
                let _ = state_guard.db.with_conn(player::pause);
            }
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
    } else if let Some(runtime_handle) = current_playback_runtime(state).await {
        let _ = runtime_handle.stop();
    }

    record_transition_if_changed(state, previous_track_id, &snapshot, "radio", true).await;

    let state_guard = state.read().await;
    if let Some(track_id) = play_track
        .as_ref()
        .or(snapshot.state.current_track.as_ref())
        .map(|track| track.id)
    {
        let _ = state_guard
            .event_tx
            .send(AppEvent::TrackChanged { track_id });
    }
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
    drop(state_guard);

    Ok(overlay_snapshot_with_external_track(state, snapshot).await)
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
        // User-driven radio start; reset post-clear suppression so the
        // freshly-built queue gets normal automix gating downstream.
        g.user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
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

    // Build queue atomically and collect pending row IDs for background tasks.
    let build = db
        .with_conn(move |conn| {
            Ok(
                crate::server::radio_pipeline::build_radio_queue_from_candidates(
                    conn,
                    payload.seed_track_id,
                    radio_queue.tracks,
                )?,
            )
        })
        .map_err(|e| {
            tracing::error!("radio_start: queue build failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to build queue" })),
            )
        })?;
    let first_item = build.first_item;
    let pending_item_ids = build.pending_item_ids;
    let pending_count = pending_item_ids.len();

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
            let (event_tx, tidal_http_client) = {
                let s = state.read().await;
                (s.event_tx.clone(), s.tidal_http_client.clone())
            };
            for item_id in pending_item_ids {
                let sem = semaphore.clone();
                let db_bg = db.clone();
                let tok = tokens.clone();
                let tx = event_tx.clone();
                let http = tidal_http_client.clone();
                tokio::spawn(async move {
                    let _permit = sem.acquire_owned().await.ok();
                    resolve_pending_row(db_bg, tok, item_id, tx, http).await;
                });
            }
        } else {
            tracing::warn!(
                "radio_start: Tidal tokens unavailable — pending rows will rely on lazy resolution"
            );
        }
    }

    {
        let s = state.read().await;
        let _ = s.event_tx.send(AppEvent::QueueUpdated);
    }

    let snapshot = start_first_radio_queue_item(&state).await?;

    Ok(Json(json!({
        "first_playable": first_playable,
        "pending_count": pending_count,
        "state": snapshot.state,
        "queue": snapshot.queue
    })))
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
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let blend = payload.blend.unwrap_or_default();
    let limit = payload.limit.unwrap_or(60).clamp(8, 200);
    let exclude = payload.exclude_track_ids.unwrap_or_default();

    let (db, lastfm) = {
        let g = state.read().await;
        g.user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
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
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "radio orchestration failed" })),
        )
    })?;

    let (first_playable, pending_count) = build_radio_queue_and_spawn_resolvers(
        &state,
        &db,
        None,
        queue.tracks.clone(),
        "radio_album",
    )
    .await?;
    let snapshot = start_first_radio_queue_item(&state).await?;
    let mut body = serde_json::to_value(queue).unwrap_or(json!({}));
    body["first_playable"] = first_playable;
    body["pending_count"] = json!(pending_count);
    body["state"] = json!(snapshot.state);
    body["queue"] = json!(snapshot.queue);
    Ok(Json(body))
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
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let blend = payload.blend.unwrap_or_default();
    let limit = payload.limit.unwrap_or(60).clamp(8, 200);
    let exclude = payload.exclude_track_ids.unwrap_or_default();

    let (db, lastfm) = {
        let g = state.read().await;
        g.user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
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
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "radio orchestration failed" })),
        )
    })?;

    let (first_playable, pending_count) = build_radio_queue_and_spawn_resolvers(
        &state,
        &db,
        None,
        queue.tracks.clone(),
        "radio_artist",
    )
    .await?;
    let snapshot = start_first_radio_queue_item(&state).await?;
    let mut body = serde_json::to_value(queue).unwrap_or(json!({}));
    body["first_playable"] = first_playable;
    body["pending_count"] = json!(pending_count);
    body["state"] = json!(snapshot.state);
    body["queue"] = json!(snapshot.queue);
    Ok(Json(body))
}

async fn get_discovery_space_meta(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;

    let total_tracks: i64 = state
        .db
        .with_conn(|conn| {
            conn.query_row("SELECT COUNT(*) FROM tracks", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(Into::into)
        })
        .unwrap_or(0);

    let model_row: Option<(String, String, Option<String>, i64)> = state
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT model_key, status, trained_at, dimension
             FROM embedding_models
             WHERE is_active = 1
             ORDER BY trained_at IS NULL, trained_at DESC
             LIMIT 1",
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
        })
        .ok()
        .flatten();

    let (model_key, model_status, trained_at, vector_dim, embedding_count) = match &model_row {
        Some((key, status, trained, dim)) => {
            let count: i64 = state
                .db
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT COUNT(*) FROM track_embeddings te
                     JOIN embedding_models em ON em.id = te.model_id
                     WHERE em.model_key = ?1",
                        rusqlite::params![key],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(Into::into)
                })
                .unwrap_or(0);
            (
                Some(key.clone()),
                Some(status.clone()),
                trained.clone(),
                Some(*dim),
                count,
            )
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

    let artists = state
        .read()
        .await
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT a.id, a.name, COUNT(th.track_id) as listen_count
             FROM artists a
             LEFT JOIN tracks t ON t.artist_id = a.id
             LEFT JOIN track_history th ON th.track_id = t.id
             GROUP BY a.id, a.name
             ORDER BY listen_count DESC
             LIMIT ?",
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
        })
        .unwrap_or_default();

    let max_count = artists.iter().map(|(_, _, c)| *c).max().unwrap_or(1) as f64;
    let artist_count = artists.len();

    let artist_nodes: Vec<Value> = artists
        .into_iter()
        .enumerate()
        .map(|(i, (id, name, count))| {
            let angle = (i as f64 / artist_count.max(1) as f64) * std::f64::consts::PI * 2.0;
            let radius = 80.0 + (i as f64 * 43.0).sin() * 120.0;
            let affinity = if max_count > 0.0 {
                count as f64 / max_count
            } else {
                0.0
            };
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
        state_guard.tidal_http_client.clone(),
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
            guard.tidal_http_client.clone(),
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

async fn clear_ephemeral_playback_markers(state: &SharedState, clear_mix_queue: bool) {
    let mut state_guard = state.write().await;
    state_guard.external_playback_track = None;
    state_guard.ephemeral_tidal_track = None;
    if clear_mix_queue {
        state_guard.pending_tidal_mix_queue.lock().unwrap().clear();
    }
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
    } else if snapshot.state.current_track.is_none()
        && let Some(track) = state_guard.external_playback_track.as_ref()
    {
        snapshot.state.current_track = Some(track.clone());
    }
    // Surface the pending TIDAL mix queue (auto-advance items behind the
    // currently-playing ephemeral track) into the visible queue so UP NEXT
    // shows the rest of the mix instead of "empty".
    let pending = state_guard
        .pending_tidal_mix_queue
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    drop(state_guard);
    if !pending.is_empty() {
        let start_position = snapshot
            .queue
            .iter()
            .map(|q| q.position)
            .max()
            .unwrap_or(-1)
            + 1;
        for (offset, p) in pending.into_iter().enumerate() {
            let track = crate::db::models::Track {
                id: -p.tidal_track_id,
                title: p.title,
                artist_id: 0,
                artist_name: p.artist_name,
                album_id: None,
                album_title: p.album_title,
                disc_number: None,
                track_number: None,
                duration_ms: p.duration_ms,
                isrc: None,
                tidal_id: Some(p.tidal_track_id),
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
                artwork_url: p.artwork_url,
            };
            // Negative ids for both queue id + track id so the frontend can
            // tell these are in-memory placeholders and skip remove/reorder
            // until proper ephemeral-queue management ships.
            snapshot.queue.push(crate::db::models::QueueItem {
                id: -(offset as i64 + 1),
                track,
                position: start_position + offset as i32,
                source: "tidal_mix".to_string(),
                reason: None,
                is_pending: false,
            });
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
    let deleted_track_ids: Vec<i64> = track_pairs.iter().map(|(local_id, _)| *local_id).collect();
    let outcome = match db.with_conn(|conn| {
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
    use std::sync::atomic::Ordering;

    let (http_client, event_tx, running) = {
        let g = state.read().await;
        (
            g.http_client.clone(),
            g.event_tx.clone(),
            g.musicbrainz_enrich_running.clone(),
        )
    };

    if running.load(Ordering::SeqCst) {
        return Ok(Json(json!({ "status": "already_running" })));
    }

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

    running.store(true, Ordering::SeqCst);

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
            1,
        )
        .await;
        running.store(false, Ordering::SeqCst);
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

    // Broadcast queue / playback / library events based on the reconcile outcome.
    {
        let s = state.read().await;
        if result.reconcile.queue_changed {
            let _ = s.event_tx.send(AppEvent::QueueUpdated);
        }
        if result.reconcile.current_changed {
            let _ = s.event_tx.send(AppEvent::PlaybackStateChanged);
        }
        let _ = s.event_tx.send(AppEvent::LibrarySynced);
    }

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
                if (e.to_string().contains("401")
                    || e.to_string().to_lowercase().contains("unauthorized"))
                    && let Ok(refreshed) = recover_tidal_session(&state, &http, &t).await
                {
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
    let limit = params.limit.unwrap_or(20);

    // Snapshot what each side needs without holding the read lock across the
    // Sportify HTTP call.
    let (db, sportify_client, cache_cfg) = {
        let s = state.read().await;
        (
            s.db.clone(),
            s.sportify_client.clone(),
            s.sportify_cache_config,
        )
    };

    // Local DB search and Sportify playlist search are independent — run them
    // concurrently. Local search must succeed (existing contract); Sportify is
    // best-effort (upstream may break).
    let q = params.q.clone();
    let db_for_local = db.clone();
    let local_fut = async move { db_for_local.with_conn(|conn| queries::search(conn, &q, limit)) };

    let spotify_fut = async {
        match sportify_client {
            Some(client) => fetch_spotify_playlist_search_compact(
                &client,
                &db,
                &cache_cfg,
                &params.q,
                limit.min(20).max(1) as u32,
            )
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("sportify playlist search failed: {}", e);
                Vec::new()
            }),
            None => Vec::new(),
        }
    };

    let (local_res, spotify_playlists) = tokio::join!(local_fut, spotify_fut);
    let local = local_res.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({
        "tracks": local.tracks,
        "albums": local.albums,
        "artists": local.artists,
        "spotify_playlists": spotify_playlists,
    })))
}

/// Compact playlist-search result tailored for inline rendering in /search
/// and Ctrl+K. Drops the heavyweight track-list payload — the ephemeral
/// view fetches that on click.
#[derive(Debug, Serialize)]
struct SpotifyPlaylistSearchItem {
    spotify_id: String,
    name: String,
    description: Option<String>,
    image_url: Option<String>,
    owner: Option<String>,
    follower_count: Option<i64>,
    total_tracks: Option<i32>,
}

async fn fetch_spotify_playlist_search_compact(
    client: &crate::services::sportify::SportifyClient,
    db: &crate::db::Database,
    cfg: &crate::services::sportify::cache::SportifyCacheConfig,
    query: &str,
    limit: u32,
) -> anyhow::Result<Vec<SpotifyPlaylistSearchItem>> {
    use crate::services::sportify::client::SportifySearchKind;
    use crate::services::sportify::recommend::cached_search;

    if query.trim().is_empty() {
        return Ok(Vec::new());
    }

    let results = cached_search(
        client,
        db,
        cfg,
        query,
        SportifySearchKind::Playlist,
        limit,
        0,
    )
    .await?;

    Ok(results
        .playlists
        .into_iter()
        .filter_map(|p| {
            let id = p.spotify_id()?;
            Some(SpotifyPlaylistSearchItem {
                spotify_id: id,
                name: p.title().unwrap_or_default(),
                description: p.description.clone(),
                image_url: p.best_thumbnail(),
                owner: p
                    .owner
                    .as_ref()
                    .and_then(|o| o.display_name().map(str::to_string)),
                follower_count: p.follower_count(),
                total_tracks: p.total_track_count(),
            })
        })
        .collect())
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
        let active = state_guard
            .audio_active
            .load(std::sync::atomic::Ordering::Relaxed);
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
            "exclusive_engaged": info.exclusive_engaged,
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
    let snapshot = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(player::load_snapshot)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    // Reuse the overlay so the pending TIDAL mix queue + any external/ephemeral
    // current-track shows up here, matching what /api/playback/state returns.
    let snapshot = overlay_snapshot_with_external_track(&state, snapshot).await;
    Ok(Json(json!({ "queue": snapshot.queue })))
}

async fn play_track(
    State(state): State<SharedState>,
    Json(payload): Json<PlaybackTrackRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    // Library tracks have positive IDs. id == 0 is the COALESCE sentinel for
    // pending queue rows (resolution_state='pending'); id < 0 is the ephemeral
    // Tidal-only convention used by /api/tidal/play_ephemeral. Neither belongs
    // in this route.
    if payload.track_id <= 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "invalid_track_id",
                "message": "play_track requires a positive library track id.",
                "track_id": payload.track_id,
            })),
        ));
    }

    let previous_track_id = current_playback_track_id(&state).await;
    let playback_generation = bump_playback_generation(&state).await;
    // User-driven play; reset the post-clear suppression so automix
    // re-engages naturally instead of waiting out the 60s window.
    {
        let g = state.read().await;
        g.user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }
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
    clear_ephemeral_playback_markers(&state, true).await;

    let user_quality = current_user_audio_quality(&state).await;
    let stream_request = match player::build_tidal_stream_request(&track, user_quality.clone()) {
        Some(request) => request,
        None => {
            let paused_snapshot = {
                let state_guard = state.read().await;
                state_guard.db.with_conn(player::pause).ok()
            };
            // Flush the prior TIDAL session before bailing — otherwise the
            // active session keeps accumulating against the still-playing
            // previous track, and the next successful play_track records a
            // bogus multi-hour listen.
            if let Some(snap) = paused_snapshot {
                sync_session_after_snapshot(
                    &state,
                    &snap,
                    Some(player::ListenSessionEndReason::Stopped),
                )
                .await;
            }
            return Err((
                StatusCode::NOT_IMPLEMENTED,
                Json(json!({
                    "status": "local_playback_not_supported",
                    "message": "Local-library playback is not wired into the host audio runtime yet.",
                    "track_id": track.id,
                })),
            ));
        }
    };
    let stream_info = match resolve_tidal_playback_stream(&state, &track, &stream_request).await {
        Ok(info) => info,
        Err(error) => {
            let state_guard = state.read().await;
            let _ = state_guard.db.with_conn(player::pause);
            return Err(tidal_playback_error_response(
                track.id,
                error,
                "TIDAL stream could not be resolved before playback.",
            ));
        }
    };
    tracing::info!(
        target: "noor.playback.tidal",
        event = "playback_stream_ready",
        track_id = track.id,
        "TIDAL stream resolved before playback start"
    );

    let runtime_handle = ensure_playback_runtime_for_track(&state, &track).await?;
    let crossfade_ms = current_crossfade_ms(&state).await;
    let job =
        player::build_playback_preparation(&track, Some(&stream_info), crossfade_ms, user_quality)
            .with_generation(playback_generation);
    align_device_to_stream_rate(&state, &runtime_handle, &stream_info).await;
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
                    &http,
                    &token,
                    tidal_id,
                    &quality,
                    duration_ms,
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
            // play_track resets `user_cleared_at` to 0 above, so the
            // suppression window cannot apply to this user-driven fill.
            let result = bg_db.with_conn(|conn| {
                player::ensure_automix_queue_depth(conn, player::AUTOMIX_MIN_UPCOMING, false)
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
    if let Some(runtime_handle) = current_playback_runtime(&state).await
        && let Err(error) = runtime_handle.pause()
    {
        let message = format!("Failed to pause host audio playback: {error}");
        report_playback_failure(&state, &message);
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    let snapshot = {
        let state = state.read().await;
        state
            .db
            .with_conn(player::pause)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    // Flush the in-progress session to listen_history on pause so analytics
    // shows partial listens without waiting for the next track-change. The
    // snapshot has is_playing=false, so sync_session_after_snapshot won't
    // start a new session — resume_session_after_snapshot will reopen one
    // (reusing the same session_id if the gap is < 30 min).
    sync_session_after_snapshot(
        &state,
        &snapshot,
        Some(player::ListenSessionEndReason::Stopped),
    )
    .await;

    let state_guard = state.read().await;
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);

    let snapshot = overlay_snapshot_with_external_track(&state, snapshot).await;
    Ok(Json(json!({ "state": snapshot.state })))
}

async fn resume_playback(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    if let Some(runtime_handle) = current_playback_runtime(&state).await
        && let Err(error) = runtime_handle.resume()
    {
        let message = format!("Failed to resume host audio playback: {error}");
        report_playback_failure(&state, &message);
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    let snapshot = {
        let state = state.read().await;
        state
            .db
            .with_conn(player::resume)
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

fn import_metadata_from_search_track(t: TidalSearchTrack) -> tidal_import::ImportTrackMetadata {
    tidal_import::ImportTrackMetadata {
        tidal_id: t.id,
        title: t.title,
        artist_name: t.artist_name.unwrap_or_default(),
        artist_tidal_id: t.artist_id,
        artist_picture: t.artist_picture,
        album_title: t.album_title,
        album_tidal_id: t.album_id,
        album_artwork_url: t.artwork_url,
        duration_ms: Some(t.duration * 1000),
    }
}

fn import_metadata_from_tidal_track(t: TidalTrack) -> tidal_import::ImportTrackMetadata {
    let album_title = t.album.as_ref().map(|album| album.title.clone());
    let album_tidal_id = t.album.as_ref().map(|album| album.id);
    let album_artwork_url = t
        .album
        .as_ref()
        .and_then(|album| TidalClient::get_artwork_url(&album.cover, 640));
    tidal_import::ImportTrackMetadata {
        tidal_id: t.id,
        title: t.title,
        artist_name: t.artist.name,
        artist_tidal_id: Some(t.artist.id),
        artist_picture: t.artist.picture,
        album_title,
        album_tidal_id,
        album_artwork_url,
        duration_ms: Some(t.duration * 1000),
    }
}

async fn find_pending_tidal_match(
    client: &TidalClient,
    pending_artist: &str,
    pending_title: &str,
    tidal_id_hint: Option<i64>,
) -> anyhow::Result<Option<(f64, tidal_import::ImportTrackMetadata)>> {
    if let Some(tidal_id) = tidal_id_hint.filter(|id| *id > 0) {
        let track = client.get_track(tidal_id).await?;
        return Ok(Some((1.0, import_metadata_from_tidal_track(track))));
    }

    let query = format!("{} {}", pending_artist, pending_title);
    let results = client.search(&query, 5).await?;
    let best = results
        .into_iter()
        .filter_map(|t| {
            let s = score_tidal_candidate(
                t.artist_name.as_deref().unwrap_or(""),
                &t.title,
                pending_artist,
                pending_title,
            );
            if s >= MATCH_QUALITY_THRESHOLD {
                Some((s, t))
            } else {
                None
            }
        })
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    Ok(best.map(|(score, track)| (score, import_metadata_from_search_track(track))))
}

/// Atomically promote a pending queue row to a resolved library row.
///
/// Returns `true` iff this caller won the promotion race (queue row had
/// `track_id IS NULL` at the moment of UPDATE). Both resolver paths funnel
/// through this so the event-emission contract is the same: any successful
/// promotion broadcasts `QueueUpdated` exactly once.
fn promote_pending_row_emit(
    db: &crate::db::Database,
    event_tx: &tokio::sync::broadcast::Sender<AppEvent>,
    queue_item_id: i64,
    local_track_id: i64,
    score_stored: i32,
) -> bool {
    let promoted = db
        .with_conn(move |conn| {
            Ok(conn.execute(
                "UPDATE queue
                 SET track_id = ?1, resolved_at = datetime('now'),
                     tidal_match_score = ?2, resolving_at = NULL
                 WHERE id = ?3 AND track_id IS NULL",
                rusqlite::params![local_track_id, score_stored, queue_item_id],
            )? == 1)
        })
        .unwrap_or(false);
    if promoted {
        let _ = event_tx.send(AppEvent::QueueUpdated);
    }
    promoted
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
    event_tx: tokio::sync::broadcast::Sender<AppEvent>,
    http: reqwest::Client,
) {
    let row: Option<(String, String, Option<i64>)> = db
        .with_conn(move |conn| {
            Ok(conn
                .query_row(
                    "SELECT pending_artist, pending_title, tidal_id_hint FROM queue
                     WHERE id = ?1 AND track_id IS NULL AND pending_at IS NOT NULL",
                    rusqlite::params![queue_item_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?)
        })
        .unwrap_or(None);

    let (pending_artist, pending_title, tidal_id_hint) = match row {
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

    let client = TidalClient::with_http(
        http,
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let resolved = match find_pending_tidal_match(
        &client,
        &pending_artist,
        &pending_title,
        tidal_id_hint,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(queue_item_id, error = %e, "background resolver: Tidal resolve failed");
            release(&db, queue_item_id);
            return;
        }
    };

    let (score, metadata) = match resolved {
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

    let imported = crate::services::tidal::import::import_track_from_metadata(&db, metadata).await;

    let local_id = match imported {
        Ok(imp) => imp.local_id,
        Err(e) => {
            tracing::warn!(queue_item_id, error = %e, "background resolver: import failed");
            release(&db, queue_item_id);
            return;
        }
    };

    let score_stored = (score * 1000.0) as i32;
    let promoted = promote_pending_row_emit(&db, &event_tx, queue_item_id, local_id, score_stored);

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

    let (queue_item_id, pending_artist, pending_title, tidal_id_hint): (
        i64,
        String,
        String,
        Option<i64>,
    ) = db
        .with_conn(|conn| {
            Ok(conn
                .query_row(
                    "SELECT q.id, q.pending_artist, q.pending_title, q.tidal_id_hint
                 FROM playback_state ps
                 JOIN queue q ON q.id = ps.current_queue_item_id
                 WHERE ps.id = 1 AND q.track_id IS NULL AND q.pending_at IS NOT NULL",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
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

    let (tokens, tidal_http_client) = {
        let persisted = match load_persisted_tidal_tokens(state).await.ok().flatten() {
            Some(t) => t,
            None => {
                release_lock(&db, queue_item_id);
                return None;
            }
        };
        let s = state.read().await;
        (
            s.tidal_tokens.clone().unwrap_or(persisted),
            s.tidal_http_client.clone(),
        )
    };

    let client = TidalClient::with_http(
        tidal_http_client,
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let resolved =
        match find_pending_tidal_match(&client, &pending_artist, &pending_title, tidal_id_hint)
            .await
        {
            Ok(r) => r,
            Err(_) => {
                release_lock(&db, queue_item_id);
                return None;
            }
        };

    let (score, metadata) = match resolved {
        Some(pair) => pair,
        None => {
            release_lock(&db, queue_item_id);
            return None;
        }
    };

    let imported = crate::services::tidal::import::import_track_from_metadata(&db, metadata).await;

    let local_id = match imported {
        Ok(imp) => imp.local_id,
        Err(_) => {
            release_lock(&db, queue_item_id);
            return None;
        }
    };

    let score_stored = (score * 1000.0) as i32;
    // Atomic promotion: only one resolver wins even under a race.
    let event_tx = {
        let s = state.read().await;
        s.event_tx.clone()
    };
    let promoted = promote_pending_row_emit(&db, &event_tx, queue_item_id, local_id, score_stored);

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
    let _ = event_tx.send(AppEvent::TrackChanged { track_id: local_id });
    let _ = event_tx.send(AppEvent::PlaybackStateChanged);

    db.with_conn(move |conn| queue::get_track_by_id(conn, local_id))
        .ok()
        .flatten()
}

async fn advance_ephemeral_next_if_needed(
    state: &SharedState,
) -> Result<Option<Json<Value>>, (StatusCode, Json<Value>)> {
    let has_ephemeral = {
        let state_guard = state.read().await;
        state_guard.ephemeral_tidal_track.is_some()
    };
    if !has_ephemeral {
        return Ok(None);
    }

    let next = {
        let state_guard = state.read().await;
        state_guard
            .pending_tidal_mix_queue
            .lock()
            .unwrap()
            .pop_front()
    };

    if let Some(next) = next {
        start_ephemeral_tidal_playback(state, next).await?;
        let snapshot = {
            let state_guard = state.read().await;
            state_guard
                .db
                .with_conn(player::load_snapshot)
                .map_err(|_| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({
                            "status": "playback_state_update_failed",
                            "message": "Failed to load playback state after advancing TIDAL queue.",
                        })),
                    )
                })?
        };
        let snapshot = overlay_snapshot_with_external_track(state, snapshot).await;
        return Ok(Some(Json(json!({
            "state": snapshot.state,
            "queue": snapshot.queue
        }))));
    }

    {
        let mut state_guard = state.write().await;
        state_guard.ephemeral_tidal_track = None;
        if let Some(info) = state_guard.playback_runtime_info.as_mut() {
            info.active_track_id = None;
        }
        state_guard
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE playback_state
                     SET current_track_id = NULL, current_queue_item_id = NULL, position_ms = 0
                     WHERE id = 1",
                    [],
                )?;
                Ok(())
            })
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "status": "playback_state_update_failed",
                        "message": "Failed to clear TIDAL playback state before advancing queue.",
                    })),
                )
            })?;
    }

    Ok(None)
}

async fn next_track(
    State(state): State<SharedState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if let Some(response) = advance_ephemeral_next_if_needed(&state).await? {
        return Ok(response);
    }

    let playback_generation = bump_playback_generation(&state).await;
    let previous_track_id = current_playback_track_id(&state).await;
    let mut snapshot = {
        let state = state.read().await;
        let cleared = recently_cleared(&state);
        state
            .db
            .with_conn(|conn| player::next_track(conn, cleared))
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
                        let cleared = recently_cleared(&s);
                        s.db.with_conn(|conn| player::next_track(conn, cleared))
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
            .and_then(|qid| {
                snapshot
                    .queue
                    .iter()
                    .find(|q| q.id == qid)
                    .map(|q| q.position)
            })
            .or_else(|| {
                snapshot.state.current_track.as_ref().and_then(|t| {
                    snapshot
                        .queue
                        .iter()
                        .find(|q| q.track.id == t.id)
                        .map(|q| q.position)
                })
            })
            .unwrap_or(0);
        let new_upcoming = snapshot
            .queue
            .iter()
            .filter(|q| q.position > current_pos && q.source == "automix-new")
            .count();
        if new_upcoming < 2
            && let Some(track) = snapshot.state.current_track.clone()
        {
            let bg_state = state.clone();
            tokio::spawn(async move {
                inject_discovery_tracks(&bg_state, &track).await;
            });
        }
    }

    // A resolved pending track counts as Replaced, not QueueEnded.
    let end_reason = if effective_current_track.is_some() || snapshot.state.current_track.is_some()
    {
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
        )
        .with_generation(playback_generation);
        align_device_to_stream_rate(&state, &runtime_handle, &stream_info).await;
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
    let playback_generation = bump_playback_generation(&state).await;
    let previous_track_id = current_playback_track_id(&state).await;
    let mut snapshot = {
        let state = state.read().await;
        state.db.with_conn(player::previous_track).map_err(|_| {
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

    // If the previous item is a pending (non-library) queue row, resolve it to Tidal now.
    // Same pattern as `next_track`. Unresolvable rows are skipped by stepping back once more.
    let effective_current_track: Option<crate::db::models::Track> =
        if let Some(t) = snapshot.state.current_track.clone() {
            Some(t)
        } else {
            let resolved = resolve_pending_current_queue_item(&state).await;
            match resolved {
                Some(ref t) => {
                    set_external_playback_track(&state, Some(t.clone())).await;
                }
                None => {
                    if let Ok(prev_snapshot) = {
                        let s = state.read().await;
                        s.db.with_conn(player::previous_track)
                    } {
                        snapshot = prev_snapshot;
                    }
                }
            }
            resolved
        };

    record_transition_if_changed(&state, previous_track_id, &snapshot, "user", false).await;

    sync_session_after_snapshot(
        &state,
        &snapshot,
        Some(player::ListenSessionEndReason::Replaced),
    )
    .await;

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
        )
        .with_generation(playback_generation);
        align_device_to_stream_rate(&state, &runtime_handle, &stream_info).await;
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

    // The pending TIDAL mix queue lives in-memory outside the DB queue, so
    // `apply_shuffle` above never sees it. Reorder it here so flipping shuffle
    // on during a TIDAL mix actually changes what plays next. Pending entries
    // carry no genre/artist_id metadata, so genre/weighted modes degrade to a
    // plain Fisher-Yates — same shape as `true` shuffle.
    if mode != queue::ShuffleMode::Off {
        let mut q = state_guard.pending_tidal_mix_queue.lock().unwrap();
        if q.len() > 1 {
            use rand::seq::SliceRandom;
            q.make_contiguous().shuffle(&mut rand::thread_rng());
        }
    }

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
    // User-driven enqueue; clear the post-clear suppression window.
    state
        .user_cleared_at
        .store(0, std::sync::atomic::Ordering::Relaxed);
    state
        .db
        .with_conn(|conn| {
            let queue = player::enqueue_track(conn, payload.track_id, "user")?;
            let _ = state.event_tx.send(AppEvent::QueueUpdated);
            Ok(Json(json!({ "queue": queue })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

fn queue_external_insert<'a>(
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
            reason: None,
            tidal_id_hint: None,
            local_track_id: Some(positive(payload.track_id, "track_id")?),
        }),
        QueueExternalKind::Tidal => Ok(queue::ExternalTrackInsert {
            artist: non_empty(&payload.artist, "artist")?,
            title: non_empty(&payload.title, "title")?,
            source,
            reason: None,
            tidal_id_hint: Some(positive(payload.tidal_id, "tidal_id")?),
            local_track_id: None,
        }),
        QueueExternalKind::External => Ok(queue::ExternalTrackInsert {
            artist: non_empty(&payload.artist, "artist")?,
            title: non_empty(&payload.title, "title")?,
            source,
            reason: None,
            tidal_id_hint: None,
            local_track_id: None,
        }),
    }
}

fn current_queue_position(conn: &rusqlite::Connection) -> anyhow::Result<Option<i32>> {
    let by_queue_item: Option<i32> = conn
        .query_row(
            "SELECT q.position
             FROM playback_state ps
             JOIN queue q ON q.id = ps.current_queue_item_id
             WHERE ps.id = 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if by_queue_item.is_some() {
        return Ok(by_queue_item);
    }

    Ok(conn
        .query_row(
            "SELECT q.position
             FROM playback_state ps
             JOIN queue q ON q.track_id = ps.current_track_id
             WHERE ps.id = 1 AND ps.current_track_id IS NOT NULL
             ORDER BY q.position ASC, q.id ASC
             LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?)
}

async fn spawn_pending_queue_resolver(state: &SharedState, queue_item_id: i64) {
    let tokens_opt: Option<crate::services::tidal::auth::TidalTokens> = {
        let s = state.read().await;
        if let Some(t) = s.tidal_tokens.clone() {
            Some(t)
        } else {
            drop(s);
            load_persisted_tidal_tokens(state).await.ok().flatten()
        }
    };

    let Some(tokens) = tokens_opt else {
        return;
    };

    let (db, event_tx, tidal_http_client) = {
        let s = state.read().await;
        (
            s.db.clone(),
            s.event_tx.clone(),
            s.tidal_http_client.clone(),
        )
    };
    tokio::spawn(async move {
        resolve_pending_row(db, tokens, queue_item_id, event_tx, tidal_http_client).await;
    });
}

async fn queue_append(
    State(state): State<SharedState>,
    Json(payload): Json<QueueExternalRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let insert = queue_external_insert(&payload, "user_queue")
        .map_err(|message| (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))))?;

    let (queue, inserted) = {
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
                let _ = event_tx.send(AppEvent::QueueUpdated);
                Ok((queue, inserted))
            })
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "failed to append queue item" })),
                )
            })?
    };

    if let queue::InsertResult::Pending { queue_id } = inserted {
        spawn_pending_queue_resolver(&state, queue_id).await;
    }

    Ok(Json(json!({ "queue": queue })))
}

async fn queue_append_many(
    State(state): State<SharedState>,
    Json(payload): Json<QueueExternalManyRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let inserts: Vec<_> = payload
        .items
        .iter()
        .map(|item| queue_external_insert(item, "user_queue"))
        .collect::<Result<_, _>>()
        .map_err(|message| (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))))?;

    let (queue, inserted) = {
        let state_guard = state.read().await;
        state_guard
            .user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let event_tx = state_guard.event_tx.clone();
        state_guard
            .db
            .with_conn(|conn| {
                let mut inserted = Vec::with_capacity(inserts.len());
                for insert in &inserts {
                    inserted.push(queue::append_external_track(conn, insert)?);
                }
                let queue = queue::load_queue(conn)?;
                let _ = event_tx.send(AppEvent::QueueUpdated);
                Ok((queue, inserted))
            })
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "failed to append queue items" })),
                )
            })?
    };

    for item in inserted {
        if let queue::InsertResult::Pending { queue_id } = item {
            spawn_pending_queue_resolver(&state, queue_id).await;
        }
    }

    Ok(Json(json!({ "queue": queue })))
}

async fn queue_play_next(
    State(state): State<SharedState>,
    Json(payload): Json<QueueExternalRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let insert = queue_external_insert(&payload, "user_play_next")
        .map_err(|message| (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))))?;

    let (queue, inserted) = {
        let state_guard = state.read().await;
        state_guard
            .user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let event_tx = state_guard.event_tx.clone();
        state_guard
            .db
            .with_conn(|conn| {
                let inserted = match current_queue_position(conn)? {
                    Some(position) => queue::insert_external_track_after(conn, &insert, position)?,
                    None => queue::append_external_track(conn, &insert)?,
                };
                let queue = queue::load_queue(conn)?;
                let _ = event_tx.send(AppEvent::QueueUpdated);
                Ok((queue, inserted))
            })
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "failed to insert queue item" })),
                )
            })?
    };

    if let queue::InsertResult::Pending { queue_id } = inserted {
        spawn_pending_queue_resolver(&state, queue_id).await;
    }

    Ok(Json(json!({ "queue": queue })))
}

async fn queue_play_next_many(
    State(state): State<SharedState>,
    Json(payload): Json<QueueExternalManyRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let inserts: Vec<_> = payload
        .items
        .iter()
        .map(|item| queue_external_insert(item, "user_play_next"))
        .collect::<Result<_, _>>()
        .map_err(|message| (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))))?;

    let (queue, inserted) = {
        let state_guard = state.read().await;
        state_guard
            .user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let event_tx = state_guard.event_tx.clone();
        state_guard
            .db
            .with_conn(|conn| {
                let mut inserted = Vec::with_capacity(inserts.len());
                match current_queue_position(conn)? {
                    Some(position) => {
                        // Insert in reverse so repeated "after current" inserts preserve
                        // the caller's original order in the queue.
                        for insert in inserts.iter().rev() {
                            inserted
                                .push(queue::insert_external_track_after(conn, insert, position)?);
                        }
                    }
                    None => {
                        for insert in &inserts {
                            inserted.push(queue::append_external_track(conn, insert)?);
                        }
                    }
                }
                let queue = queue::load_queue(conn)?;
                let _ = event_tx.send(AppEvent::QueueUpdated);
                Ok((queue, inserted))
            })
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "failed to insert queue items" })),
                )
            })?
    };

    for item in inserted {
        if let queue::InsertResult::Pending { queue_id } = item {
            spawn_pending_queue_resolver(&state, queue_id).await;
        }
    }

    Ok(Json(json!({ "queue": queue })))
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
                Some(reasons) => {
                    player::replace_queue_with_reasons(conn, &payload.track_ids, reasons, "user")?
                }
                None => player::replace_queue_with_tracks(conn, &payload.track_ids, "user")?,
            };
            // Phase 2c-ii-a: append pending (last.fm) candidates after library tracks.
            if let Some(pending) = &payload.pending_candidates
                && !pending.is_empty()
            {
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

async fn clear_queue_route(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
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
            // Return the full PlaybackSnapshot ({state, queue}) so the UI can
            // refresh both at once — additive over the prior `{queue}` shape:
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
            state
                .user_cleared_at
                .store(now_secs, std::sync::atomic::Ordering::Relaxed);
            let _ = state.event_tx.send(AppEvent::QueueUpdated);
            Ok(Json(json!({
                "queue": snapshot.queue,
                "playback_state": snapshot.state,
            })))
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
    // Smart-playlist genre rules are inclusion-only — the RuleClause enum has
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
            }
            Err(e) => {
                tracing::error!("TIDAL polling failed: {}", e);
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
    offset: Option<i32>,
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

#[derive(Serialize)]
struct TidalSearchVideoResp {
    tidal_id: i64,
    title: String,
    duration_ms: Option<i64>,
    artist_id: Option<i64>,
    artist_name: Option<String>,
    album_tidal_id: Option<i64>,
    artwork_url: Option<String>,
    quality: Option<String>,
    explicit: Option<bool>,
    r#type: String,
}

async fn tidal_search(
    State(state): State<SharedState>,
    Query(params): Query<TidalSearchParams>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let tokens = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
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
    let offset = params.offset.unwrap_or(0).max(0);
    // Snapshot what we need from state in one lock acquisition.
    let (db, http_client, tidal_http_client) = {
        let s = state.read().await;
        (
            s.db.clone(),
            s.http_client.clone(),
            s.tidal_http_client.clone(),
        )
    };

    let cache_cfg = crate::services::tidal::cache::TidalSearchCacheConfig::default();

    // Cache check — best-effort. A read failure must NOT block the upstream call.
    let cached = db
        .with_conn(|conn| {
            crate::services::tidal::cache::get_search(conn, &cache_cfg, &params.q, limit, offset)
        })
        .ok()
        .flatten();

    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );

    let results = if let Some(hit) = cached {
        hit
    } else {
        let fetched = match client.search_catalog(&params.q, limit, offset).await {
            Ok(r) => r,
            Err(e) if error_looks_like_auth(&e) => {
                let refreshed = recover_tidal_session(&state, &http_client, &tokens)
                    .await
                    .map_err(|re| {
                        (
                            StatusCode::BAD_GATEWAY,
                            Json(
                                json!({ "error": format!("TIDAL session refresh failed: {}", re) }),
                            ),
                        )
                    })?;
                let retry_client = TidalClient::with_http(
                    tidal_http_client,
                    refreshed.access_token.clone(),
                    refreshed.country_code.clone(),
                );
                retry_client
                    .search_catalog(&params.q, limit, offset)
                    .await
                    .map_err(|e2| {
                        (
                            StatusCode::BAD_GATEWAY,
                            Json(json!({ "error": e2.to_string() })),
                        )
                    })?
            }
            Err(e) => {
                return Err((
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": e.to_string() })),
                ));
            }
        };
        // Best-effort cache write — log and continue on failure.
        let to_cache = fetched.clone();
        let q_owned = params.q.clone();
        let lim_for_write = limit;
        let off_for_write = offset;
        if let Err(e) = db.with_conn(move |conn| {
            crate::services::tidal::cache::put_search(
                conn,
                &q_owned,
                lim_for_write,
                off_for_write,
                &to_cache,
            )
        }) {
            tracing::warn!("tidal_search_cache write failed: {}", e);
        }
        fetched
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

// ─── TIDAL Playlist Search + Tracks ───────────────────────────────────────────

fn tidal_video_to_resp(video: TidalSearchVideo) -> TidalSearchVideoResp {
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

async fn tidal_request_tokens(
    state: &SharedState,
) -> Result<Option<tidal_auth::TidalTokens>, (StatusCode, Json<Value>)> {
    let persisted = load_persisted_tidal_tokens(state).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
    })?;
    let s = state.read().await;
    Ok(s.tidal_tokens.clone().or(persisted))
}

async fn tidal_video_search(
    State(state): State<SharedState>,
    Query(params): Query<TidalSearchParams>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let Some(tokens) = tidal_request_tokens(&state).await? else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let limit = params.limit.unwrap_or(20).min(50);
    let offset = params.offset.unwrap_or(0).max(0);
    let (http_client, tidal_http_client) = {
        let s = state.read().await;
        (s.http_client.clone(), s.tidal_http_client.clone())
    };
    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let videos = match client.search_videos(&params.q, limit, offset).await {
        Ok(videos) => videos,
        Err(e) if error_looks_like_auth(&e) => {
            let refreshed = recover_tidal_session(&state, &http_client, &tokens)
                .await
                .map_err(|re| {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({ "error": format!("TIDAL session refresh failed: {}", re) })),
                    )
                })?;
            let retry_client = TidalClient::with_http(
                tidal_http_client,
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            retry_client
                .search_videos(&params.q, limit, offset)
                .await
                .map_err(|e2| {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({ "error": e2.to_string() })),
                    )
                })?
        }
        Err(e) => {
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": e.to_string() })),
            ));
        }
    };

    Ok(Json(json!({
        "videos": videos.into_iter().map(tidal_video_to_resp).collect::<Vec<_>>()
    })))
}

#[derive(Debug, Deserialize)]
struct TidalVideoPlaybackParams {
    quality: Option<String>,
}

fn tidal_video_stream_error_response(
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

async fn tidal_video_playback(
    State(state): State<SharedState>,
    Path(video_id): Path<i64>,
    Query(params): Query<TidalVideoPlaybackParams>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
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
            let refreshed = recover_tidal_session(&state, &http_client, &tokens)
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

async fn tidal_video_mix_items(
    State(state): State<SharedState>,
    Path(mix_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let Some(tokens) = tidal_request_tokens(&state).await? else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let (http_client, tidal_http_client) = {
        let s = state.read().await;
        (s.http_client.clone(), s.tidal_http_client.clone())
    };
    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let items = match client.get_video_mix_items(&mix_id).await {
        Ok(items) => items,
        Err(e) if error_looks_like_auth(&e) => {
            let refreshed = recover_tidal_session(&state, &http_client, &tokens)
                .await
                .map_err(|re| {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({ "error": format!("TIDAL session refresh failed: {}", re) })),
                    )
                })?;
            let retry_client = TidalClient::with_http(
                tidal_http_client,
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            retry_client
                .get_video_mix_items(&mix_id)
                .await
                .map_err(|e2| {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({ "error": e2.to_string() })),
                    )
                })?
        }
        Err(e) => {
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": e.to_string() })),
            ));
        }
    };

    Ok(Json(json!({
        "items": items.into_iter().map(tidal_video_to_resp).collect::<Vec<_>>()
    })))
}

#[derive(Debug, Deserialize)]
struct TidalPlaylistSearchParams {
    q: String,
    #[serde(default)]
    limit: Option<i32>,
    #[serde(default)]
    offset: Option<i32>,
}

async fn tidal_playlist_search(
    State(state): State<SharedState>,
    Query(params): Query<TidalPlaylistSearchParams>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let tokens = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
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
    let offset = params.offset.unwrap_or(0).max(0);
    let (http_client, tidal_http_client) = {
        let s = state.read().await;
        (s.http_client.clone(), s.tidal_http_client.clone())
    };
    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let playlists = match client.search_playlists(&params.q, limit, offset).await {
        Ok(r) => r,
        Err(e) if error_looks_like_auth(&e) => {
            let refreshed = recover_tidal_session(&state, &http_client, &tokens)
                .await
                .map_err(|re| {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({ "error": format!("TIDAL session refresh failed: {}", re) })),
                    )
                })?;
            let retry_client = TidalClient::with_http(
                tidal_http_client,
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            retry_client
                .search_playlists(&params.q, limit, offset)
                .await
                .map_err(|e2| {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({ "error": e2.to_string() })),
                    )
                })?
        }
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

async fn tidal_playlist_tracks(
    State(state): State<SharedState>,
    Path(uuid): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let tokens = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
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

    let (http_client, tidal_http_client) = {
        let s = state.read().await;
        (s.http_client.clone(), s.tidal_http_client.clone())
    };
    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let resp = match client.get_playlist_tracks(&uuid, 100, 0).await {
        Ok(r) => r,
        Err(e) if error_looks_like_auth(&e) => {
            let refreshed = recover_tidal_session(&state, &http_client, &tokens)
                .await
                .map_err(|re| {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({ "error": format!("TIDAL session refresh failed: {}", re) })),
                    )
                })?;
            let retry_client = TidalClient::with_http(
                tidal_http_client,
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            retry_client
                .get_playlist_tracks(&uuid, 100, 0)
                .await
                .map_err(|e2| {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({ "error": e2.to_string() })),
                    )
                })?
        }
        Err(e) => {
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": e.to_string() })),
            ));
        }
    };

    // TidalTrack: id (i64), title (String), duration (i64), artist (TidalArtist, not Option),
    // album: Option<TidalAlbumRef> with cover: Option<String>
    let playable: Vec<serde_json::Value> = resp
        .items
        .iter()
        .map(|t| {
            json!({
                "tidal_id": t.id,
                "title": t.title,
                "artist_name": t.artist.name,
                "album_title": t.album.as_ref().map(|a| &a.title),
                "artwork_url": t.album.as_ref().and_then(|a| a.cover.as_ref()).and_then(|c| {
                    TidalClient::get_artwork_url(&Some(c.clone()), 640)
                }),
                "duration_ms": t.duration * 1000,
                "track_id": 0,
                "is_in_library": false,
            })
        })
        .collect();

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
    // If the picked track is sitting in the pending mix queue, jump to it —
    // drop entries before it (and the entry itself, which we're about to
    // start) and leave the rest queued. Only fully clear when the user
    // chose something outside the current mix.
    {
        let s = state.read().await;
        let mut q = s.pending_tidal_mix_queue.lock().unwrap();
        match q
            .iter()
            .position(|p| p.tidal_track_id == body.tidal_track_id)
        {
            Some(idx) => {
                q.drain(..=idx);
            }
            None => q.clear(),
        }
    }
    let track = crate::PendingEphemeralTidalTrack {
        tidal_track_id: body.tidal_track_id,
        title: body.title,
        artist_name: body.artist_name,
        album_title: body.album_title,
        artwork_url: body.artwork_url,
        duration_ms: body.duration_ms,
    };
    start_ephemeral_tidal_playback(&state, track).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, serde::Deserialize)]
struct PlayTidalMixRequest {
    tracks: Vec<PlayTidalRequest>,
}

/// Play the first track immediately and stash the rest in the pending
/// ephemeral queue so `handle_runtime_finished` can advance through them.
/// Used by the home Your Mixes shelf when a tile is clicked.
async fn play_tidal_mix(
    State(state): State<SharedState>,
    Json(body): Json<PlayTidalMixRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if body.tracks.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Mix has no tracks" })),
        ));
    }

    let mut iter = body
        .tracks
        .into_iter()
        .map(|t| crate::PendingEphemeralTidalTrack {
            tidal_track_id: t.tidal_track_id,
            title: t.title,
            artist_name: t.artist_name,
            album_title: t.album_title,
            artwork_url: t.artwork_url,
            duration_ms: t.duration_ms,
        });
    let first = iter.next().expect("non-empty per check above");
    let rest: Vec<crate::PendingEphemeralTidalTrack> = iter.collect();

    // Replace any existing pending queue with this mix's continuation.
    {
        let s = state.read().await;
        let mut q = s.pending_tidal_mix_queue.lock().unwrap();
        q.clear();
        q.extend(rest);
    }

    start_ephemeral_tidal_playback(&state, first).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Resolve a TIDAL stream URL and start ephemeral playback. Shared by the
/// single-track entry point (`play_tidal_ephemeral`), the mix entry point
/// (`play_tidal_mix`), and the auto-advance hook (`handle_runtime_finished`)
/// when stepping through a queued mix.
async fn start_ephemeral_tidal_playback(
    state: &SharedState,
    track: crate::PendingEphemeralTidalTrack,
) -> Result<(), (StatusCode, Json<Value>)> {
    // Resolve TIDAL tokens (same pattern as tidal_search)
    let (tokens, http_client, tidal_http_client) = {
        let persisted = load_persisted_tidal_tokens(state).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;
        let s = state.read().await;
        (
            s.tidal_tokens.clone().or(persisted),
            s.http_client.clone(),
            s.tidal_http_client.clone(),
        )
    };

    let Some(tokens) = tokens else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    // Backstop for callers that don't ship artwork (Spotify-resolved playlist
    // tracks below the fold never trigger the lazy IntersectionObserver, so
    // they arrive with `artwork_url: null`). Look up the TIDAL track once and
    // reuse its album cover at the standard 640×640 size.
    let mut track = track;
    if track.artwork_url.is_none() {
        let lookup_client = TidalClient::with_http(
            tidal_http_client.clone(),
            tokens.access_token.clone(),
            tokens.country_code.clone(),
        );
        if let Ok(t) = lookup_client.get_track(track.tidal_track_id).await {
            track.artwork_url = t
                .album
                .as_ref()
                .and_then(|a| a.cover.as_ref())
                .and_then(|c| TidalClient::get_artwork_url(&Some(c.clone()), 640));
        }
    }

    let stream_req = tidal_stream::StreamRequest::new(track.tidal_track_id, "LOSSLESS");
    let stream_info =
        match tidal_stream::resolve_stream(&http_client, &tokens.access_token, &stream_req).await {
            Ok(info) => info,
            Err(e) if e.is_session_expired() => {
                let refreshed = recover_tidal_session(state, &http_client, &tokens)
                    .await
                    .map_err(|re| {
                        (
                            StatusCode::BAD_GATEWAY,
                            Json(
                                json!({ "error": format!("TIDAL session refresh failed: {}", re) }),
                            ),
                        )
                    })?;
                tidal_stream::resolve_stream(&http_client, &refreshed.access_token, &stream_req)
                    .await
                    .map_err(|e2| {
                        (
                            StatusCode::BAD_GATEWAY,
                            Json(json!({ "error": format!("TIDAL stream resolve failed: {e2}") })),
                        )
                    })?
            }
            Err(e) => {
                return Err((
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": format!("TIDAL stream resolve failed: {e}") })),
                ));
            }
        };

    // Build a synthetic Track with a negative id to avoid any DB collision
    let synthetic = crate::db::models::Track {
        id: -track.tidal_track_id,
        title: track.title.clone(),
        artist_id: 0,
        artist_name: track.artist_name.clone(),
        album_id: None,
        album_title: track.album_title.clone(),
        disc_number: None,
        track_number: None,
        duration_ms: track.duration_ms,
        isrc: None,
        tidal_id: Some(track.tidal_track_id),
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
        artwork_url: track.artwork_url.clone(),
    };

    let playback_generation = bump_playback_generation(state).await;
    let snapshot = {
        let mut state_guard = state.write().await;
        state_guard.external_playback_track = None;
        state_guard.ephemeral_tidal_track = Some(synthetic.clone());
        state_guard.current_stream_display = Some(crate::StreamDisplayInfo {
            audio_quality: stream_info.audio_quality.clone(),
            sample_rate: stream_info.sample_rate,
            bit_depth: stream_info.bit_depth,
        });
        state_guard.pending_stream_display = None;
        let snapshot = state_guard
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE playback_state
                     SET current_track_id = NULL,
                         current_queue_item_id = NULL,
                         position_ms = 0,
                         is_playing = 1
                     WHERE id = 1",
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

    // Build the playback job and start it via the runtime
    let crossfade_ms = current_crossfade_ms(state).await;
    let job =
        player::build_playback_preparation(&synthetic, Some(&stream_info), crossfade_ms, None)
            .with_generation(playback_generation);
    let runtime_handle = match ensure_playback_runtime_for_track(state, &synthetic).await {
        Ok(handle) => handle,
        Err(error) => {
            let state_guard = state.read().await;
            let _ = state_guard.db.with_conn(player::pause);
            return Err(error);
        }
    };
    align_device_to_stream_rate(state, &runtime_handle, &stream_info).await;
    runtime_handle.play(job).map_err(|e| {
        let message = format!("Failed to start host audio playback: {e}");
        report_playback_failure(state, &message);
        if let Ok(state_guard) = state.try_read() {
            let _ = state_guard.db.with_conn(player::pause);
        }
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": message })),
        )
    })?;

    sync_session_after_snapshot(
        state,
        &snapshot,
        Some(player::ListenSessionEndReason::Replaced),
    )
    .await;

    Ok(())
}

async fn tidal_artist_profile(
    State(state): State<SharedState>,
    Path(tidal_artist_id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (tokens, http_client, tidal_http_client) = {
        let persisted = load_persisted_tidal_tokens(&state).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;
        let s = state.read().await;
        (
            s.tidal_tokens.clone().or(persisted),
            s.http_client.clone(),
            s.tidal_http_client.clone(),
        )
    };

    let Some(tokens) = tokens else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let (top_tracks_page, albums_page) = match tokio::try_join!(
        client.get_artist_top_tracks(tidal_artist_id, 10, 0),
        client.get_artist_albums(tidal_artist_id, 50, 0, Some("ALBUMS")),
    ) {
        Ok(pair) => pair,
        Err(e) if error_looks_like_auth(&e) => {
            let refreshed = recover_tidal_session(&state, &http_client, &tokens)
                .await
                .map_err(|re| {
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({ "error": format!("TIDAL session refresh failed: {}", re) })),
                    )
                })?;
            let retry_client = TidalClient::with_http(
                tidal_http_client.clone(),
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            tokio::try_join!(
                retry_client.get_artist_top_tracks(tidal_artist_id, 10, 0),
                retry_client.get_artist_albums(tidal_artist_id, 50, 0, Some("ALBUMS")),
            )
            .map_err(|e2| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": e2.to_string() })),
                )
            })?
        }
        Err(e) => {
            return Err((
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": e.to_string() })),
            ));
        }
    };

    // Fetch the artist's own profile separately so a transient failure
    // (rate-limit, 404 on the artist endpoint) doesn't kill the whole
    // route — top-tracks/albums already loaded successfully above.
    let artist_profile = {
        let probe = TidalClient::with_http(
            tidal_http_client.clone(),
            tokens.access_token.clone(),
            tokens.country_code.clone(),
        );
        match probe.get_artist(tidal_artist_id).await {
            Ok(a) => Some(a),
            Err(e) => {
                tracing::debug!(
                    "tidal_artist_profile: artist record fetch failed for {}: {}",
                    tidal_artist_id,
                    e
                );
                None
            }
        }
    };

    let artist_name = artist_profile
        .as_ref()
        .map(|a| a.name.clone())
        .or_else(|| top_tracks_page.items.first().map(|t| t.artist.name.clone()));
    let picture_url = artist_profile
        .as_ref()
        .and_then(|a| TidalClient::get_artwork_url(&a.picture, 320));

    let top_tracks: Vec<serde_json::Value> = top_tracks_page
        .items
        .iter()
        .map(|t| {
            let artwork_url =
                TidalClient::get_artwork_url(&t.album.as_ref().and_then(|a| a.cover.clone()), 320);
            json!({
                "tidal_id": t.id,
                "title": t.title,
                "duration_ms": t.duration * 1000,
                "artwork_url": artwork_url,
                "album_title": t.album.as_ref().map(|a| &a.title),
                "album_tidal_id": t.album.as_ref().map(|a| a.id),
                "artist_name": t.artist.name,
                "artist_tidal_id": t.artist.id,
            })
        })
        .collect();

    let albums: Vec<serde_json::Value> = albums_page
        .items
        .iter()
        .map(|a| {
            let artwork_url = TidalClient::get_artwork_url(&a.cover, 320);
            json!({
                "tidal_id": a.id,
                "local_id": null,
                "title": a.title,
                "artwork_url": artwork_url,
                "release_date": a.release_date,
                "release_type": a.release_type,
                "number_of_tracks": a.number_of_tracks,
                "artist_name": a.artist.name,
                "in_library": false,
            })
        })
        .collect();

    Ok(Json(json!({
        "artist_name": artist_name,
        "picture_url": picture_url,
        "top_tracks": top_tracks,
        "albums": albums,
    })))
}

/// Get sync info (last sync time, auto-sync settings).
async fn get_sync_info(
    State(state): State<SharedState>,
    Query(params): Query<serde_json::Map<String, serde_json::Value>>,
) -> Result<Json<Value>, StatusCode> {
    let service = params
        .get("service")
        .and_then(|v| v.as_str())
        .unwrap_or("tidal");
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
            Ok(Json(
                json!({ "service": service, "auto_sync_daily": payload.enabled }),
            ))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Public function to trigger auto-sync from server startup.
pub async fn trigger_auto_sync(state: &SharedState, service: &str) -> anyhow::Result<SyncStats> {
    use std::sync::atomic::Ordering;

    if service != "tidal" {
        return Err(anyhow::anyhow!(
            "Unsupported auto-sync service: {}",
            service
        ));
    }

    // Get tokens + reentrancy/cancel flags
    let persisted_tokens = load_persisted_tidal_tokens(state).await?;
    let (tokens, running_flag, cancel_flag, tidal_http_client) = {
        let s = state.read().await;
        let tokens = s
            .tidal_tokens
            .clone()
            .or(persisted_tokens)
            .ok_or_else(|| anyhow::anyhow!("No TIDAL tokens available for auto-sync"))?;
        (
            tokens,
            s.tidal_sync_running.clone(),
            s.tidal_sync_cancel.clone(),
            s.tidal_http_client.clone(),
        )
    };

    if running_flag
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(anyhow::anyhow!(
            "TIDAL sync is already running; auto-sync skipped"
        ));
    }
    cancel_flag.store(false, Ordering::SeqCst);
    let _running = TidalSyncRunningGuard(running_flag);

    let client = TidalClient::with_http(
        tidal_http_client,
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );

    // Run sync
    let result = run_tidal_sync_with_reauth(&client, state, tokens, &cancel_flag).await;
    match result {
        Ok(stats) => {
            // Record sync timestamp
            state.read().await.db.with_conn(|conn| {
                queries::update_sync_timestamp(
                    conn,
                    "tidal",
                    stats.tracks as i64,
                    stats.albums as i64,
                )
            })?;

            // Broadcast completion
            let s = state.read().await;
            let _ = s.event_tx.send(AppEvent::LibrarySynced);
            Ok(stats)
        }
        Err(e) => {
            let s = state.read().await;
            let _ = s.event_tx.send(AppEvent::SyncFailed {
                service: "tidal".to_string(),
                message: e.to_string(),
            });
            Err(e)
        }
    }
}

/// Sync TIDAL library into local database.
async fn tidal_sync_library(
    State(state): State<SharedState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use std::sync::atomic::Ordering;

    // Get tokens and reentrancy/cancel flags
    let persisted_tokens = load_persisted_tidal_tokens(&state).await.map_err(|error| {
        TidalSyncStartError::SessionCheckFailed(error.to_string()).into_response()
    })?;
    let (tokens, running_flag, cancel_flag, tidal_http_client) = {
        let s = state.read().await;
        let tokens = s
            .tidal_tokens
            .clone()
            .or(persisted_tokens)
            .ok_or(TidalSyncStartError::NotConnected)?;
        (
            tokens,
            s.tidal_sync_running.clone(),
            s.tidal_sync_cancel.clone(),
            s.tidal_http_client.clone(),
        )
    };

    // Reentrancy guard — refuse to start a second concurrent sync.
    if running_flag
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(TidalSyncStartError::AlreadyRunning.into_response());
    }
    cancel_flag.store(false, Ordering::SeqCst);

    // From here on, any early return MUST release the running flag — wrap the
    // setup phase in a RAII guard. The spawned task will take ownership of the
    // guard via mem::replace once the work actually starts.
    let mut setup_guard = Some(TidalSyncRunningGuard(running_flag.clone()));

    // Create TIDAL client
    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );

    let (session, session_state) = ensure_tidal_session(&state, &tokens, &client)
        .await
        .map_err(|error| {
            // setup_guard drops here on early-return path → releases running.
            drop(setup_guard.take());
            error.into_response()
        })?;

    // Hand the guard off to the background task so the flag stays set for the
    // entire sync duration (and is released on completion or panic).
    let task_guard = setup_guard.take().expect("guard still held");

    // Run sync in background
    let state_clone = state.clone();
    let sync_tokens = session.clone();
    let cancel_for_task = cancel_flag.clone();
    let http_for_task = tidal_http_client;
    tokio::spawn(async move {
        let _running = task_guard; // released on scope exit
        tracing::info!(
            target: "noor.sync.tidal",
            event = "background_start",
            session_state = session_state.as_str(),
            user_id = %sync_tokens.user_id,
            "TIDAL sync background task started"
        );
        let client = TidalClient::with_http(
            http_for_task,
            sync_tokens.access_token.clone(),
            sync_tokens.country_code.clone(),
        );
        match run_tidal_sync_with_reauth(&client, &state_clone, sync_tokens, &cancel_for_task).await
        {
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
                if let Err(e) = state_clone.read().await.db.with_conn(|conn| {
                    queries::update_sync_timestamp(
                        conn,
                        "tidal",
                        stats.tracks as i64,
                        stats.albums as i64,
                    )
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
                let s = state_clone.read().await;
                let _ = s.event_tx.send(AppEvent::SyncFailed {
                    service: "tidal".to_string(),
                    message: e.to_string(),
                });
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

/// Cancel the in-flight TIDAL sync. Sets the cancel flag; the running task
/// observes it between pages and returns early. Always returns 200 — the
/// frontend uses this idempotently and doesn't care whether a sync was actually
/// running.
async fn tidal_sync_cancel(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    use std::sync::atomic::Ordering;
    let s = state.read().await;
    let was_running = s.tidal_sync_running.load(Ordering::SeqCst);
    s.tidal_sync_cancel.store(true, Ordering::SeqCst);
    Ok(Json(
        json!({ "status": if was_running { "cancelling" } else { "idle" } }),
    ))
}

/// Perform the actual TIDAL sync (runs in background task).
async fn do_tidal_sync(
    client: &TidalClient,
    state: &SharedState,
    user_id: &str,
    cancel: &std::sync::atomic::AtomicBool,
) -> anyhow::Result<SyncStats> {
    use crate::services::tidal::client::TidalClient as TC;
    use futures::stream::{self, StreamExt};
    use std::sync::atomic::Ordering;

    let check_cancel = || -> anyhow::Result<()> {
        if cancel.load(Ordering::SeqCst) {
            anyhow::bail!("TIDAL sync cancelled");
        }
        Ok(())
    };

    let mut stats = SyncStats::default();
    let mut favorite_album_ids = HashSet::new();
    let mut favorite_track_ids = HashSet::new();

    // Read previous run's counts so `apply_tidal_favorite_flags` can refuse to
    // wipe favorites if this run somehow returns zero items.
    let (prev_track_count, prev_album_count) = {
        let s = state.read().await;
        s.db.with_conn(|conn| {
            Ok(queries::get_sync_info(conn, "tidal")?
                .map(|i| (i.last_sync_track_count, i.last_sync_album_count))
                .unwrap_or((0, 0)))
        })?
    };

    // ── Sync favorite artists ────────────────────────
    tracing::info!("Syncing TIDAL artists...");
    let mut offset = 0;
    loop {
        check_cancel()?;
        let resp = client.get_favorite_artists(user_id, 100, offset).await?;
        if resp.items.is_empty() {
            break;
        }
        let artist_total = resp
            .total_number_of_items
            .unwrap_or((offset + resp.items.len() as i32) as i64)
            .max(1) as f32;
        {
            let s = state.read().await;
            s.db.with_conn(|conn| {
                let tx = conn.unchecked_transaction()?;
                for fav in &resp.items {
                    let a = &fav.item;
                    let photo = a.picture.as_ref().map(|p| {
                        let path = p.replace('-', "/");
                        format!("https://resources.tidal.com/images/{}/480x480.jpg", path)
                    });
                    tx.execute(
                        "INSERT INTO artists (tidal_id, name, photo_url) VALUES (?1, ?2, ?3)
                         ON CONFLICT(tidal_id) DO UPDATE SET name=excluded.name, photo_url=COALESCE(excluded.photo_url, artists.photo_url)",
                        rusqlite::params![a.id, a.name, photo],
                    )?;
                    stats.artists += 1;
                }
                tx.commit()?;
                Ok(())
            })?;
        }
        offset += resp.items.len() as i32;
        // Artists phase shows up as 0.0 → 0.05 — small but non-zero so users see
        // movement during what used to be a silent phase.
        let artist_progress = ((offset as f32 / artist_total) * 0.05).clamp(0.0, 0.05);
        send_tidal_sync_progress(state, artist_progress).await;
        if resp
            .total_number_of_items
            .is_none_or(|t| offset as i64 >= t)
        {
            break;
        }
    }
    tracing::info!("Synced {} artists", stats.artists);

    // ── Sync favorite albums ─────────────────────────
    tracing::info!("Syncing TIDAL albums...");
    offset = 0;
    loop {
        check_cancel()?;
        let resp = client.get_favorite_albums(user_id, 100, offset).await?;
        if resp.items.is_empty() {
            break;
        }
        // Batch the page's album upserts in one transaction.
        {
            let s = state.read().await;
            s.db.with_conn(|conn| {
                let tx = conn.unchecked_transaction()?;
                for fav in &resp.items {
                    let album = &fav.item;
                    let artwork = TC::get_artwork_url(&album.cover, 640);
                    let year: Option<i32> = album
                        .release_date
                        .as_ref()
                        .and_then(|d| d.split('-').next())
                        .and_then(|y| y.parse().ok());
                    let photo = album.artist.picture.as_ref().map(|p| {
                        let path = p.replace('-', "/");
                        format!("https://resources.tidal.com/images/{}/480x480.jpg", path)
                    });
                    tx.execute(
                        "INSERT INTO artists (tidal_id, name, photo_url) VALUES (?1, ?2, ?3)
                         ON CONFLICT(tidal_id) DO UPDATE SET name=excluded.name, photo_url=COALESCE(excluded.photo_url, artists.photo_url)",
                        rusqlite::params![album.artist.id, album.artist.name, photo],
                    )?;
                    tx.execute(
                        "INSERT INTO albums (tidal_id, title, artist_id, year, artwork_url, release_type, track_count, is_favorite, source)
                         VALUES (?1, ?2, (SELECT id FROM artists WHERE tidal_id=?3), ?4, ?5, ?6, ?7, 1, 'tidal')
                         ON CONFLICT(tidal_id) DO UPDATE SET title=excluded.title, year=COALESCE(excluded.year, albums.year),
                         artwork_url=COALESCE(excluded.artwork_url, albums.artwork_url), track_count=COALESCE(excluded.track_count, albums.track_count),
                         is_favorite=1",
                        rusqlite::params![album.id, album.title, album.artist.id, year, artwork, album.release_type, album.number_of_tracks],
                    )?;
                }
                tx.commit()?;
                Ok(())
            })?;
        }
        for fav in &resp.items {
            stats.albums += 1;
            favorite_album_ids.insert(fav.item.id);
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
            check_cancel()?;
            // Bound each per-album fetch so a single hung Tidal request can't
            // stall the chunk for ~30s (reqwest default). One retry on error
            // or timeout handles transient network blips; a second timeout
            // surfaces as an Err that the loop below quietly skips.
            let album_fetch_timeout = std::time::Duration::from_secs(15);
            let mut fetches = stream::iter(album_chunk.iter().copied())
                .map(|album_id| async move {
                    let first = tokio::time::timeout(
                        album_fetch_timeout,
                        client.get_album_tracks(album_id),
                    )
                    .await;
                    match first {
                        Ok(Ok(resp)) => Ok(resp),
                        _ => match tokio::time::timeout(
                            album_fetch_timeout,
                            client.get_album_tracks(album_id),
                        )
                        .await
                        {
                            Ok(result) => result,
                            Err(_) => Err(anyhow::anyhow!(
                                "get_album_tracks timed out twice for album {album_id}"
                            )),
                        },
                    }
                })
                .buffer_unordered(10);

            while let Some(result) = fetches.next().await {
                if let Ok(tracks_resp) = result {
                    let s = state.read().await;
                    s.db.with_conn(|conn| {
                        let tx = conn.unchecked_transaction()?;
                        for track in &tracks_resp.items {
                            insert_tidal_track(&tx, track, false)?;
                            stats.tracks += 1;
                        }
                        tx.commit()?;
                        Ok(())
                    })?;
                }

                albums_hydrated_in_page += 1;
                let processed_albums = offset as usize + albums_hydrated_in_page;
                // Albums phase: 0.05 → 0.5. Artists phase ate 0.0–0.05.
                let progress_fraction =
                    (0.05 + (processed_albums as f32 / album_total) * 0.45).clamp(0.05, 0.5);
                send_tidal_sync_progress(state, progress_fraction).await;
            }
        }

        offset += resp.items.len() as i32;
        if resp
            .total_number_of_items
            .is_none_or(|t| offset as i64 >= t)
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
        check_cancel()?;
        let resp = client.get_favorite_tracks(user_id, 100, offset).await?;
        if resp.items.is_empty() {
            break;
        }
        {
            let s = state.read().await;
            s.db.with_conn(|conn| {
                let tx = conn.unchecked_transaction()?;
                for fav in &resp.items {
                    let track = &fav.item;
                    favorite_track_ids.insert(track.id);
                    // Ensure artist
                    tx.execute(
                        "INSERT INTO artists (tidal_id, name) VALUES (?1, ?2) ON CONFLICT(tidal_id) DO UPDATE SET name=excluded.name",
                        rusqlite::params![track.artist.id, track.artist.name],
                    )?;
                    // Ensure album ref
                    if let Some(ref album_ref) = track.album {
                        let artwork = TC::get_artwork_url(&album_ref.cover, 640);
                        tx.execute(
                            "INSERT OR IGNORE INTO albums (tidal_id, title, artist_id, artwork_url, is_favorite, source)
                             VALUES (?1, ?2, (SELECT id FROM artists WHERE tidal_id=?3), ?4, 0, 'tidal')",
                            rusqlite::params![album_ref.id, album_ref.title, track.artist.id, artwork],
                        )?;
                    }
                    insert_tidal_track(&tx, track, true)?;
                    stats.tracks += 1;
                }
                tx.commit()?;
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
            .is_none_or(|t| offset as i64 >= t)
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
            .is_none_or(|t| playlist_offset as i64 >= t)
        {
            break;
        }
    }
    let total_playlists = all_playlists.len().max(1);
    for (playlist_index, playlist) in all_playlists.iter().enumerate() {
        check_cancel()?;
        // Upsert the playlist row up front so metadata sticks even if the
        // track-fetch errors out partway. The DELETE+INSERT below for
        // `playlist_tracks` is wrapped in a single transaction so the playlist
        // never appears empty mid-sync.
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
            // Fetch all pages first, then DELETE+INSERT atomically. If the API
            // errors mid-fetch, the existing playlist contents stay intact.
            let mut all_tracks: Vec<crate::services::tidal::client::TidalTrack> = Vec::new();
            let mut track_offset = 0;
            loop {
                check_cancel()?;
                let tracks_resp = client
                    .get_playlist_tracks(&playlist.uuid, 100, track_offset)
                    .await?;
                if tracks_resp.items.is_empty() {
                    break;
                }
                let fetched = tracks_resp.items.len() as i32;
                all_tracks.extend(tracks_resp.items);
                track_offset += fetched;
                if tracks_resp
                    .total_number_of_items
                    .is_none_or(|t| track_offset as i64 >= t)
                {
                    break;
                }
            }

            let s = state.read().await;
            s.db.with_conn(|conn| {
                let tx = conn.unchecked_transaction()?;
                tx.execute(
                    "DELETE FROM playlist_tracks WHERE playlist_id=?1",
                    rusqlite::params![pid],
                )?;
                let mut position = 0;
                for track in &all_tracks {
                    tx.execute(
                        "INSERT INTO artists (tidal_id, name) VALUES (?1, ?2) ON CONFLICT(tidal_id) DO UPDATE SET name=excluded.name",
                        rusqlite::params![track.artist.id, track.artist.name],
                    )?;
                    if let Some(ref album_ref) = track.album {
                        let artwork = TC::get_artwork_url(&album_ref.cover, 640);
                        tx.execute(
                            "INSERT OR IGNORE INTO albums (tidal_id, title, artist_id, artwork_url, is_favorite, source)
                             VALUES (?1, ?2, (SELECT id FROM artists WHERE tidal_id=?3), ?4, 0, 'tidal')",
                            rusqlite::params![album_ref.id, album_ref.title, track.artist.id, artwork],
                        )?;
                    }
                    insert_tidal_track(&tx, track, false)?;

                    let track_id: Option<i64> = tx
                        .query_row(
                            "SELECT id FROM tracks WHERE tidal_id=?1",
                            rusqlite::params![track.id],
                            |row| row.get(0),
                        )
                        .ok();
                    if let Some(tid) = track_id {
                        tx.execute(
                            "INSERT OR IGNORE INTO playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
                            rusqlite::params![pid, tid, position],
                        )?;
                        position += 1;
                    }
                }
                tx.commit()?;
                Ok(())
            })?;
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
            apply_tidal_favorite_flags(conn, "albums", &favorite_album_ids, prev_album_count)?;
            apply_tidal_favorite_flags(conn, "tracks", &favorite_track_ids, prev_track_count)?;
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
    cancel: &std::sync::atomic::AtomicBool,
) -> anyhow::Result<SyncStats> {
    match do_tidal_sync(client, state, &tokens.user_id, cancel).await {
        Ok(stats) => Ok(stats),
        Err(err) if error_looks_like_auth(&err) => {
            tracing::warn!(
                target: "noor.sync.tidal",
                event = "sync_auth_failure",
                user_id = %tokens.user_id,
                error = %err,
                "TIDAL sync hit an auth error; trying refresh-token recovery"
            );

            let (http, tidal_http_client) = {
                let s = state.read().await;
                (s.http_client.clone(), s.tidal_http_client.clone())
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
            let retry_client = TidalClient::with_http(
                tidal_http_client,
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            tracing::info!(
                target: "noor.sync.tidal",
                event = "sync_recovered",
                user_id = %refreshed.user_id,
                "TIDAL sync session recovered; retrying sync"
            );
            do_tidal_sync(&retry_client, state, &refreshed.user_id, cancel).await
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
    AlreadyRunning,
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
            Self::AlreadyRunning => (
                StatusCode::CONFLICT,
                Json(json!({
                    "status": "already_running",
                    "message": "A TIDAL sync is already in progress."
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

/// RAII guard that flips `tidal_sync_running` back to `false` on drop, so a
/// panic or early return inside the spawned sync task can never leave the flag
/// stuck and lock out future syncs.
struct TidalSyncRunningGuard(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for TidalSyncRunningGuard {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
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
    let tidal_http_client = state.read().await.tidal_http_client.clone();
    let validation_client = TidalClient::with_http(
        tidal_http_client,
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

fn current_playback_generation(state: &crate::AppState) -> u64 {
    state
        .playback_generation
        .load(std::sync::atomic::Ordering::Relaxed)
}

async fn current_playback_generation_async(state: &SharedState) -> u64 {
    let state_guard = state.read().await;
    current_playback_generation(&state_guard)
}

async fn bump_playback_generation(state: &SharedState) -> u64 {
    let state_guard = state.read().await;
    state_guard
        .playback_generation
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        + 1
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
                Ok(playback_runtime::PlaybackRuntimeEvent::Finished {
                    track_id,
                    generation,
                }) => {
                    // Track is no longer producing audio — clear the flag before advancing.
                    state
                        .write()
                        .await
                        .audio_active
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    if let Err(error) =
                        handle_runtime_finished(state.clone(), track_id, generation).await
                    {
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
                    state_guard
                        .audio_active
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    let last_error = state_guard
                        .playback_runtime_info
                        .as_ref()
                        .and_then(|info| info.last_error.clone());
                    let prev_exclusive = state_guard
                        .playback_runtime_info
                        .as_ref()
                        .map(|i| i.exclusive_engaged)
                        .unwrap_or(false);
                    state_guard.playback_runtime_info = Some(PlaybackRuntimeInfo {
                        device_name,
                        sample_rate,
                        channels,
                        active_track_id: None,
                        last_error,
                        exclusive_engaged: prev_exclusive,
                    });
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::Started {
                    track_id,
                    generation,
                    ..
                }) => {
                    let mut state_guard = state.write().await;
                    if current_playback_generation(&state_guard) != generation {
                        continue;
                    }
                    // CPAL buffer threshold crossed — samples are actually flowing now.
                    state_guard
                        .audio_active
                        .store(true, std::sync::atomic::Ordering::Relaxed);
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
                    state_guard
                        .audio_active
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    if let Some(info) = state_guard.playback_runtime_info.as_mut() {
                        info.active_track_id = None;
                    }
                    state_guard.external_playback_track = None;
                    state_guard.ephemeral_tidal_track = None;
                    state_guard.current_stream_display = None;
                    state_guard.pending_stream_display = None;
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::NearEnd {
                    track_id,
                    generation,
                }) => {
                    // Pre-decode the next track so the transition is gapless.
                    if let Err(err) = handle_near_end(state.clone(), track_id, generation).await {
                        warn!("Failed to pre-buffer next track: {err:?}");
                    }
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::ExclusiveModeEngaged {
                    device_name,
                }) => {
                    let mut state_guard = state.write().await;
                    if let Some(info) = state_guard.playback_runtime_info.as_mut() {
                        info.exclusive_engaged = true;
                    }
                    let _ = state_guard.event_tx.send(AppEvent::AudioExclusiveEngaged {
                        device: device_name,
                    });
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::ExclusiveModeFailed {
                    reason,
                    device_name,
                }) => {
                    let mut state_guard = state.write().await;
                    if let Some(info) = state_guard.playback_runtime_info.as_mut() {
                        info.exclusive_engaged = false;
                    }
                    let _ = state_guard.event_tx.send(AppEvent::AudioExclusiveFailed {
                        device: device_name,
                        reason,
                    });
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::ExclusiveModeReleased {
                    device_name,
                }) => {
                    let mut state_guard = state.write().await;
                    if let Some(info) = state_guard.playback_runtime_info.as_mut() {
                        info.exclusive_engaged = false;
                    }
                    let _ = state_guard.event_tx.send(AppEvent::AudioExclusiveReleased {
                        device: device_name,
                    });
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
async fn handle_near_end(
    state: SharedState,
    current_track_id: i64,
    generation: u64,
) -> anyhow::Result<()> {
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
        if current_playback_generation(&state_guard) != generation {
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

        let cleared = recently_cleared(&state_guard);
        let next = state_guard
            .db
            .with_conn(|conn| player::peek_next_track(conn, cleared))?;
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

    let stream_info = match resolve_tidal_playback_stream(&state, &next, &stream_request).await {
        Ok(info) => Some(info),
        Err(error) => {
            warn!(
                "Skipping pre-buffer for next track {}: {}",
                next.id,
                describe_tidal_playback_error(&error)
            );
            return Ok(());
        }
    };

    {
        let state_guard = state.read().await;
        let runtime_token = state_guard
            .playback_runtime
            .as_ref()
            .map(|runtime| runtime.access_token.as_str());
        let current_token = state_guard
            .tidal_tokens
            .as_ref()
            .map(|tokens| tokens.access_token.as_str());
        if runtime_token != current_token {
            info!(
                "Skipping pre-buffer for next track {} after TIDAL session refresh; next transition will cold-start",
                next.id
            );
            return Ok(());
        }
    };

    {
        let state_guard = state.read().await;
        let active_id = state_guard
            .playback_runtime_info
            .as_ref()
            .and_then(|info| info.active_track_id);
        let db_current = state_guard
            .db
            .with_conn(player::current_track_id)
            .unwrap_or(None);
        let cleared = recently_cleared(&state_guard);
        let still_next = state_guard
            .db
            .with_conn(|conn| player::peek_next_track(conn, cleared))
            .ok()
            .flatten()
            .map(|track| track.id);
        if active_id != Some(current_track_id)
            || db_current != Some(current_track_id)
            || current_playback_generation(&state_guard) != generation
            || still_next != Some(next.id)
        {
            return Ok(());
        }
    }

    // If sample_rate_follow is enabled and the next track's rate differs from current,
    // rebuild the output device at the new rate before PrepareNext.
    {
        let state_guard = state.read().await;
        if let (Some(stream), Some(info)) =
            (stream_info.as_ref(), &state_guard.playback_runtime_info)
        {
            let audio_settings = state_guard
                .db
                .with_conn(|conn| {
                    crate::db::audio_settings::load(conn).map_err(anyhow::Error::from)
                })
                .ok();
            if let Some(settings) = audio_settings
                && settings.sample_rate_follow
                && let Some(next_rate) = stream.sample_rate
            {
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
                        settings.exclusive_release_grace_secs,
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

    let _gapless = crate::playback::gapless::plan_from_stream(
        stream_info.as_ref(),
        crate::playback::gapless::GaplessSettings::new(true, crossfade_ms),
    );
    let job =
        player::build_playback_preparation(&next, stream_info.as_ref(), crossfade_ms, user_quality)
            .with_generation(generation);

    {
        let state_guard = state.read().await;
        let active_id = state_guard
            .playback_runtime_info
            .as_ref()
            .and_then(|info| info.active_track_id);
        let db_current = state_guard
            .db
            .with_conn(player::current_track_id)
            .unwrap_or(None);
        if active_id != Some(current_track_id) || db_current != Some(current_track_id) {
            return Ok(());
        }
        if current_playback_generation(&state_guard) != generation {
            return Ok(());
        }
    }

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

async fn handle_runtime_finished(
    state: SharedState,
    finished_track_id: i64,
    generation: u64,
) -> anyhow::Result<()> {
    {
        let state_guard = state.read().await;
        if current_playback_generation(&state_guard) != generation {
            return Ok(());
        }
    }

    let external_finished = {
        let state_guard = state.read().await;
        state_guard
            .external_playback_track
            .as_ref()
            .map(|track| track.id == finished_track_id)
            .unwrap_or(false)
            || state_guard
                .ephemeral_tidal_track
                .as_ref()
                .map(|track| track.id == finished_track_id)
                .unwrap_or(false)
    };
    if external_finished {
        // Auto-advance through any queued ephemeral mix continuation before
        // tearing down. Pop the next track out of the pending queue and start
        // it; if the queue is empty, fall through to the existing teardown.
        let next = {
            let s = state.read().await;
            s.pending_tidal_mix_queue.lock().unwrap().pop_front()
        };
        if let Some(next) = next {
            // Clear the previous ephemeral track marker so the new one's
            // PlaybackStateChanged + Started events overwrite cleanly.
            {
                let mut state_guard = state.write().await;
                state_guard.external_playback_track = None;
                state_guard.ephemeral_tidal_track = None;
            }
            // Skip-and-retry advance: a single TIDAL hiccup (especially a 429
            // rate-limit a few tracks into a mix) used to nuke the entire
            // remaining queue. Now we treat 429 as recoverable (sleep + retry
            // the same track once) and any other failure as track-specific
            // (skip to the next item). Only tear down when the deque is empty
            // or we hit MAX_CONSEC_FAILURES distinct tracks failing in a row.
            const MAX_CONSEC_FAILURES: u32 = 3;
            let mut current = next;
            let mut consecutive_failures: u32 = 0;
            let mut started = false;
            loop {
                let mut result = start_ephemeral_tidal_playback(&state, current.clone()).await;
                if let Err((status, _)) = &result
                    && status.as_u16() == 429
                {
                    tracing::warn!(
                        "TIDAL 429 advancing mix to '{}' — backing off 3s and retrying once",
                        current.title
                    );
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    result = start_ephemeral_tidal_playback(&state, current.clone()).await;
                }
                match result {
                    Ok(()) => {
                        started = true;
                        break;
                    }
                    Err((status, body)) => {
                        consecutive_failures += 1;
                        tracing::warn!(
                            "Failed to advance to '{}' ({status}, fail {consecutive_failures}/{MAX_CONSEC_FAILURES}): {} — skipping",
                            current.title,
                            body.0
                        );
                        if consecutive_failures >= MAX_CONSEC_FAILURES {
                            tracing::warn!(
                                "Hit max consecutive failures advancing TIDAL mix; clearing remaining queue"
                            );
                            let s = state.read().await;
                            s.pending_tidal_mix_queue.lock().unwrap().clear();
                            break;
                        }
                        let popped = {
                            let s = state.read().await;
                            s.pending_tidal_mix_queue.lock().unwrap().pop_front()
                        };
                        match popped {
                            Some(p) => current = p,
                            None => break,
                        }
                    }
                }
            }
            if started {
                return Ok(());
            }
            // Fall through to teardown so the UI doesn't get stuck on a
            // ghost track.
        }
        {
            let mut state_guard = state.write().await;
            // Flush any in-flight listen session before tearing down so the
            // last track of an ephemeral mix isn't dropped on the floor.
            let flushed_track_id = match flush_active_listen_session_locked(
                &mut state_guard,
                chrono::Utc::now(),
                player::ListenSessionEndReason::QueueEnded,
            ) {
                Ok(outcome) => outcome.flushed_track_id,
                Err(err) => {
                    tracing::warn!("flush on ephemeral teardown failed: {err}");
                    None
                }
            };
            state_guard.external_playback_track = None;
            state_guard.ephemeral_tidal_track = None;
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
            if let Some(track_id) = flushed_track_id {
                let _ = state_guard
                    .event_tx
                    .send(AppEvent::ListenHistoryUpdated { track_id });
            }
        }
        let state_guard = state.read().await;
        let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
        return Ok(());
    }

    let snapshot = {
        let state_guard = state.read().await;
        let cleared = recently_cleared(&state_guard);
        state_guard.db.with_conn(|conn| {
            let current_track_id = player::current_track_id(conn)?;
            let current_state = player::load_state(conn)?;
            if current_track_id != Some(finished_track_id) || !current_state.is_playing {
                return Ok(None);
            }

            let snapshot = player::next_track(conn, cleared)?;
            Ok(Some(snapshot))
        })?
    };

    let Some(snapshot) = snapshot else {
        // Track-id mismatch (e.g. user already advanced to another track via
        // play_track, or playback was paused). The runtime-finished session
        // is for `finished_track_id` and may still be sitting in
        // active_listen_session — flush it before bailing so the partial
        // listen isn't lost. Direct flush, not sync_session_after_snapshot:
        // we don't want to start a new session for whatever DB state has now.
        let mut state_guard = state.write().await;
        let track_id_for_event = match flush_active_listen_session_locked(
            &mut state_guard,
            chrono::Utc::now(),
            player::ListenSessionEndReason::Stopped,
        ) {
            Ok(outcome) => outcome.flushed_track_id,
            Err(err) => {
                tracing::warn!("flush on runtime-finished mismatch failed: {err}");
                None
            }
        };
        if let Some(track_id) = track_id_for_event {
            let _ = state_guard
                .event_tx
                .send(AppEvent::ListenHistoryUpdated { track_id });
        }
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
        let runtime_handle = ensure_playback_runtime_for_track(&state, track)
            .await
            .map_err(|(status, body)| {
                anyhow::anyhow!("playback runtime unavailable ({status}): {}", body.0)
            })?;
        let prepared_status = runtime_handle.track_status(track.id, generation);
        if matches!(
            prepared_status,
            playback_runtime::PlaybackTrackStatus::Active
                | playback_runtime::PlaybackTrackStatus::Prepared
        ) {
            let job = player::build_playback_preparation(
                track,
                None,
                effective_crossfade_ms(&state, snapshot.state.crossfade_ms).await,
                user_quality,
            )
            .with_generation(generation);
            runtime_handle.switch_to(job)?;
            {
                let mut state_guard = state.write().await;
                if let Some(pending) = state_guard.pending_stream_display.take() {
                    state_guard.current_stream_display = Some(pending);
                }
            }
        } else {
            let Some(stream_request) =
                player::build_tidal_stream_request(track, user_quality.clone())
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
            let job = player::build_playback_preparation(
                track,
                Some(&stream_info),
                effective_crossfade_ms(&state, snapshot.state.crossfade_ms).await,
                user_quality,
            )
            .with_generation(generation);
            align_device_to_stream_rate(&state, &runtime_handle, &stream_info).await;
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
        state_guard.ephemeral_tidal_track = None;
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

async fn resume_session_after_snapshot(state: &SharedState, snapshot: &player::PlaybackSnapshot) {
    let Some(track) = snapshot.state.current_track.as_ref() else {
        return;
    };

    let mut state = state.write().await;
    let now = chrono::Utc::now();
    match state.active_listen_session.as_mut() {
        Some(session) if session.track_id == track.id => session.resume(now),
        _ => {
            let source = state
                .db
                .with_conn(|conn| Ok(player::lookup_current_listen_source(conn)))
                .unwrap_or(crate::db::models::ListenSource::Unknown);
            let prior = state.live_listen_session.as_ref();
            state.active_listen_session = Some(player::ActiveListenSession::start(
                track.id, now, source, prior,
            ));
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

// If sample-rate-follow is enabled and the freshly-resolved stream's native
// rate differs from the device's current rate, swap the device to the new
// rate before the track starts. Without this, the first track of a session
// (and any track played from a cold start) plays at the device's existing
// rate with a software resampler in the path — not bit-perfect. The
// next-track pre-buffer path already does this; this helper makes every
// play/switch site behave the same way.
async fn align_device_to_stream_rate(
    state: &SharedState,
    handle: &playback_runtime::PlaybackRuntimeHandle,
    stream_info: &tidal_stream::StreamInfo,
) {
    let (next_rate, settings, current_rate) = {
        let guard = state.read().await;
        let Some(info) = guard.playback_runtime_info.as_ref() else {
            return;
        };
        let Some(rate_i32) = stream_info.sample_rate else {
            return;
        };
        if rate_i32 <= 0 {
            return;
        }
        let next_rate = rate_i32 as u32;
        if next_rate == info.sample_rate {
            return;
        }
        let settings = match guard
            .db
            .with_conn(|conn| crate::db::audio_settings::load(conn).map_err(Into::into))
        {
            Ok(s) => s,
            Err(_) => return,
        };
        if !settings.sample_rate_follow {
            return;
        }
        (next_rate, settings, info.sample_rate)
    };

    let device_sel = match settings.output_device.as_ref() {
        Some(name) => playback_runtime::OutputDeviceSelection::Named(name.clone()),
        None => playback_runtime::OutputDeviceSelection::Default,
    };
    if let Err(e) = handle.device_swap(
        device_sel,
        settings.exclusive_mode,
        settings.sample_rate_follow,
        Some(next_rate),
        settings.exclusive_release_grace_secs,
    ) {
        warn!(
            "align_device_to_stream_rate: device_swap to {next_rate} Hz (from {current_rate} Hz) failed: {e}"
        );
    }
}

// ───── Audio output settings ────────────────────────────────────────────────
//
// `GET /api/audio/devices`     — enumerate cpal output devices
// `GET /api/audio/settings`    — current persisted AudioSettings
// `PUT /api/audio/settings`    — persist + (if device/exclusive/SR-follow changed) live-swap
// `POST /api/audio/exclusive/retry` — force a fresh DeviceSwap to retry exclusive grab

/// Re-issue the active output device's `DeviceSwap` so the runtime tries to
/// grab WASAPI exclusive again. Used by the "Retry" button on the red-pill
/// banner after the user has closed the blocking app.
async fn post_audio_exclusive_retry(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let guard = state.read().await;
    let settings = guard
        .db
        .with_conn(|conn| crate::db::audio_settings::load(conn).map_err(anyhow::Error::from))
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "message": e.to_string() })),
            )
        })?;

    if !settings.exclusive_mode {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "message": "exclusive_mode is off; nothing to retry"
            })),
        ));
    }

    if let Some(runtime) = guard.playback_runtime.as_ref()
        && let Err(e) = runtime.handle.device_swap(
            playback_runtime::OutputDeviceSelection::from_pref(settings.output_device.as_deref()),
            settings.exclusive_mode,
            settings.sample_rate_follow,
            None,
            settings.exclusive_release_grace_secs,
        )
    {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "message": e.to_string() })),
        ));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

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
    Json(mut new): Json<crate::db::audio_settings::AudioSettings>,
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
    // Clamp the user-facing grace setting so a malformed PUT can't disable
    // exclusive entirely (0) or wedge the device for an absurd duration.
    new.exclusive_release_grace_secs =
        crate::db::audio_settings::clamp_exclusive_release_grace_secs(
            new.exclusive_release_grace_secs,
        );

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
        // Grace-secs change is included so an active exclusive render thread
        // gets the new value next time it's re-grabbed (the running thread's
        // grace_secs is captured at construction; a swap rebuilds with the
        // new value).
        let needs_swap = old.output_device != saved.output_device
            || old.exclusive_mode != saved.exclusive_mode
            || old.sample_rate_follow != saved.sample_rate_follow
            || old.exclusive_release_grace_secs != saved.exclusive_release_grace_secs;

        if needs_swap
            && let Some(runtime) = guard.playback_runtime.as_ref()
            && let Err(e) = runtime.handle.device_swap(
                playback_runtime::OutputDeviceSelection::from_pref(saved.output_device.as_deref()),
                saved.exclusive_mode,
                saved.sample_rate_follow,
                None,
                saved.exclusive_release_grace_secs,
            )
        {
            warn!("Audio settings update: live device_swap failed: {e}");
        }

        (old, saved)
    };

    // Quality changed → re-issue the current track at the new quality so the
    // user immediately hears (and sees) the new tier. The track restarts from
    // 0; preserving position would require partial-stream offset support that
    // TIDAL's playbackinfo API doesn't expose.
    if old.quality != new.quality
        && let Err(e) = reissue_current_track_at_new_quality(&state).await
    {
        warn!("Audio settings update: re-issue at new quality failed: {e}");
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
    let generation = current_playback_generation_async(state).await;
    let job =
        player::build_playback_preparation(&track, Some(&stream_info), crossfade_ms, user_quality)
            .with_generation(generation);

    align_device_to_stream_rate(state, &handle, &stream_info).await;
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

/// True when the user manually cleared the queue within the last 60 seconds.
/// `ensure_automix_queue_depth` reads this so an immediately-following automix
/// pass doesn't refill the queue and visually negate the user's clear.
fn recently_cleared(state: &crate::AppState) -> bool {
    let cleared_at = state
        .user_cleared_at
        .load(std::sync::atomic::Ordering::Relaxed);
    if cleared_at == 0 {
        return false;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    now - cleared_at < 60
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
    // Capture both the flush outcome and a "now playing" payload (if a new
    // track just started) inside the write lock, then dispatch the Last.fm
    // calls outside the lock so the spawned tasks never contend with us.
    let (flushed_track_id, scrobble_completed_payload, now_playing_payload) = {
        let mut state = state.write().await;
        let now = chrono::Utc::now();

        let outcome = if let Some(reason) = end_reason {
            flush_active_listen_session_locked(&mut state, now, reason)
                .map_err(|err| {
                    error!("failed to flush listen session: {err}");
                })
                .unwrap_or(FlushOutcome {
                    flushed_track_id: None,
                    scrobble_completed: None,
                })
        } else {
            FlushOutcome {
                flushed_track_id: None,
                scrobble_completed: None,
            }
        };

        let mut now_playing: Option<(String, String, Option<String>, Option<i64>, String)> = None;
        if snapshot.state.is_playing {
            if let Some(track) = snapshot.state.current_track.as_ref() {
                let source = state
                    .db
                    .with_conn(|conn| Ok(player::lookup_current_listen_source(conn)))
                    .unwrap_or(crate::db::models::ListenSource::Unknown);
                let prior = state.live_listen_session.as_ref();
                state.active_listen_session = Some(player::ActiveListenSession::start(
                    track.id, now, source, prior,
                ));
                now_playing = Some((
                    track.artist_name.clone().unwrap_or_default(),
                    track.title.clone(),
                    track.album_title.clone(),
                    track.duration_ms,
                    track.source.clone(),
                ));
            }
        } else if snapshot.state.current_track.is_none() {
            state.active_listen_session = None;
        }

        (
            outcome.flushed_track_id,
            outcome.scrobble_completed,
            now_playing,
        )
    };

    if let Some(track_id) = flushed_track_id {
        let state_guard = state.read().await;
        let _ = state_guard
            .event_tx
            .send(AppEvent::ListenHistoryUpdated { track_id });
    }

    // Fire-and-forget Last.fm scrobbles. Helpers no-op when source != tidal,
    // when LASTFM_API_SECRET is unset, or when no session_key is stored.
    if let Some((artist, title, album, duration_ms, listened_ms, started_at_unix, source)) =
        scrobble_completed_payload
        && !artist.is_empty()
        && !title.is_empty()
    {
        crate::services::lastfm::scrobble::spawn_scrobble_completed(
            state.clone(),
            artist,
            title,
            album,
            duration_ms,
            listened_ms,
            started_at_unix,
            &source,
        );
    }
    if let Some((artist, title, album, duration_ms, source)) = now_playing_payload
        && !artist.is_empty()
        && !title.is_empty()
    {
        crate::services::lastfm::scrobble::spawn_now_playing(
            state.clone(),
            artist,
            title,
            album,
            duration_ms,
            &source,
        );
    }
}

pub(crate) struct FlushOutcome {
    flushed_track_id: Option<i64>,
    /// (artist, title, album, duration_ms, listened_ms, started_at_unix, source).
    /// `None` when there's nothing eligible to consider for a scrobble call.
    /// The actual eligibility + source filter happens in the scrobble helper.
    scrobble_completed: Option<(String, String, Option<String>, i64, i64, i64, String)>,
}

pub(crate) fn flush_active_listen_session_locked(
    state: &mut crate::AppState,
    now: chrono::DateTime<chrono::Utc>,
    _reason: player::ListenSessionEndReason,
) -> anyhow::Result<FlushOutcome> {
    let Some(mut session) = state.active_listen_session.take() else {
        return Ok(FlushOutcome {
            flushed_track_id: None,
            scrobble_completed: None,
        });
    };

    session.pause(now);
    let listened_ms = session.listened_ms_at(now);
    // Skip sessions shorter than 5 seconds to avoid spurious near-zero entries
    // from rapid track changes or accidental clicks.
    if listened_ms < 5_000 {
        return Ok(FlushOutcome {
            flushed_track_id: None,
            scrobble_completed: None,
        });
    }

    let started_at = session.started_at.to_rfc3339();
    let started_at_unix = session.started_at.timestamp();
    let track_id = session.track_id;
    if track_id <= 0 {
        return Ok(FlushOutcome {
            flushed_track_id: None,
            scrobble_completed: None,
        });
    }
    let session_id = session.session_id.clone();
    let source = session.source;
    let position_in_session = session.position_in_session;
    let transition_from_track_id = session.transition_from_track_id;
    let write_result = state.db.with_conn(|conn| {
        let track = queue::get_track_by_id(conn, track_id)?.ok_or_else(|| {
            anyhow::anyhow!("track {} missing when flushing listen session", track_id)
        })?;
        let completed = player::is_completed_listen(&track, listened_ms);
        queries::record_listen_history(
            conn,
            track_id,
            &started_at,
            listened_ms,
            completed,
            Some(&session_id),
            Some(source),
            Some(position_in_session),
            transition_from_track_id,
        )?;
        queries::increment_track_play_summary(conn, track_id, &started_at, completed)?;
        // Capture the fields needed for a Last.fm scrobble. The scrobble
        // helper itself does the source filter + eligibility check + silent
        // no-op when scrobbling isn't configured.
        let payload = (
            track.artist_name.clone().unwrap_or_default(),
            track.title.clone(),
            track.album_title.clone(),
            track.duration_ms.unwrap_or(0),
            listened_ms,
            started_at_unix,
            track.source.clone(),
        );
        Ok((completed, payload))
    });
    // On DB error: restore the session so the next flush attempt retries
    // (helpful for shutdown_handler and clear_tidal_session, which don't
    // immediately start a replacement session). Track-transition flushes
    // are still racy — the next sync_session_after_snapshot will overwrite
    // active_listen_session with a new track's session — but those weren't
    // recoverable before either.
    let (completed, scrobble_payload) = match write_result {
        Ok(v) => v,
        Err(err) => {
            state.active_listen_session = Some(session);
            return Err(err);
        }
    };
    state.live_listen_session = Some(session.to_live_session(now));

    tracing::info!(
        target: "noor.playback.history",
        track_id,
        listened_ms,
        completed,
        "flushed listen session"
    );

    Ok(FlushOutcome {
        flushed_track_id: Some(track_id),
        scrobble_completed: Some(scrobble_payload),
    })
}

async fn clear_tidal_session(state: &SharedState) -> anyhow::Result<()> {
    let mut s = state.write().await;
    if let Some(runtime) = s.playback_runtime.take() {
        let _ = runtime.handle.shutdown();
    }
    s.playback_runtime_info = None;
    // Flush any in-flight listen session so disconnecting TIDAL doesn't drop
    // the partial listen on the floor. flush_*_locked take()s the session on
    // success; if the DB write fails the session stays in s.active_listen_session
    // and is cleared by the explicit None below.
    if let Err(err) = flush_active_listen_session_locked(
        &mut s,
        chrono::Utc::now(),
        player::ListenSessionEndReason::Stopped,
    ) {
        tracing::warn!("flush on tidal disconnect failed: {err}");
    }
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
    favorite_ids: &HashSet<i64>,
    prev_count: i64,
) -> anyhow::Result<()> {
    // Refuse to wipe favorites if this run somehow returned zero items but the
    // previous run had a real population — almost always a transient TIDAL API
    // hiccup, not a legitimate "user unfavorited everything".
    if favorite_ids.is_empty() && prev_count > 0 {
        anyhow::bail!(
            "Refusing to clear is_favorite on '{}': sync returned 0 favorites but previous run had {}",
            table,
            prev_count
        );
    }

    // Scope the reset to TIDAL-sourced rows so manually-imported albums/tracks
    // (e.g. from `import_tidal_album`) keep whatever favorite state they had —
    // they aren't "TIDAL favorites" in the strict sync sense.
    let reset_sql = format!(
        "UPDATE {table} SET is_favorite = 0 WHERE source = 'tidal' AND tidal_id IS NOT NULL"
    );
    conn.execute(&reset_sql, [])?;

    let mut sorted_ids: Vec<i64> = favorite_ids.iter().copied().collect();
    sorted_ids.sort_unstable();

    for chunk in sorted_ids.chunks(800) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("UPDATE {table} SET is_favorite = 1 WHERE tidal_id IN ({placeholders})");
        conn.execute(&sql, rusqlite::params_from_iter(chunk.iter()))?;
    }

    Ok(())
}

// Returns `[]` in steady state: the Tidal v1 endpoints we use don't expose
// genre fields. Kept for free in case Tidal adds them later. See
// docs/tidal-genre-source-investigation.md (2026-04-30).
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
async fn get_home_releases(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    use crate::services::lastfm;

    // Pull api_key from the existing Last.fm credentials row. If Last.fm
    // isn't configured, we 503 so the frontend renders the connect/empty
    // state instead of falling back to the old AllMusic RSS feed.
    let (http, api_key) = {
        let s = state.read().await;
        let api_key =
            s.db.with_conn(|conn| Ok(lastfm::auth::load_credentials(conn).ok().flatten()))
                .ok()
                .flatten()
                .map(|c| c.api_key);
        (s.http_client.clone(), api_key)
    };
    let Some(api_key) = api_key.filter(|k| !k.is_empty()) else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };

    match lastfm::releases::fetch_new_releases_cached(&http, &api_key).await {
        Ok(releases) => Ok(Json(json!({
            "releases": releases,
            "source": "lastfm_api",
        }))),
        Err(e) => {
            tracing::warn!("Last.fm new-releases pipeline failed: {e}");
            Err(StatusCode::BAD_GATEWAY)
        }
    }
}

// ─── TIDAL: Your Mixes ───────────────────────────────────────────────────────

/// Returns the authenticated user's TIDAL mixes (Daily Discovery, My Mix N,
/// Master Mix, etc) for the home page Your Mixes shelf.
///
/// 503 when TIDAL is disconnected so the frontend can render its connect prompt.
async fn get_tidal_mixes(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    // Persisted-tokens fallback covers the cold-boot race: the home page
    // mounts before `tidal_status` has rehydrated `state.tidal_tokens` from
    // disk, so a direct in-memory check returns 503 even though the user is
    // connected. Other TIDAL endpoints follow this same pattern.
    let (tokens, http_client, tidal_http_client, mixes_cache) = {
        let in_memory = {
            let s = state.read().await;
            (
                s.tidal_tokens.clone(),
                s.http_client.clone(),
                s.tidal_http_client.clone(),
                s.tidal_mixes_cache.clone(),
            )
        };
        match in_memory.0 {
            Some(t) => (Some(t), in_memory.1, in_memory.2, in_memory.3),
            None => {
                let persisted = load_persisted_tidal_tokens(&state).await.ok().flatten();
                (persisted, in_memory.1, in_memory.2, in_memory.3)
            }
        }
    };
    let Some(tokens) = tokens else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };

    // 6h TTL cache. TIDAL refreshes mixes daily; revisiting Home shouldn't
    // round-trip TIDAL each time. Cache is cleared on app restart.
    {
        let guard = mixes_cache.lock().unwrap();
        if let Some((stored_at, cached)) = guard.as_ref()
            && stored_at.elapsed() < Duration::from_secs(6 * 60 * 60)
        {
            return Ok(Json(
                json!({ "mixes": cached, "source": "tidal", "cached": true }),
            ));
        }
    }
    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let mixes = match client.get_my_mixes().await {
        Ok(mixes) => mixes,
        Err(e) if error_looks_like_auth(&e) => {
            let refreshed = recover_tidal_session(&state, &http_client, &tokens)
                .await
                .map_err(|_| StatusCode::BAD_GATEWAY)?;
            let retry = TidalClient::with_http(
                tidal_http_client,
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            retry.get_my_mixes().await.map_err(|e| {
                tracing::warn!("TIDAL get_my_mixes failed after token refresh: {e}");
                StatusCode::BAD_GATEWAY
            })?
        }
        Err(e) => {
            tracing::warn!("TIDAL get_my_mixes failed: {e}");
            return Err(StatusCode::BAD_GATEWAY);
        }
    };
    {
        let mut guard = mixes_cache.lock().unwrap();
        *guard = Some((Instant::now(), mixes.clone()));
    }
    Ok(Json(json!({ "mixes": mixes, "source": "tidal" })))
}

// ─── TIDAL: Personal Radio Stations ──────────────────────────────────────────

/// Returns the user's personal TIDAL radio stations for the home shelf.
/// Same pattern as `get_tidal_mixes` — 503 when disconnected, 6h TTL cache.
async fn get_tidal_radio_stations(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let (tokens, http_client, tidal_http_client, radio_cache) = {
        let in_memory = {
            let s = state.read().await;
            (
                s.tidal_tokens.clone(),
                s.http_client.clone(),
                s.tidal_http_client.clone(),
                s.tidal_radio_stations_cache.clone(),
            )
        };
        match in_memory.0 {
            Some(t) => (Some(t), in_memory.1, in_memory.2, in_memory.3),
            None => {
                let persisted = load_persisted_tidal_tokens(&state).await.ok().flatten();
                (persisted, in_memory.1, in_memory.2, in_memory.3)
            }
        }
    };
    let Some(tokens) = tokens else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };

    {
        let guard = radio_cache.lock().unwrap();
        if let Some((stored_at, cached)) = guard.as_ref()
            && stored_at.elapsed() < Duration::from_secs(6 * 60 * 60)
        {
            return Ok(Json(
                json!({ "stations": cached, "source": "tidal", "cached": true }),
            ));
        }
    }

    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let stations = match client.get_my_radio_stations().await {
        Ok(s) => s,
        Err(e) if error_looks_like_auth(&e) => {
            let refreshed = recover_tidal_session(&state, &http_client, &tokens)
                .await
                .map_err(|_| StatusCode::BAD_GATEWAY)?;
            let retry = TidalClient::with_http(
                tidal_http_client,
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            retry.get_my_radio_stations().await.map_err(|e| {
                tracing::warn!("TIDAL get_my_radio_stations failed after token refresh: {e}");
                StatusCode::BAD_GATEWAY
            })?
        }
        Err(e) => {
            tracing::warn!("TIDAL get_my_radio_stations failed: {e}");
            return Err(StatusCode::BAD_GATEWAY);
        }
    };
    {
        let mut guard = radio_cache.lock().unwrap();
        *guard = Some((Instant::now(), stations.clone()));
    }
    Ok(Json(json!({ "stations": stations, "source": "tidal" })))
}

// ─── TIDAL: Home discover modules ────────────────────────────────────────────

/// Returns the editorial modules from `pages/home` (what the TIDAL web client
/// renders as the "discover" surface — The Hits, New Tracks, New Albums,
/// Spotlighted Uploads, From our editors). 503 when TIDAL is disconnected so
/// the frontend can render its connect prompt instead of an error toast.
async fn get_tidal_home_modules(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let (tokens, http_client, tidal_http_client) = {
        let in_memory = {
            let s = state.read().await;
            (
                s.tidal_tokens.clone(),
                s.http_client.clone(),
                s.tidal_http_client.clone(),
            )
        };
        match in_memory.0 {
            Some(t) => (Some(t), in_memory.1, in_memory.2),
            None => {
                let persisted = load_persisted_tidal_tokens(&state).await.ok().flatten();
                (persisted, in_memory.1, in_memory.2)
            }
        }
    };
    let Some(tokens) = tokens else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };

    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let modules = match client.get_home_modules().await {
        Ok(m) => m,
        Err(e) if error_looks_like_auth(&e) => {
            let refreshed = recover_tidal_session(&state, &http_client, &tokens)
                .await
                .map_err(|_| StatusCode::BAD_GATEWAY)?;
            let retry = TidalClient::with_http(
                tidal_http_client,
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            retry.get_home_modules().await.map_err(|e| {
                tracing::warn!("TIDAL get_home_modules failed after token refresh: {e}");
                StatusCode::BAD_GATEWAY
            })?
        }
        Err(e) => {
            tracing::warn!("TIDAL get_home_modules failed: {e}");
            return Err(StatusCode::BAD_GATEWAY);
        }
    };
    Ok(Json(json!({ "modules": modules, "source": "tidal" })))
}

/// Returns the full item set for one home discover module, used by the
/// per-module "View all" detail route. The home preview only ships 5 items
/// for TRACK_LIST modules; this handler resolves the module id back to the
/// upstream `dataApiPath` and follows it to load the complete list.
async fn get_tidal_discover_module_items(
    State(state): State<SharedState>,
    Path(module_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let limit: u32 = params
        .get("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50)
        .min(200);

    let (tokens, http_client, tidal_http_client) = {
        let in_memory = {
            let s = state.read().await;
            (
                s.tidal_tokens.clone(),
                s.http_client.clone(),
                s.tidal_http_client.clone(),
            )
        };
        match in_memory.0 {
            Some(t) => (Some(t), in_memory.1, in_memory.2),
            None => {
                let persisted = load_persisted_tidal_tokens(&state).await.ok().flatten();
                (persisted, in_memory.1, in_memory.2)
            }
        }
    };
    let Some(tokens) = tokens else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };

    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );

    let modules = match client.get_home_modules().await {
        Ok(m) => m,
        Err(e) if error_looks_like_auth(&e) => {
            let refreshed = recover_tidal_session(&state, &http_client, &tokens)
                .await
                .map_err(|_| StatusCode::BAD_GATEWAY)?;
            let retry = TidalClient::with_http(
                tidal_http_client.clone(),
                refreshed.access_token.clone(),
                refreshed.country_code.clone(),
            );
            retry.get_home_modules().await.map_err(|e| {
                tracing::warn!("get_home_modules failed after refresh: {e}");
                StatusCode::BAD_GATEWAY
            })?
        }
        Err(e) => {
            tracing::warn!("get_home_modules failed: {e}");
            return Err(StatusCode::BAD_GATEWAY);
        }
    };

    let Some(module) = modules.into_iter().find(|m| m.id == module_id) else {
        return Err(StatusCode::NOT_FOUND);
    };
    let module_kind = module.kind.clone();
    let module_title = module.title.clone();
    // Modules without a `dataApiPath` (e.g. ALBUM_LIST already returning all
    // items inline) just echo back the preview items — that's the whole set.
    let items = if let Some(path) = module.more_path.as_deref() {
        let access_token = tokens.access_token.clone();
        let country_code = tokens.country_code.clone();
        let live = TidalClient::with_http(tidal_http_client, access_token, country_code);
        match live
            .get_module_items_via_path(path, &module_kind, limit)
            .await
        {
            Ok(items) if !items.is_empty() => items,
            _ => module.items, // fall back to the preview if the show-more call fails or returns 0
        }
    } else {
        module.items
    };

    Ok(Json(json!({
        "module": {
            "id": module_id,
            "title": module_title,
            "kind": module_kind,
            "items": items,
        },
        "source": "tidal",
    })))
}

/// Returns the playable tracks inside a TIDAL mix. Frontend calls this when
/// the user clicks a mix card on the home Your Mixes shelf, then queues +
/// plays the first track via the existing TIDAL playback path.
async fn get_tidal_mix_tracks(
    State(state): State<SharedState>,
    Path(mix_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let (tokens, tidal_http_client) = {
        let s = state.read().await;
        let tidal_http = s.tidal_http_client.clone();
        let in_memory = s.tidal_tokens.clone();
        drop(s);
        match in_memory {
            Some(t) => (Some(t), tidal_http),
            None => {
                let persisted = load_persisted_tidal_tokens(&state).await.ok().flatten();
                (persisted, tidal_http)
            }
        }
    };
    let Some(tokens) = tokens else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };
    let client = TidalClient::with_http(
        tidal_http_client,
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let items = client.get_mix_tracks(&mix_id).await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": e.to_string() })),
        )
    })?;

    // Reuse the same shape `getTidalAlbumTracks` returns so the frontend's
    // `playTidalTrackNow` consumer can reuse its existing track-mapping.
    let tracks: Vec<Value> = items
        .into_iter()
        .map(|t| {
            let artwork = t
                .album
                .as_ref()
                .and_then(|al| al.cover.as_ref())
                .and_then(|c| {
                    crate::services::tidal::client::TidalClient::get_artwork_url(
                        &Some(c.clone()),
                        640,
                    )
                });
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
            })
        })
        .collect();

    Ok(Json(json!({ "tracks": tracks })))
}

// ─── Last.fm scrobble auth (server-side web-auth flow) ──────────────────────

/// Reasoning lives in `services/lastfm/scrobble.rs` and the plan file. Short
/// version: the user goes Settings → "Connect Last.fm account" → we open
/// `https://www.last.fm/api/auth/?api_key=...&token=...` in a new tab → user
/// clicks "Yes, allow access" → returns to NOORwave → "I've authorized" button
/// fires /complete → we redeem the token for a session_key (encrypted on disk).

async fn lastfm_auth_start(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    use crate::services::lastfm;

    let (http, api_secret, api_key) = {
        let s = state.read().await;
        let api_key =
            s.db.with_conn(|conn| Ok(lastfm::auth::load_credentials(conn).ok().flatten()))
                .ok()
                .flatten()
                .map(|c| c.api_key);
        (s.http_client.clone(), s.lastfm_api_secret.clone(), api_key)
    };
    let Some(api_secret) = api_secret else {
        return Err(StatusCode::NOT_IMPLEMENTED);
    };
    let Some(api_key) = api_key.filter(|k| !k.is_empty()) else {
        return Ok(Json(json!({
            "status": "error",
            "message": "Save a Last.fm API key first."
        })));
    };

    let token = match lastfm::scrobble::get_token(&http, &api_key, &api_secret).await {
        Ok(t) => t,
        Err(e) => {
            return Ok(Json(json!({
                "status": "error",
                "message": format!("auth.getToken failed: {e}")
            })));
        }
    };

    // Stash the pending token server-side so /complete doesn't have to trust
    // the client to round-trip it.
    let stash_result = state.read().await.db.with_conn(|conn| {
        lastfm::auth::set_pending_token(conn, &token)?;
        Ok(())
    });
    if let Err(e) = stash_result {
        tracing::warn!("Failed to stash Last.fm pending_token: {e}");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    let auth_url = format!(
        "https://www.last.fm/api/auth/?api_key={}&token={}",
        urlencoding::encode(&api_key),
        urlencoding::encode(&token)
    );
    Ok(Json(json!({
        "status": "awaiting",
        "auth_url": auth_url,
    })))
}

async fn lastfm_auth_complete(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    use crate::services::lastfm;

    let (http, api_secret, api_key, pending_token, master_key) = {
        let s = state.read().await;
        let creds =
            s.db.with_conn(|conn| Ok(lastfm::auth::load_credentials(conn).ok().flatten()))
                .ok()
                .flatten();
        (
            s.http_client.clone(),
            s.lastfm_api_secret.clone(),
            creds.as_ref().map(|c| c.api_key.clone()),
            creds.and_then(|c| c.pending_token),
            s.master_key.clone(),
        )
    };
    let Some(api_secret) = api_secret else {
        return Err(StatusCode::NOT_IMPLEMENTED);
    };
    let Some(api_key) = api_key.filter(|k| !k.is_empty()) else {
        return Ok(Json(json!({
            "status": "error",
            "message": "Last.fm API key not configured."
        })));
    };
    let Some(token) = pending_token else {
        return Ok(Json(json!({
            "status": "error",
            "message": "No pending auth — call /api/lastfm/auth/start first."
        })));
    };

    let session = match lastfm::scrobble::get_session(&http, &api_key, &api_secret, &token).await {
        Ok(s) => s,
        Err(e) => {
            // Don't drop the pending_token on a "not yet authorized" error —
            // the user might just need a few more seconds in the browser.
            // The user can retry by clicking the button again.
            return Ok(Json(json!({
                "status": "not_yet_authorized",
                "message": format!("auth.getSession failed: {e}")
            })));
        }
    };

    let persist_result = state.read().await.db.with_conn(|conn| {
        lastfm::auth::save_session_key(
            conn,
            &master_key,
            &session.session_key,
            &session.user_name,
        )?;
        Ok(())
    });
    if let Err(e) = persist_result {
        tracing::warn!("Failed to persist Last.fm session: {e}");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    Ok(Json(json!({
        "status": "connected",
        "user": session.user_name,
    })))
}

async fn lastfm_auth_disconnect(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::lastfm;
    let _ = state.read().await.db.with_conn(|conn| {
        lastfm::auth::clear_session(conn)?;
        Ok(())
    });
    Ok(Json(json!({"status": "disconnected"})))
}

/// Get daily picks curated from user's library using learning model
async fn get_home_picks(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let state_guard = state.read().await;
    let db = &state_guard.db;

    // Get top tracks from listening history with variety
    let picks = db
        .with_conn(|conn| {
            // Fetch recent top tracks that aren't played in last 7 days (rediscovery)
            let tracks = queries::get_tracks(conn, "play_count", "desc", 20, 0, false, false)?;

            // Get tracks from different genres for variety
            let mut genre_tracks = conn.prepare(
                "SELECT t.*, g.name as genre_name
             FROM tracks t
             JOIN track_genres tg ON t.id = tg.track_id
             JOIN genres g ON tg.genre_id = g.id
             WHERE t.play_count > 0
             ORDER BY RANDOM()
             LIMIT 10",
            )?;

            let genre_picks: Vec<serde_json::Value> = genre_tracks
                .query_map([], |row| {
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
                })?
                .filter_map(|r| r.ok())
                .collect();

            Ok((tracks, genre_picks))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

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
async fn get_home_articles(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let aggregator = state.read().await.rss_aggregator.clone();
    let articles = aggregator.get_articles().await;

    Ok(Json(json!({
        "articles": articles,
        "source": "allmusic_rss"
    })))
}

/// Get music industry news from multiple RSS sources
async fn get_home_news(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
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

async fn start_spotify_enrichment(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
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
        )
        .await;

        running.store(false, Ordering::SeqCst);
        if result.is_ok() {
            let _ = event_tx.send(AppEvent::MusicBrainzEnriched);
        }
    });

    Ok(Json(json!({"status": "started", "total": total})))
}

async fn get_spotify_enrichment_status(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use std::sync::atomic::Ordering;
    let s = state.read().await;
    let enriched: i64 =
        s.db.with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(DISTINCT track_id) FROM track_genres WHERE source = 'spotify'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap_or(0);
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

// Manual cleanup: delete `tidal_stream` track rows that have no remaining
// references (no listen history, not favorited, not in any queue/playlist/etc).
// Safe to run any time. CASCADE FKs (track_neighbors, embeddings, transitions,
// audio_dsp_features, etc.) take care of trained-data cleanup automatically.
async fn purge_orphan_tidal_stream_tracks(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    let result: anyhow::Result<usize> = s.db.with_conn(|conn| {
        // Filter against every non-CASCADE FK referencing tracks(id) so the
        // DELETE doesn't fail with a constraint violation.
        let deleted = conn.execute(
            "DELETE FROM tracks
             WHERE source = 'tidal_stream'
               AND is_favorite = 0
               AND id NOT IN (SELECT track_id FROM listen_history WHERE track_id IS NOT NULL)
               AND id NOT IN (SELECT track_id FROM queue WHERE track_id IS NOT NULL)
               AND id NOT IN (SELECT track_id FROM playlist_tracks)
               AND id NOT IN (SELECT current_track_id FROM playback_state WHERE current_track_id IS NOT NULL)
               AND id NOT IN (SELECT track_id FROM shuffle_state)
               AND id NOT IN (SELECT track_id FROM duplicate_group_members)
               AND id NOT IN (SELECT track_id FROM acrcloud_results)",
            [],
        )?;
        Ok(deleted)
    });
    match result {
        Ok(deleted) => {
            tracing::info!(deleted, "purge_orphan_tidal_stream_tracks");
            Ok(Json(json!({ "status": "ok", "deleted": deleted })))
        }
        Err(e) => Ok(Json(json!({
            "status": "error",
            "message": format!("Purge failed: {}", e),
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
            let creds = lastfm::auth::LastFmCredentials {
                api_key,
                ..Default::default()
            };
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
    let (creds, has_secret) = {
        let s = state.read().await;
        let creds =
            s.db.with_conn(|conn| Ok(lastfm::auth::load_credentials(conn).ok().flatten()))
                .ok()
                .flatten();
        (creds, s.lastfm_api_secret.is_some())
    };
    let enrichment = creds
        .as_ref()
        .map(|c| !c.api_key.is_empty())
        .unwrap_or(false);
    let user = creds.as_ref().and_then(|c| c.session_user.clone());
    let scrobbling = enrichment && has_secret && user.is_some();
    Ok(Json(json!({
        // Legacy field kept for backward compat with any existing caller of
        // /api/lastfm/status — equivalent to `enrichment`.
        "configured": enrichment,
        "enrichment": enrichment,
        "scrobbling": scrobbling,
        "scrobble_available": has_secret,
        "user": user,
    })))
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

    let (
        http,
        event_tx,
        running,
        cancel,
        total_atom,
        processed_atom,
        prefetch_total_atom,
        prefetch_done_atom,
        started_at_atom,
    ) = {
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
    let enriched: i64 =
        s.db.with_conn(|conn| {
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
        (
            s.analysis_tx.clone(),
            s.audio_analysis_cancel.clone(),
            s.audio_analysis_running.clone(),
        )
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
    s.audio_analysis_cancel
        .store(true, std::sync::atomic::Ordering::Relaxed);
    s.audio_analysis_running
        .store(false, std::sync::atomic::Ordering::Relaxed);
    Ok(Json(json!({ "status": "stopped" })))
}

async fn get_audio_analysis_status(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    let analyzed =
        s.db.with_conn(queries::count_audio_dsp_features)
            .unwrap_or(0);
    Ok(Json(json!({
        "running": s.audio_analysis_running.load(std::sync::atomic::Ordering::Relaxed),
        "analyzed": analyzed,
    })))
}

async fn get_passive_dsp(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    let enabled =
        s.db.with_conn(|conn| Ok(crate::services::audio_analysis::is_passive_enabled(conn)))
            .unwrap_or(true);
    Ok(Json(json!({ "enabled": enabled })))
}

#[derive(Deserialize)]
struct PassiveDspBody {
    enabled: bool,
}

async fn set_passive_dsp(
    State(state): State<SharedState>,
    Json(payload): Json<PassiveDspBody>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    s.db.with_conn(|conn| {
        crate::services::audio_analysis::set_passive_enabled(conn, payload.enabled)
            .map_err(anyhow::Error::from)
    })
    .map_err(|e| {
        tracing::error!("failed to persist passive_dsp_enabled: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(json!({ "enabled": payload.enabled })))
}

async fn get_track_audio_features(
    State(state): State<SharedState>,
    Path(track_id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    let features =
        s.db.with_conn(|conn| queries::get_audio_dsp_features(conn, track_id))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "features": features })))
}

async fn get_audio_features_stats(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    let stats =
        s.db.with_conn(queries::get_audio_features_stats)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "stats": stats })))
}

async fn get_library_analytics(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    let summary =
        s.db.with_conn(|conn| {
            let tracks = queries::get_all_tracks(conn)?;
            let playlists = queries::get_playlists(conn)?;
            let genre_paths = queries::get_track_genre_paths_with_fallback(conn)?;
            let mut context = crate::smart::analytics::AnalyticsContext::new();
            for (track_id, rows) in genre_paths {
                context =
                    context.with_track_genres(track_id, queries::ResolvedGenre::paths_only(&rows));
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
    s.db.with_conn(queries::delete_all_audio_dsp_features)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "status": "reset" })))
}

/// GET /api/library/audio-features/quality — coverage / confidence breakdown.
async fn get_audio_features_quality(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let s = state.read().await;
    let q =
        s.db.with_conn(queries::get_audio_features_quality)
            .map_err(|e| {
                tracing::error!("audio-features/quality query failed: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    Ok(Json(json!({
        "total_tracks": q.total_tracks,
        "analyzed": q.analyzed,
        "analysis_current": q.analysis_current,
        "analysis_stale": q.analysis_stale,
        "low_confidence_bpm": q.low_confidence_bpm,
        "low_confidence_key": q.low_confidence_key,
        "no_preview_url": q.no_preview_url,
        "fingerprinted": q.fingerprinted,
    })))
}

/// GET /api/library/analyze/reanalyze-stale — re-queue every track whose
/// stored `analysis_version` is not the current `CURRENT_ANALYSIS_VERSION`
/// (see `crate::services::audio_analysis::CURRENT_ANALYSIS_VERSION`). If the
/// analysis actor isn't wired we still return the count of stale tracks so the
/// caller can decide what to do next.
async fn reanalyze_stale_tracks(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let (db, analysis_tx) = {
        let s = state.read().await;
        (s.db.clone(), s.analysis_tx.clone())
    };

    let stale_ids = db
        .with_conn(queries::get_stale_analysis_track_ids)
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
            // CURRENT_ANALYSIS_VERSION is a compile-time constant — safe to interpolate.
            conn.execute(
                &format!(
                    "DELETE FROM audio_dsp_features WHERE analysis_version != '{}'",
                    crate::services::audio_analysis::CURRENT_ANALYSIS_VERSION,
                ),
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
    /// Optional country (Last.fm only). Accepts either an ISO 3166-1 alpha-2
    /// code (e.g. "AU") which is mapped via `CURATED_COUNTRIES`, or the full
    /// English name (e.g. "United States") for legacy/free-form callers.
    country: Option<String>,
    /// Optional curated genre key (Last.fm only), e.g. "hip-hop".
    /// Mutually exclusive with `country`.
    tag: Option<String>,
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
    /// Top genre name for the resolved local track (None for Tidal-only entries
    /// where we have no genre data without an extra API call).
    genre: Option<String>,
}

/// Fill in missing artwork for chart entries by searching Tidal for the
/// (artist, title) pair. Updates `image_url` (top-level fallback) and the
/// nested `tidal_playable.artwork_url` so the frontend's preference chain
/// always lands on something usable.
async fn enrich_chart_artwork(state: &SharedState, entries: &mut Vec<ChartEntryDto>) {
    use futures::stream::{FuturesUnordered, StreamExt};

    /// Last.fm's blank-star fallback. We never want to surface this — both the
    /// usable-art check below and the replace-on-enrich path treat it as empty.
    const LASTFM_PLACEHOLDER: &str = "2a96cbd8b46e442fc41c2b86b821562f";

    fn is_unusable(url: Option<&str>) -> bool {
        let Some(url) = url else { return true };
        let trimmed = url.trim();
        trimmed.is_empty() || trimmed.contains(LASTFM_PLACEHOLDER)
    }

    fn has_usable_art(e: &ChartEntryDto) -> bool {
        let local_ok = !is_unusable(
            e.local_track
                .as_ref()
                .and_then(|t| t.artwork_url.as_deref()),
        );
        let tp_ok = !is_unusable(
            e.tidal_playable
                .as_ref()
                .and_then(|tp| tp.artwork_url.as_deref()),
        );
        let img_ok = !is_unusable(e.image_url.as_deref());
        local_ok || tp_ok || img_ok
    }

    let needs: Vec<(usize, String, String)> = entries
        .iter()
        .enumerate()
        .filter_map(|(idx, e)| {
            if has_usable_art(e) {
                return None;
            }
            let title = e
                .local_track
                .as_ref()
                .map(|t| t.title.clone())
                .or_else(|| e.tidal_playable.as_ref().map(|tp| tp.title.clone()))?;
            let artist = e
                .local_track
                .as_ref()
                .and_then(|t| t.artist_name.clone())
                .or_else(|| {
                    e.tidal_playable
                        .as_ref()
                        .and_then(|tp| tp.artist_name.clone())
                })?;
            Some((idx, artist, title))
        })
        .collect();

    if needs.is_empty() {
        return;
    }

    let (tokens, tidal_http_client) = {
        let s = state.read().await;
        (s.tidal_tokens.clone(), s.tidal_http_client.clone())
    };
    let tokens = match tokens {
        Some(t) => Some(t),
        None => load_persisted_tidal_tokens(state).await.ok().flatten(),
    };
    let Some(tokens) = tokens else {
        return;
    };

    let client = TidalClient::with_http(
        tidal_http_client,
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );

    let mut tasks = FuturesUnordered::new();
    for (idx, artist, title) in needs {
        let client = client.clone();
        tasks.push(async move {
            let q = format!("{artist} {title}");
            let result = client.search(&q, 1).await.ok();
            let url = result
                .and_then(|r| r.into_iter().next())
                .and_then(|t| t.artwork_url);
            (idx, url)
        });
    }

    while let Some((idx, url)) = tasks.next().await {
        let Some(url) = url else { continue };
        if let Some(entry) = entries.get_mut(idx) {
            if is_unusable(entry.image_url.as_deref()) {
                entry.image_url = Some(url.clone());
            }
            if let Some(tp) = entry.tidal_playable.as_mut()
                && is_unusable(tp.artwork_url.as_deref())
            {
                tp.artwork_url = Some(url);
            }
        }
    }
}

/// Look up the most-confident genre name for each track id in a single query.
/// Returns a map keyed by track_id; tracks with no genre rows are absent.
fn fetch_top_genres_for_tracks(
    db: &crate::db::Database,
    track_ids: &[i64],
) -> HashMap<i64, String> {
    if track_ids.is_empty() {
        return HashMap::new();
    }
    db.with_conn(|conn| {
        let placeholders = std::iter::repeat_n("?", track_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        // Pick the highest-confidence genre per track. Ties broken by
        // alphabetical order so the result is stable.
        let sql = format!(
            "SELECT track_id, name FROM (
                SELECT tg.track_id, g.name,
                       ROW_NUMBER() OVER (
                           PARTITION BY tg.track_id
                           ORDER BY tg.confidence DESC, g.name ASC
                       ) AS rn
                FROM track_genres tg
                JOIN genres g ON g.id = tg.genre_id
                WHERE tg.track_id IN ({placeholders})
             ) WHERE rn = 1"
        );
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = track_ids
            .iter()
            .map(|id| id as &dyn rusqlite::ToSql)
            .collect();
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (id, name) = row?;
            map.insert(id, name);
        }
        Ok(map)
    })
    .unwrap_or_default()
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
/// 2-hour TTL matches the frontend's in-memory cache so the trending shelf
/// stays static across page navigations within the window.
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
        cache.insert(
            key,
            ChartCacheEntry {
                inserted_at: Instant::now(),
                payload,
            },
        );
    }
}

async fn get_charts(
    State(state): State<SharedState>,
    Query(params): Query<ChartParams>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use crate::services::charts::curated;

    let source = params
        .source
        .as_deref()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "lastfm".to_string());
    let limit = params.limit.unwrap_or(50).clamp(1, 100);

    let country_input = params
        .country
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let tag_input = params
        .tag
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    if country_input.is_some() && tag_input.is_some() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "tag_country_exclusive" })),
        ));
    }

    // Resolve country: ISO code or full name → curated entry. Two-char inputs
    // that aren't curated codes are rejected; longer strings that don't match
    // a curated `lastfm_name` pass through as free-form (legacy callers).
    // The cache token is always the ISO code when curated, so `?country=AU`
    // and `?country=Australia` collapse to one cache entry.
    let (country_resolved, country_cache_token): (Option<String>, Option<String>) =
        match country_input {
            None => (None, None),
            Some(s) if s.len() == 2 => match curated::find_country_by_code(s) {
                Some(entry) => (
                    Some(entry.lastfm_name.to_string()),
                    Some(entry.code.to_string()),
                ),
                None => {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        Json(json!({ "error": "unknown_country" })),
                    ));
                }
            },
            Some(s) => match curated::find_country_by_code_or_name(s) {
                Some(entry) => (
                    Some(entry.lastfm_name.to_string()),
                    Some(entry.code.to_string()),
                ),
                None => (Some(s.to_string()), Some(s.to_ascii_uppercase())),
            },
        };

    let tag_resolved: Option<&'static curated::GenreEntry> = match tag_input {
        Some(key) => match curated::find_genre(key) {
            Some(entry) => Some(entry),
            None => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": "unknown_genre" })),
                ));
            }
        },
        None => None,
    };

    let cache_key = format!(
        "{}|{}|{}|{}",
        source,
        limit,
        country_cache_token.as_deref().unwrap_or(""),
        tag_resolved.map(|g| g.key).unwrap_or("")
    );
    if let Some(cached) = chart_cache_get(&cache_key) {
        return Ok(Json(cached));
    }

    let mut entries: Vec<ChartEntryDto> = match source.as_str() {
        "tidal" => fetch_tidal_chart(&state, limit as i32).await.map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("tidal chart: {e}") })),
            )
        })?,
        _ => fetch_lastfm_chart(&state, limit, country_resolved.as_deref(), tag_resolved)
            .await
            .map_err(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({ "error": format!("lastfm chart: {e}") })),
                )
            })?,
    };

    // Backfill missing artwork via Tidal search. Last.fm's chart.getTopTracks
    // mostly returns a generic placeholder image, and many older library albums
    // have NULL artwork_url, so this is the difference between blank tiles and
    // real covers on the trending shelf.
    enrich_chart_artwork(&state, &mut entries).await;

    let payload = json!({
        "source": source,
        "limit": limit,
        "country": country_cache_token,
        "tag": tag_resolved.map(|g| g.key),
        "tracks": entries,
    });
    chart_cache_put(cache_key, payload.clone());
    Ok(Json(payload))
}

async fn list_lastfm_genres() -> Json<Value> {
    use crate::services::charts::curated::{CURATED_GENRES, DEFAULT_GENRE_KEY};
    let genres: Vec<_> = CURATED_GENRES
        .iter()
        .map(|g| json!({ "key": g.key, "label": g.label }))
        .collect();
    Json(json!({ "genres": genres, "default_genre": DEFAULT_GENRE_KEY }))
}

async fn list_lastfm_countries() -> Json<Value> {
    use crate::services::charts::curated::{CURATED_COUNTRIES, DEFAULT_COUNTRY_CODE};
    let countries: Vec<_> = CURATED_COUNTRIES
        .iter()
        .map(|c| json!({ "code": c.code, "label": c.label }))
        .collect();
    Json(json!({ "countries": countries, "default_country": DEFAULT_COUNTRY_CODE }))
}

async fn fetch_lastfm_chart(
    state: &SharedState,
    limit: u32,
    country: Option<&str>,
    genre: Option<&'static crate::services::charts::curated::GenreEntry>,
) -> anyhow::Result<Vec<ChartEntryDto>> {
    let (http, db) = {
        let s = state.read().await;
        (s.http_client.clone(), s.db.clone())
    };
    let client = LastFmClient::load(http, &db)
        .ok_or_else(|| anyhow::anyhow!("Last.fm API key not configured"))?;

    let tracks = if let Some(genre) = genre {
        // Genre tag — fan out to every Last.fm tag the curated entry maps to,
        // merge, dedupe by normalised (artist, title), sum playcounts on dupes,
        // sort desc by playcount, truncate. Single-tag entries skip the merge.
        if genre.lastfm_tags.len() == 1 {
            client
                .get_top_tracks_by_tag(genre.lastfm_tags[0], limit)
                .await?
        } else {
            use futures::future::join_all;
            // Overfetch per leg so dedup across overlapping tags doesn't shrink
            // the merged list below the requested `limit`. Capped at Last.fm's
            // per-call ceiling.
            let fan_limit = limit.saturating_mul(2).min(100);
            let calls = genre.lastfm_tags.iter().map(|tag| {
                let c = client.clone();
                let t = (*tag).to_string();
                async move { c.get_top_tracks_by_tag(&t, fan_limit).await }
            });
            let results = join_all(calls).await;
            let mut merged: Vec<crate::metadata::lastfm::LastFmChartTrack> = Vec::new();
            let mut by_key: HashMap<String, usize> = HashMap::new();
            for res in results {
                let list = match res {
                    Ok(l) => l,
                    Err(e) => {
                        tracing::warn!("tag fan-out leg failed: {}", e);
                        continue;
                    }
                };
                for t in list {
                    let key = crate::services::radio::normalize_for_dedup(&t.artist, &t.title);
                    if key.is_empty() {
                        continue;
                    }
                    if let Some(&idx) = by_key.get(&key) {
                        // Merge: sum playcounts/listeners; prefer the first non-empty
                        // image and mbid (already populated on the existing entry).
                        let existing: &mut crate::metadata::lastfm::LastFmChartTrack =
                            &mut merged[idx];
                        existing.playcount = match (existing.playcount, t.playcount) {
                            (Some(a), Some(b)) => Some(a.saturating_add(b)),
                            (a, b) => a.or(b),
                        };
                        existing.listeners = match (existing.listeners, t.listeners) {
                            (Some(a), Some(b)) => Some(a.saturating_add(b)),
                            (a, b) => a.or(b),
                        };
                        if existing.image_url.as_deref().unwrap_or("").is_empty() {
                            existing.image_url = t.image_url;
                        }
                        if existing.mbid.is_none() {
                            existing.mbid = t.mbid;
                        }
                    } else {
                        by_key.insert(key, merged.len());
                        merged.push(t);
                    }
                }
            }
            merged.sort_by(|a, b| b.playcount.unwrap_or(0).cmp(&a.playcount.unwrap_or(0)));
            merged.truncate(limit as usize);
            merged
        }
    } else {
        client.get_top_chart(limit, country).await?
    };

    // Resolve each (artist, title) to a local library track when present.
    // We do this in a single DB call by collecting all (artist, title) pairs
    // and matching case-insensitively.
    let pairs: Vec<(String, String)> = tracks
        .iter()
        .map(|t| (t.artist.clone(), t.title.clone()))
        .collect();
    let local_map = resolve_chart_pairs_to_local(&db, &pairs).unwrap_or_default();
    let local_ids: Vec<i64> = local_map.values().map(|t| t.id).collect();
    let genre_map = fetch_top_genres_for_tracks(&db, &local_ids);

    let mut out = Vec::with_capacity(tracks.len());
    for t in tracks {
        let key = format!(
            "{}\u{0001}{}",
            t.artist.to_ascii_lowercase(),
            t.title.to_ascii_lowercase()
        );
        let local_track = local_map.get(&key).cloned();
        let genre = local_track
            .as_ref()
            .and_then(|lt| genre_map.get(&lt.id).cloned());
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
            genre,
        });
    }
    Ok(out)
}

async fn fetch_tidal_chart(state: &SharedState, limit: i32) -> anyhow::Result<Vec<ChartEntryDto>> {
    let (tokens_opt, http, db, tidal_http_client) = {
        let s = state.read().await;
        (
            s.tidal_tokens.clone(),
            s.http_client.clone(),
            s.db.clone(),
            s.tidal_http_client.clone(),
        )
    };
    let persisted = load_persisted_tidal_tokens(state).await?;
    let tokens = tokens_opt.or(persisted);
    let Some(tokens) = tokens else {
        // Tidal not connected; degrade to empty list rather than failing.
        tracing::warn!("Tidal chart requested but Tidal not connected");
        return Ok(Vec::new());
    };
    let client = TidalClient::with_http(
        tidal_http_client,
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
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

    let resolved_local_ids: Vec<i64> = local_tracks.values().map(|t| t.id).collect();
    let genre_map = fetch_top_genres_for_tracks(&db, &resolved_local_ids);

    let mut out = Vec::with_capacity(tracks.len());
    for t in tracks {
        let local_track = known
            .get(&t.id)
            .and_then(|lid| local_tracks.get(lid))
            .cloned();
        let genre = local_track
            .as_ref()
            .and_then(|lt| genre_map.get(&lt.id).cloned());
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
            genre,
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

        let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(db))));

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

    /// Build a fresh `AppState` backed by `db`. Single source of truth for test
    /// initializers — when `crate::AppState` gains a field, add it here once.
    fn fresh_test_state(db: Database) -> crate::AppState {
        let (event_tx, _) = tokio::sync::broadcast::channel(16);
        crate::AppState {
            db,
            event_tx,
            http_client: reqwest::Client::new(),
            tidal_http_client: reqwest::Client::new(),
            tidal_tokens: None,
            tidal_mixes_cache: Arc::new(std::sync::Mutex::new(None)),
            tidal_radio_stations_cache: Arc::new(std::sync::Mutex::new(None)),
            spotify_tokens: None,
            playback_runtime: None,
            playback_runtime_info: None,
            playback_generation: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            current_stream_display: None,
            pending_stream_display: None,
            active_listen_session: None,
            live_listen_session: None,
            external_playback_track: None,
            ephemeral_tidal_track: None,
            tidal_login_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            rss_aggregator: Arc::new(crate::services::rss_feeds::FeedAggregator::new(
                reqwest::Client::new(),
            )),
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
            musicbrainz_enrich_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            tidal_sync_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            tidal_sync_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            lastfm_enrich_total: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            lastfm_enrich_processed: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            lastfm_prefetch_total: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            lastfm_prefetch_done: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            lastfm_enrich_started_at: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            discovery_train_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            refreshed_seeds: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            embedding_cache: Arc::new(tokio::sync::Mutex::new(None)),
            master_key: crate::services::crypto::MasterKey::load_or_generate(
                &std::env::temp_dir().join(format!("noor-test-key-{}", uuid::Uuid::new_v4())),
            )
            .expect("test master key"),
            pending_tidal_mix_queue: Arc::new(std::sync::Mutex::new(
                std::collections::VecDeque::new(),
            )),
            lastfm_api_secret: None,
            server_token: String::new(),
            audio_active: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            user_cleared_at: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            spotify_public_stats_enabled: false,
            sportify_client: None,
            sportify_cache_config: crate::services::sportify::cache::SportifyCacheConfig::default(),
            sportify_resolve_config:
                crate::services::sportify::cache::SportifyResolveConfig::default(),
        }
    }

    /// Build a minimal test app backed by a fresh in-memory database.
    async fn build_test_app() -> Router {
        let db_path = std::env::temp_dir().join(format!("noor-test-{}.db", uuid::Uuid::new_v4()));
        let db = Database::open(&db_path).expect("db opened");
        db.run_migrations().expect("migrations");
        db.with_conn(|conn| schema::run_migrations(conn))
            .expect("schema migrations");
        api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(db))))
    }

    fn fresh_migrated_db() -> (Database, std::path::PathBuf) {
        let db_path = std::env::temp_dir().join(format!("noor-test-{}.db", uuid::Uuid::new_v4()));
        let db = Database::open(&db_path).expect("db opened");
        db.run_migrations().expect("migrations");
        db.with_conn(|conn| schema::run_migrations(conn))
            .expect("schema migrations");
        (db, db_path)
    }

    fn seed_basic_tracks(db: &Database) {
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (1, 'Seed Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (
                    id, title, artist_id, duration_ms, source, fidelity_score
                 ) VALUES
                    (1, 'First Track', 1, 180000, 'tidal_stream', 0),
                    (2, 'Second Track', 1, 180000, 'tidal_stream', 0)",
                [],
            )?;
            Ok(())
        })
        .expect("seed tracks");
    }

    #[tokio::test]
    async fn clear_queue_returns_snapshot_and_preserves_current() {
        let (db, db_path) = fresh_migrated_db();
        seed_basic_tracks(&db);
        let current_qid: i64 = db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO tracks (
                        id, title, artist_id, duration_ms, source, fidelity_score
                     ) VALUES (3, 'Third Track', 1, 180000, 'tidal_stream', 0)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
                    [],
                )?;
                let qid = conn.last_insert_rowid();
                conn.execute(
                    "INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'user')",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO queue (track_id, position, source) VALUES (3, 2, 'user')",
                    [],
                )?;
                conn.execute(
                    "UPDATE playback_state
                     SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
                     WHERE id = 1",
                    rusqlite::params![qid],
                )?;
                Ok(qid)
            })
            .unwrap();

        let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
            db.clone(),
        ))));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/playback/queue/clear")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();

        let queue = body["queue"].as_array().expect("queue array");
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0]["id"], current_qid);
        assert_eq!(queue[0]["track"]["id"], 1);
        assert_eq!(body["playback_state"]["current_track"]["id"], 1);
        assert_eq!(body["playback_state"]["current_queue_item_id"], current_qid);

        let persisted_queue_count: i64 = db
            .with_conn(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM queue", [], |row| row.get(0))?)
            })
            .unwrap();
        assert_eq!(persisted_queue_count, 1);

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn queue_append_library_track_returns_updated_queue() {
        let (db, db_path) = fresh_migrated_db();
        seed_basic_tracks(&db);
        let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
            db.clone(),
        ))));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/queue/append")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"kind":"library","track_id":1,"artist":"Seed Artist","title":"First Track"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["queue"].as_array().unwrap().len(), 1);
        assert_eq!(body["queue"][0]["track"]["id"], 1);
        assert_eq!(body["queue"][0]["is_pending"], false);

        let source: String = db
            .with_conn(|conn| {
                Ok(
                    conn.query_row("SELECT source FROM queue WHERE track_id = 1", [], |row| {
                        row.get(0)
                    })?,
                )
            })
            .unwrap();
        assert_eq!(source, "user_queue");

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn queue_play_next_tidal_inserts_pending_row_after_current_with_hint() {
        let (db, db_path) = fresh_migrated_db();
        seed_basic_tracks(&db);
        let current_qid: i64 = db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
                    [],
                )?;
                let qid = conn.last_insert_rowid();
                conn.execute(
                    "INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'user')",
                    [],
                )?;
                conn.execute(
                    "UPDATE playback_state
                     SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
                     WHERE id = 1",
                    rusqlite::params![qid],
                )?;
                Ok(qid)
            })
            .unwrap();
        assert!(current_qid > 0);

        let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
            db.clone(),
        ))));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/queue/play_next")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"kind":"tidal","tidal_id":777,"artist":"External Artist","title":"External Title"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["queue"].as_array().unwrap().len(), 3);
        assert_eq!(body["queue"][1]["is_pending"], true);
        assert_eq!(body["queue"][1]["track"]["title"], "External Title");

        let pending: (i32, String, String, String, Option<i64>) = db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT position, source, pending_artist, pending_title, tidal_id_hint
                     FROM queue WHERE track_id IS NULL",
                    [],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )?)
            })
            .unwrap();
        assert_eq!(
            pending,
            (
                1,
                "user_play_next".into(),
                "External Artist".into(),
                "External Title".into(),
                Some(777)
            )
        );

        let shifted_pos: i32 = db
            .with_conn(|conn| {
                Ok(
                    conn.query_row("SELECT position FROM queue WHERE track_id = 2", [], |row| {
                        row.get(0)
                    })?,
                )
            })
            .unwrap();
        assert_eq!(shifted_pos, 2);

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn queue_play_next_many_preserves_requested_order() {
        let (db, db_path) = fresh_migrated_db();
        seed_basic_tracks(&db);
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
                [],
            )?;
            let qid = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'user')",
                [],
            )?;
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
                 WHERE id = 1",
                rusqlite::params![qid],
            )?;
            Ok(())
        })
        .unwrap();

        let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
            db.clone(),
        ))));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/queue/play_next_many")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"items":[
                            {"kind":"tidal","tidal_id":101,"artist":"A","title":"First external"},
                            {"kind":"tidal","tidal_id":102,"artist":"B","title":"Second external"}
                        ]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["queue"].as_array().unwrap().len(), 4);
        assert_eq!(body["queue"][1]["track"]["title"], "First external");
        assert_eq!(body["queue"][2]["track"]["title"], "Second external");
        assert_eq!(body["queue"][3]["track"]["id"], 2);

        let rows: Vec<(i32, String, Option<i64>)> = db
            .with_conn(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT position, pending_title, tidal_id_hint
                     FROM queue
                     WHERE track_id IS NULL
                     ORDER BY position ASC",
                )?;
                Ok(stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .unwrap();
        assert_eq!(
            rows,
            vec![
                (1, "First external".to_string(), Some(101)),
                (2, "Second external".to_string(), Some(102)),
            ]
        );

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn queue_append_external_track_creates_pending_row_without_hint() {
        let (db, db_path) = fresh_migrated_db();
        let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
            db.clone(),
        ))));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/queue/append")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"kind":"external","artist":"Aphex Twin","title":"Xtal"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["queue"].as_array().unwrap().len(), 1);
        assert_eq!(body["queue"][0]["is_pending"], true);
        assert_eq!(body["queue"][0]["track"]["artist_name"], "Aphex Twin");
        assert_eq!(body["queue"][0]["track"]["title"], "Xtal");

        let hint: Option<i64> = db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT tidal_id_hint FROM queue WHERE track_id IS NULL",
                    [],
                    |row| row.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(hint, None);

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn promote_pending_row_emit_broadcasts_queue_updated() {
        let db_path = std::env::temp_dir().join(format!("noor-test-{}.db", uuid::Uuid::new_v4()));
        let db = Database::open(&db_path).expect("db opened");
        db.run_migrations().expect("migrations");
        db.with_conn(|conn| schema::run_migrations(conn))
            .expect("schema migrations");

        // Seed an artist + a real track to be the promotion target, plus a
        // pending queue row pointing at "Pending Artist / Pending Title".
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (1, 'Promoted Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (
                    id, title, artist_id, source, fidelity_score
                 ) VALUES (1, 'Promoted Title', 1, 'tidal_stream', 0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (track_id, position, source, pending_artist, pending_title, pending_at)
                 VALUES (NULL, 0, 'radio_pending', 'Pending Artist', 'Pending Title', datetime('now'))",
                [],
            )?;
            Ok(())
        })
        .expect("seed");

        let queue_item_id: i64 = db
            .with_conn(|conn| {
                Ok(
                    conn.query_row("SELECT id FROM queue WHERE track_id IS NULL", [], |row| {
                        row.get(0)
                    })?,
                )
            })
            .unwrap();

        let (event_tx, mut rx) = tokio::sync::broadcast::channel(8);
        let promoted = promote_pending_row_emit(&db, &event_tx, queue_item_id, 1, 950);
        assert!(promoted, "promotion must succeed for a NULL-track row");

        let evt = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("event arrived in time")
            .expect("event channel open");
        assert!(matches!(evt, AppEvent::QueueUpdated));

        // Confirm DB: the row is no longer pending.
        let resolved_track_id: Option<i64> = db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT track_id FROM queue WHERE id = ?1",
                    rusqlite::params![queue_item_id],
                    |row| row.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(resolved_track_id, Some(1));

        // Idempotency: a second promotion attempt is a no-op (track_id already set)
        // and must NOT broadcast a second event.
        let again = promote_pending_row_emit(&db, &event_tx, queue_item_id, 1, 950);
        assert!(!again);
        let no_more = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await;
        assert!(
            no_more.is_err(),
            "no second event should fire on idempotent retry"
        );

        let _ = std::fs::remove_file(db_path);
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
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
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
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(resp2.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["host_mode"], true);
        assert!(
            body["bind_address"]
                .as_str()
                .unwrap()
                .starts_with("0.0.0.0")
        );
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
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
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
            body["hint"]
                .as_str()
                .unwrap_or("")
                .contains("seed_tidal_id"),
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
