use super::transport::command as transport_command;
use super::transport::events::{
    describe_tidal_playback_error, handle_near_end, report_playback_failure,
};
use super::transport::generation::current as current_playback_generation;
use super::transport::listen::{flush_active_listen_session_locked, record_transition_if_changed};
use super::transport::pending::{resolve_pending_row, spawn_pending_queue_resolver};
use super::transport::runtime::RuntimeUnavailable;
use super::transport::settings::{
    reissue_current_track_at_new_quality, runtime_output_settings_from_audio_settings,
};
use super::transport::snapshot::build_live_playback_snapshot;
use super::transport::snapshot::{
    current_playback_track_id, overlay_snapshot_with_external_track, recently_cleared,
};
use super::transport::start::{Dispatch, StartError};
use super::transport::stream::{
    TidalPlaybackError, resolve_tidal_playback_stream, runtime_stream_resolver,
};
use super::transport::toggle as transport_toggle;
use crate::db::queries;
use crate::metadata::discogs::DiscogsClient;
use crate::metadata::lastfm::LastFmClient;
use crate::playback::{automix, player, queue, runtime as playback_runtime};
use crate::services::discovery::{DiscoveryCandidateSeed, TidalDiscoveryProvider};
use crate::services::learning as discovery_learning;
use crate::services::tidal::{
    auth as tidal_auth,
    client::{TidalClient, TidalSearchCatalog, TidalSearchTrack, TidalSearchVideo, TidalTrack},
    import as tidal_import, stream as tidal_stream,
};
use crate::smart::external_discovery as external_discovery_engine;
use crate::{AppEvent, SharedState};
use anyhow::Context;
pub(crate) use audio_settings_routes::*;
use axum::{
    Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Json, Response},
    routing::{get, patch, post, put},
};
pub(crate) use discovery_search_routes::*;
pub(crate) use dj_pair::*;
pub(crate) use favorite_routes::*;
pub(crate) use queue_routes::*;
pub(crate) use radio_routes::*;
use resolve_routes::*;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
pub(in crate::server) use tidal_auth_routes::*;
pub(crate) use tidal_catalog_routes::*;
pub(crate) use tidal_library::*;
pub(crate) use tidal_match::*;
use tracing::{error, info, warn};

mod analytics_routes;
mod artwork_cache_routes;
mod audio_analysis_routes;
mod audio_settings_routes;
pub(crate) mod catalog_routes;
mod catalogue_routes;
mod chart_routes;
mod discovery_routes;
mod discovery_search_routes;
mod discovery_space_routes;
mod dj_pair;
pub(crate) mod dj_routes;
mod download_routes;
mod duplicates_routes;
mod enrichment_routes;
mod favorite_routes;
mod genre_routes;
pub(crate) mod home_routes;
pub(crate) mod home_suggestions;
mod library_batch_routes;
pub(crate) mod maintenance_routes;
mod playlist_routes;
mod queue_routes;
mod radio_routes;
mod resolve_routes;
mod search_routes;
mod setup_discovery_routes;
mod sportify_routes;
mod tidal_auth_routes;
mod tidal_catalog_routes;
mod tidal_content_routes;
mod tidal_home_routes;
mod tidal_library;
mod tidal_match;
mod tidal_sync_routes;
mod video_discovery_routes;
mod video_station_routes;
pub use discovery_routes::{TrainingSpawn, spawn_discovery_training};
pub use tidal_sync_routes::trigger_auto_sync;

type TidalPlaylistTracksCache = Arc<Mutex<HashMap<String, (Instant, Vec<TidalTrack>)>>>;

pub(crate) const TIDAL_PLAYLIST_TRACKS_CACHE_TTL: Duration = Duration::from_secs(60 * 60);
pub(crate) const PLAYBACK_FINISH_DB_LOCK_RETRY_LIMIT: usize = 60;
pub(crate) const PLAYBACK_FINISH_DB_LOCK_RETRY_DELAY_SECS: u64 = 2;
pub(crate) const PLAYBACK_ADVANCE_PENDING_SKIP_LIMIT: usize = 8;
pub(crate) const PLAYBACK_PENDING_BUSY_RETRY_LIMIT: usize = 5;
pub(crate) const PLAYBACK_PENDING_BUSY_RETRY_DELAY_MS: u64 = 200;
const TIDAL_SEARCH_UPSTREAM_TIMEOUT_SECS: u64 = 8;

fn current_user_audio_quality_locked(
    state: &crate::AppState,
) -> Option<crate::db::audio_settings::AudioQuality> {
    state
        .db
        .with_conn(|conn| crate::db::audio_settings::load(conn).map_err(anyhow::Error::from))
        .ok()
        .map(|settings| settings.quality)
}

#[derive(Debug, Deserialize)]
pub struct PlaybackTrackRequest {
    track_id: i64,
}

/// Body of POST /api/playback/queue/play-item - jump to a queue row by id.
#[derive(Debug, Deserialize)]
pub struct PlayQueueItemRequest {
    queue_item_id: i64,
}

#[derive(Debug, Deserialize)]
pub struct PositionRequest {
    position_ms: i64,
    /// Opt in to the segment-restart path for out-of-buffer targets (option C:
    /// true DASH segment seek). When false the runtime rejects out-of-buffer
    /// seeks with HTTP 409 (#43 behavior). When true the runtime tears down
    /// the current engine and starts a new one at the nearest DASH segment
    /// boundary. Default `false` so existing clients (mobile remote, future
    /// integrators) keep the safer semantics.
    #[serde(default)]
    allow_segment_seek: bool,
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

pub fn api_routes(state: SharedState) -> Router {
    Router::new()
        .route(
            "/api/tidal/content-settings",
            get(tidal_content_routes::get_settings).put(tidal_content_routes::put_settings),
        )
        .route(
            "/api/setup/discovery",
            get(setup_discovery_routes::get_status).post(setup_discovery_routes::update),
        )
        // Library endpoints
        .route("/api/tracks", get(catalog_routes::get_tracks))
        .route("/api/tracks/count", get(catalog_routes::get_track_count))
        .route("/api/history", get(catalog_routes::get_history))
        .route("/api/albums", get(catalog_routes::get_albums))
        .route(
            "/api/albums/decades",
            get(catalog_routes::get_album_decades),
        )
        .route(
            "/api/albums/{id}/credits",
            get(catalog_routes::get_album_credits),
        )
        .route(
            "/api/albums/{id}/tracks",
            get(catalog_routes::get_album_tracks),
        )
        .route(
            "/api/albums/{id}/spotify-stats",
            get(catalog_routes::get_album_spotify_stats),
        )
        .route("/api/artists", get(catalog_routes::get_artists))
        .route(
            "/api/artists/letters",
            get(catalog_routes::get_artist_letters),
        )
        .route("/api/artists/{id}", get(catalog_routes::get_artist))
        .route(
            "/api/artists/{id}/tracks",
            get(catalog_routes::get_artist_tracks),
        )
        .route(
            "/api/artists/{id}/discography",
            get(catalog_routes::get_artist_discography),
        )
        .route(
            "/api/artists/{id}/spotify-stats",
            get(catalog_routes::get_artist_spotify_stats),
        )
        .route(
            "/api/tidal/albums/{id}/tracks",
            get(catalog_routes::get_tidal_album_tracks),
        )
        .route(
            "/api/tidal/albums/{id}/import",
            post(catalog_routes::import_tidal_album),
        )
        .route(
            "/api/tidal/tracks/import",
            post(catalog_routes::import_tidal_track_for_radio),
        )
        .route("/api/genres", get(genre_routes::get_genres))
        .route(
            "/api/genres/snapshot",
            get(genre_routes::get_genre_snapshot),
        )
        .route("/api/genres/heat", get(genre_routes::get_genre_heat))
        .route(
            "/api/genres/co-occurrence",
            get(genre_routes::get_genre_co_occurrence),
        )
        .route("/api/genres/cohorts", get(genre_routes::get_genre_cohorts))
        .route(
            "/api/genres/evolution",
            get(genre_routes::get_genre_evolution),
        )
        .route(
            "/api/genres/audio-metrics",
            get(genre_routes::get_genre_audio_metrics),
        )
        .route(
            "/api/genres/{id}/tracks",
            get(genre_routes::get_genre_tracks),
        )
        .route(
            "/api/playlists",
            get(playlist_routes::get_playlists).post(playlist_routes::create_playlist_route),
        )
        .route(
            "/api/playlists/{id}",
            patch(playlist_routes::update_playlist_route)
                .delete(playlist_routes::delete_playlist_route),
        )
        .route(
            "/api/playlists/{id}/tracks",
            get(playlist_routes::get_playlist_tracks)
                .post(playlist_routes::add_tracks_to_playlist_route)
                .delete(playlist_routes::remove_playlist_tracks_route),
        )
        .route(
            "/api/playlists/{id}/tracks/move",
            post(playlist_routes::move_playlist_track_route),
        )
        .route(
            "/api/playlists/{id}/refresh",
            post(playlist_routes::refresh_playlist_route),
        )
        .route(
            "/api/playlists/{id}/favorite",
            patch(playlist_routes::toggle_playlist_favorite_route),
        )
        .route(
            "/api/playlists/{id}/cover-sample",
            get(playlist_routes::get_playlist_cover_sample),
        )
        .route(
            "/api/smart/playlists",
            post(playlist_routes::create_smart_playlist_route),
        )
        .route(
            "/api/smart/playlists/{id}",
            put(playlist_routes::update_smart_playlist_route)
                .delete(playlist_routes::delete_smart_playlist_route),
        )
        .route(
            "/api/smart/playlists/{id}/evaluate",
            get(playlist_routes::evaluate_smart_playlist),
        )
        .route(
            "/api/smart/playlists/preview",
            post(playlist_routes::preview_smart_playlist),
        )
        .route(
            "/api/artists/search",
            get(playlist_routes::search_artists_route),
        )
        .route(
            "/api/analytics/overview",
            get(analytics_routes::get_analytics_overview),
        )
        .route(
            "/api/analytics/dashboard",
            get(analytics_routes::get_analytics_dashboard),
        )
        .route(
            "/api/analytics/signals",
            get(analytics_routes::get_analytics_signals),
        )
        .route(
            "/api/analytics/listens/recent",
            get(analytics_routes::get_recent_listens),
        )
        .route(
            "/api/discovery/preview",
            post(discovery_routes::preview_discovery),
        )
        .route(
            "/api/discovery/new",
            post(discovery_routes::discover_new_music),
        )
        .route(
            "/api/discovery/save",
            post(discovery_routes::save_discovery_track),
        )
        .route("/api/discovery/play", post(play_discovery_track))
        .route(
            "/api/discovery/connections",
            post(discovery_routes::discover_connected_music),
        )
        .route(
            "/api/discovery/status",
            get(discovery_routes::get_discovery_status),
        )
        .route(
            "/api/discovery/train",
            post(discovery_routes::start_discovery_training),
        )
        .route(
            "/api/discovery/train/status",
            get(discovery_routes::get_discovery_training_status),
        )
        .route(
            "/api/discovery/train/stop",
            post(discovery_routes::stop_discovery_training),
        )
        .route(
            "/api/discovery/train/intensity",
            get(discovery_routes::get_discovery_intensity)
                .post(discovery_routes::set_discovery_intensity),
        )
        .route(
            "/api/discovery/train/engine",
            get(discovery_routes::get_discovery_engine)
                .post(discovery_routes::set_discovery_engine),
        )
        .route(
            "/api/discovery/train/safety",
            get(discovery_routes::get_discovery_safety),
        )
        .route(
            "/api/discovery/train/safety-profile",
            get(discovery_routes::get_discovery_safety_profile)
                .post(discovery_routes::set_discovery_safety_profile),
        )
        .route(
            "/api/discovery/feedback",
            post(discovery_routes::record_discovery_feedback),
        )
        .route(
            "/api/discovery/upgrade",
            get(discovery_routes::get_discovery_upgrade),
        )
        .route(
            "/api/discovery/feedback/summary",
            get(discovery_routes::discovery_feedback_summary),
        )
        .route(
            "/api/recommendations/not-for-me",
            post(discovery_routes::set_not_for_me).delete(discovery_routes::clear_not_for_me),
        )
        .route(
            "/api/discovery/presets",
            get(discovery_routes::get_discovery_presets)
                .post(discovery_routes::create_discovery_preset),
        )
        // Similar Radio
        .route("/api/discovery/radio", post(get_radio_tracks))
        .route(
            "/api/discovery/radio/compute",
            post(compute_radio_similarity),
        )
        .route("/api/discovery/radio/status", get(radio_similarity_status))
        // Discovery Sound Space
        .route(
            "/api/discovery/space",
            post(discovery_space_routes::get_discovery_space),
        )
        .route(
            "/api/discovery/blend/space",
            post(discovery_space_routes::get_discovery_blend_space),
        )
        .route(
            "/api/discovery/blend/add",
            post(discovery_space_routes::add_discovery_blend_to_queue),
        )
        .route(
            "/api/discovery/blend/play",
            post(discovery_space_routes::play_discovery_blend),
        )
        .route(
            "/api/discovery/blend/radio",
            post(discovery_space_routes::make_discovery_blend_radio),
        )
        .route(
            "/api/discovery/rerank",
            post(discovery_space_routes::rerank_discovery_space),
        )
        .route(
            "/api/discovery/space/queue",
            post(discovery_space_routes::queue_discovery_space_tracks),
        )
        // Sportify-based discovery resolver - single, bulk, and cache-only status poll.
        .route("/api/resolve/tidal/track", get(resolve_tidal_track))
        .route(
            "/api/library/catalogue/status",
            get(catalogue_routes::status),
        )
        .route("/api/resolve/tidal/bulk", post(resolve_tidal_bulk))
        .route("/api/resolve/tidal/status", get(resolve_tidal_status))
        // Sportify (anonymous Spotify metadata proxy) discovery surface.
        // Sportify is upstream and subject to breakage - every handler is
        // cache-first, every failure surfaces as JSON error or empty list,
        // and nothing here writes to library tables. Worst case for an
        // outage is a degraded /discover; existing library data is never
        // affected.
        .route(
            "/api/discovery/sportify/search",
            get(sportify_routes::sportify_discovery_search),
        )
        .route(
            "/api/discovery/sportify/track/{spotify_id}",
            get(sportify_routes::sportify_discovery_track),
        )
        .route(
            "/api/discovery/sportify/album/{spotify_id}",
            get(sportify_routes::sportify_discovery_album),
        )
        .route(
            "/api/discovery/sportify/playlist/{spotify_id}/meta",
            get(sportify_routes::sportify_discovery_playlist_meta),
        )
        .route(
            "/api/discovery/sportify/playlist/{spotify_id}",
            get(sportify_routes::sportify_discovery_playlist),
        )
        .route(
            "/api/discovery/sportify/artist/{spotify_id}",
            get(sportify_routes::sportify_discovery_artist),
        )
        .route(
            "/api/discovery/sportify/artist/{spotify_id}/top-tracks",
            get(sportify_routes::sportify_discovery_artist_top_tracks),
        )
        .route(
            "/api/discovery/sportify/artist/{spotify_id}/related",
            get(sportify_routes::sportify_discovery_artist_related),
        )
        .route(
            "/api/discovery/sportify/album/{spotify_id}/related",
            get(sportify_routes::sportify_discovery_album_related),
        )
        .route(
            "/api/discovery/sportify/track/{spotify_id}/related",
            get(sportify_routes::sportify_discovery_track_related),
        )
        // Save an ephemeral Spotify-sourced playlist into the user's library.
        // Imports each resolved TIDAL track + creates a noor playlist; rows
        // without a TIDAL match are skipped (counted in the response).
        .route(
            "/api/spotify-playlist/save",
            post(sportify_routes::save_spotify_playlist),
        )
        .route(
            "/api/spotify-track/save",
            post(sportify_routes::save_spotify_track),
        )
        .route(
            "/api/spotify-album/save",
            post(sportify_routes::save_spotify_album),
        )
        .route("/api/radio/song", post(radio_song))
        .route("/api/radio/album", post(radio_album))
        .route("/api/radio/artist", post(radio_artist))
        .route("/api/radio/start", post(radio_start))
        .route(
            "/api/discovery/space/meta",
            get(discovery_space_routes::get_discovery_space_meta),
        )
        .route("/api/discovery/artists", get(get_discovery_artists))
        .route(
            "/api/library/batch/add-to-playlist",
            post(library_batch_routes::batch_add_to_playlist),
        )
        .route(
            "/api/library/batch/delete",
            post(library_batch_routes::batch_delete_items),
        )
        .route(
            "/api/library/batch/set-genre",
            post(library_batch_routes::batch_set_genre),
        )
        .route(
            "/api/library/enrich/musicbrainz",
            post(enrichment_routes::start_musicbrainz_enrichment),
        )
        .route(
            "/api/library/enrich/musicbrainz/status",
            get(enrichment_routes::get_musicbrainz_status),
        )
        .route(
            "/api/library/enrich/musicbrainz/portable",
            get(enrichment_routes::get_musicbrainz_portable_snapshot),
        )
        .route(
            "/api/library/enrich/musicbrainz/portable/export",
            post(enrichment_routes::export_musicbrainz_portable_snapshot),
        )
        .route(
            "/api/library/enrich/musicbrainz/portable/import",
            post(enrichment_routes::import_musicbrainz_portable_snapshot),
        )
        .merge(dj_routes::routes())
        .route("/api/library/tracks/favorite", post(set_track_favorite))
        .route("/api/library/albums/favorite", post(set_album_favorite))
        // Duplicates
        .route(
            "/api/library/duplicates/scan",
            post(duplicates_routes::scan_duplicates),
        )
        .route(
            "/api/library/duplicates",
            get(duplicates_routes::get_duplicates),
        )
        .route(
            "/api/library/duplicates/{group_id}/resolve",
            post(duplicates_routes::resolve_duplicate_group),
        )
        .route(
            "/api/library/duplicates/{group_id}/dismiss",
            post(duplicates_routes::dismiss_duplicate_group),
        )
        // Playback
        .route("/api/playback/state", get(get_playback_state))
        .route("/api/playback/runtime", get(get_playback_runtime))
        .route("/api/playback/play", post(play_track))
        .route("/api/playback/pause", post(pause_playback))
        .route("/api/playback/resume", post(resume_playback))
        .route(
            "/api/playback/exclusive/release",
            post(release_exclusive_playback),
        )
        .route("/api/playback/previous", post(previous_track))
        .route("/api/playback/next", post(next_track))
        .route("/api/playback/position", post(set_playback_position))
        .route("/api/playback/volume", post(set_playback_volume))
        .route("/api/playback/shuffle", post(set_playback_shuffle))
        .route("/api/playback/repeat", post(set_playback_repeat))
        .route("/api/playback/automix", post(set_playback_automix))
        // Track downloads (FLAC/MP3 export to disk)
        .route(
            "/api/downloads/settings",
            get(download_routes::get_download_settings)
                .post(download_routes::set_download_settings),
        )
        .route(
            "/api/tracks/{id}/download",
            post(download_routes::download_track),
        )
        .route(
            "/api/tidal/download",
            post(download_routes::download_tidal_track),
        )
        .route(
            "/api/tidal/downloads/batch",
            post(download_routes::download_tidal_batch),
        )
        .route(
            "/api/downloads/batch",
            post(download_routes::download_batch),
        )
        .route(
            "/api/downloads/cancel",
            post(download_routes::cancel_downloads),
        )
        .route(
            "/api/downloads/status",
            get(download_routes::download_status),
        )
        .route(
            "/api/playback/queue",
            get(get_playback_queue).post(replace_playback_queue),
        )
        .route("/api/playback/queue/add", post(add_queue_track))
        .route("/api/playback/queue/play-item", post(play_queue_item))
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
        .route("/api/search", get(search_routes::search))
        .route("/api/search/audio", post(search_routes::search_audio))
        .route("/api/search/vibe", get(search_routes::search_vibe))
        .route(
            "/api/search/underrated",
            get(search_routes::search_underrated),
        )
        // TIDAL
        .route("/api/tidal/login", post(tidal_login))
        .route("/api/tidal/login/complete", post(tidal_login_complete))
        .route("/api/tidal/login/poll", post(tidal_poll))
        .route(
            "/api/tidal/sync",
            post(tidal_sync_routes::tidal_sync_library),
        )
        .route(
            "/api/tidal/sync/cancel",
            post(tidal_sync_routes::tidal_sync_cancel),
        )
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
        .route(
            "/api/tidal/video-playlists/{uuid}/items",
            get(tidal_video_playlist_items),
        )
        // Editorial video sets for the /videos browse state. Stale-while-
        // revalidate over persisted daily snapshots; never blocks on TIDAL.
        .route(
            "/api/videos/discover",
            get(video_discovery_routes::get_videos_discover),
        )
        .route(
            "/api/videos/radio/next",
            post(video_discovery_routes::post_videos_radio_next),
        )
        .route(
            "/api/videos/related",
            post(video_discovery_routes::post_videos_related),
        )
        .route(
            "/api/videos/history",
            get(video_discovery_routes::get_videos_history)
                .post(video_discovery_routes::post_videos_history)
                .delete(video_discovery_routes::delete_videos_history),
        )
        .route(
            "/api/videos/history/videos/{video_id}",
            axum::routing::delete(video_discovery_routes::delete_video_from_history),
        )
        .route(
            "/api/videos/history/{id}/finish",
            post(video_discovery_routes::post_videos_history_finish),
        )
        .route(
            "/api/videos/discovery/status",
            get(video_discovery_routes::get_video_discovery_status),
        )
        .route(
            "/api/artwork-cache",
            get(artwork_cache_routes::get_artwork_cache)
                .put(artwork_cache_routes::put_artwork_cache),
        )
        .route(
            "/api/videos/discovery/settings",
            get(video_discovery_routes::get_video_discovery_settings)
                .put(video_discovery_routes::put_video_discovery_settings),
        )
        .route(
            "/api/videos/stations",
            get(video_station_routes::get_video_stations),
        )
        .route(
            "/api/videos/stations/settings",
            get(video_station_routes::get_video_station_settings)
                .put(video_station_routes::put_video_station_settings),
        )
        .route(
            "/api/videos/stations/{id}/next",
            post(video_station_routes::post_video_station_next),
        )
        // The liked-videos library wall. Pure reads over what the background
        // resolve has found; the TIDAL fan-out is never on a request path.
        .route(
            "/api/videos/liked",
            get(video_discovery_routes::get_videos_liked),
        )
        .route(
            "/api/videos/saved",
            get(video_discovery_routes::get_saved_videos)
                .post(video_discovery_routes::post_saved_video),
        )
        .route(
            "/api/videos/liked/refresh",
            post(video_discovery_routes::post_videos_liked_refresh),
        )
        .route(
            "/api/videos/liked/hide",
            post(video_discovery_routes::post_videos_liked_hide),
        )
        .route("/api/tidal/playlists/search", get(tidal_playlist_search))
        .route(
            "/api/tidal/playlists/{uuid}/tracks",
            get(tidal_playlist_tracks),
        )
        .route("/api/tidal/artists/{tidal_id}/core", get(tidal_artist_core))
        .route("/api/tidal/artists/{tidal_id}", get(tidal_artist_profile))
        .route(
            "/api/tidal/artists/{tidal_id}/releases",
            get(catalog_routes::get_tidal_artist_release_page),
        )
        .route("/api/tidal/logout", post(tidal_logout))
        .route(
            "/api/library/tidal-stream/purge",
            post(enrichment_routes::purge_orphan_tidal_stream_tracks),
        )
        // Last.fm
        .route(
            "/api/lastfm/config",
            post(enrichment_routes::lastfm_save_config)
                .get(enrichment_routes::lastfm_status)
                .delete(enrichment_routes::lastfm_clear_config),
        )
        .route("/api/lastfm/status", get(enrichment_routes::lastfm_status))
        .route(
            "/api/listenbrainz/config",
            post(enrichment_routes::listenbrainz_save_config)
                .get(enrichment_routes::listenbrainz_status)
                .delete(enrichment_routes::listenbrainz_clear_config),
        )
        .route(
            "/api/listenbrainz/status",
            get(enrichment_routes::listenbrainz_status),
        )
        // Last.fm scrobble auth (server-side flow - `LASTFM_API_SECRET` env required)
        .route(
            "/api/lastfm/auth/start",
            post(enrichment_routes::lastfm_auth_start),
        )
        .route(
            "/api/lastfm/auth/complete",
            post(enrichment_routes::lastfm_auth_complete),
        )
        .route(
            "/api/lastfm/auth/disconnect",
            post(enrichment_routes::lastfm_auth_disconnect),
        )
        .route(
            "/api/library/enrich/lastfm",
            post(enrichment_routes::start_lastfm_enrichment),
        )
        .route(
            "/api/library/enrich/lastfm/stop",
            post(enrichment_routes::stop_lastfm_enrichment),
        )
        .route(
            "/api/library/enrich/lastfm/status",
            get(enrichment_routes::get_lastfm_enrichment_status),
        )
        .route(
            "/api/library/enrich/lastfm/reset",
            post(enrichment_routes::reset_lastfm_enrichment),
        )
        .route("/api/scrobbling/backfill", post(scrobbling_backfill))
        // Audio analysis
        .route(
            "/api/library/analyze/audio-features",
            post(audio_analysis_routes::start_audio_analysis),
        )
        .route(
            "/api/library/analyze/stop",
            post(audio_analysis_routes::stop_audio_analysis),
        )
        .route(
            "/api/library/analyze/status",
            get(audio_analysis_routes::get_audio_analysis_status),
        )
        .route(
            "/api/library/analyze/passive",
            get(audio_analysis_routes::get_passive_dsp).put(audio_analysis_routes::set_passive_dsp),
        )
        .route(
            "/api/tracks/{id}/audio-features",
            get(audio_analysis_routes::get_track_audio_features),
        )
        .route(
            "/api/tracks/{id}/bpm-multiplier",
            post(audio_analysis_routes::set_bpm_multiplier),
        )
        .route(
            "/api/library/audio-features/stats",
            get(audio_analysis_routes::get_audio_features_stats),
        )
        .route(
            "/api/library/audio-features/quality",
            get(audio_analysis_routes::get_audio_features_quality),
        )
        .route(
            "/api/library/analytics",
            get(audio_analysis_routes::get_library_analytics),
        )
        .route(
            "/api/library/analyze/reanalyze-stale",
            get(audio_analysis_routes::reanalyze_stale_tracks),
        )
        .route(
            "/api/library/analyze/reset",
            post(audio_analysis_routes::reset_audio_analysis),
        )
        .route("/api/sync/info", get(tidal_sync_routes::get_sync_info))
        .route("/api/sync/auto", post(tidal_sync_routes::set_auto_sync))
        .route(
            "/api/sync/enrichment",
            post(tidal_sync_routes::set_sync_enrichment),
        )
        .route(
            "/api/tidal/reclean",
            post(tidal_sync_routes::tidal_reclean_library),
        )
        // Status
        .route("/api/status", get(status))
        // Home page discovery endpoints
        .route("/api/home/releases", get(home_routes::get_home_releases))
        .route("/api/home/picks", get(home_routes::get_home_picks))
        .route(
            "/api/home/recommendations",
            get(home_routes::get_home_recommendations),
        )
        .route(
            "/api/home/suggestions",
            post(home_suggestions::get_home_suggestions),
        )
        .route(
            "/api/home/shuffle-picks",
            get(home_routes::get_home_shuffle_picks),
        )
        .route(
            "/api/library/top-artists",
            get(home_routes::get_library_top_artists),
        )
        .route("/api/home/articles", get(home_routes::get_home_articles))
        .route("/api/home/news", get(home_routes::get_home_news))
        // TIDAL "Your Mixes" - drives the home Your Mixes shelf above Trending.
        .route("/api/tidal/mixes", get(tidal_home_routes::get_tidal_mixes))
        .route(
            "/api/tidal/mixes/{id}/tracks",
            get(tidal_home_routes::get_tidal_mix_tracks),
        )
        // TIDAL "Personal Radio" - drives the home Personal Radio shelf.
        .route(
            "/api/tidal/radio-stations",
            get(tidal_home_routes::get_tidal_radio_stations),
        )
        // TIDAL editorial home modules - drives the search-page discover surface.
        .route(
            "/api/tidal/home-modules",
            get(tidal_home_routes::get_tidal_home_modules),
        )
        // Per-module detail items (View all). Resolves the module's
        // dataApiPath server-side and returns the full item set.
        .route(
            "/api/tidal/discover-modules/{id}/items",
            get(tidal_home_routes::get_tidal_discover_module_items),
        )
        // Generic editorial page modules. Whitelisted in the handler to
        // documented top-level pages plus mood/{id} / genre/{id}. Universal
        // across the /v1/pages/* response shape.
        .route(
            "/api/tidal/page/{section}",
            get(tidal_home_routes::get_tidal_page_modules),
        )
        .route(
            "/api/tidal/page/{section}/{id}",
            get(tidal_home_routes::get_tidal_page_modules_with_id),
        )
        // Dedicated mood routes: the moods landing returns PAGE_LINKS items,
        // which aren't tracks/albums/playlists, so they go through a parser
        // that just extracts category metadata. Drill-down then proxies to
        // the corresponding pages/{slug} TIDAL endpoint.
        .route("/api/tidal/moods", get(tidal_home_routes::get_tidal_moods))
        .route(
            "/api/tidal/mood-page/{slug}",
            get(tidal_home_routes::get_tidal_mood_page),
        )
        // Trending / charts (Phase 5)
        .route("/api/charts", get(chart_routes::get_charts))
        .route(
            "/api/charts/snapshots",
            get(chart_routes::get_chart_snapshots),
        )
        .route(
            "/api/charts/spotify/daily/import",
            post(chart_routes::import_spotify_daily_snapshot),
        )
        .route("/api/charts/matrix", get(chart_routes::get_chart_matrix))
        .route(
            "/api/charts/matrix/refresh",
            post(chart_routes::refresh_chart_matrix),
        )
        .route(
            "/api/charts/lastfm/genres",
            get(chart_routes::list_lastfm_genres),
        )
        .route(
            "/api/charts/lastfm/countries",
            get(chart_routes::list_lastfm_countries),
        )
        // Server auth management
        .route("/api/server/token", get(get_server_token_handler))
        .route(
            "/api/server/token/regenerate",
            post(regenerate_server_token_handler),
        )
        // Database size + user-triggered compaction
        .route(
            "/api/server/database/stats",
            get(maintenance_routes::get_database_stats),
        )
        .route(
            "/api/server/database/compact",
            post(maintenance_routes::compact_database),
        )
        // Server configuration
        .route("/api/server/info", get(get_server_info))
        .route("/api/server/host_mode", put(put_server_host_mode))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            tidal_content_routes::filter_browse,
        ))
        .with_state(state)
}

async fn get_server_token_handler(State(state): State<SharedState>) -> impl IntoResponse {
    let remote = state.read().await.remote.clone();
    let token = remote.shared_pin().await;
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({ "token": token })),
    )
}

async fn regenerate_server_token_handler(
    State(state): State<SharedState>,
) -> Result<impl IntoResponse, StatusCode> {
    let remote = state.read().await.remote.clone();
    let (new_token, _revoked_devices) = crate::server::remote::reset_all(&remote)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    {
        let mut s = state.write().await;
        s.server_token = new_token.clone();
    }
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({ "token": new_token })),
    ))
}

async fn get_server_info(State(state): State<SharedState>) -> Json<Value> {
    let remote = state.read().await.remote.clone();
    let runtime = remote.runtime_snapshot().await;
    Json(json!({
        "host_mode": runtime.configured_host_mode,
        "bind_address": runtime.bind_address.to_string(),
        "effective_host_mode": runtime.effective_host_mode,
        "restart_required": runtime.configured_host_mode != runtime.effective_host_mode,
        "control": runtime.control,
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn put_server_host_mode(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let host_mode = body
        .get("host_mode")
        .and_then(|v| v.as_bool())
        .ok_or(StatusCode::BAD_REQUEST);
    let Ok(host_mode) = host_mode else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    let remote = state.read().await.remote.clone();
    let before = remote.runtime_snapshot().await;
    match before.control {
        crate::server::remote::HostControl::Desktop => {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error": "DESKTOP_MANAGED", "message": "Host mode is managed by the desktop application."})),
            )
                .into_response();
        }
        crate::server::remote::HostControl::Environment
        | crate::server::remote::HostControl::CommandLine => {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error": "EXTERNAL_BIND_OVERRIDE", "message": "Host mode is controlled by the server launch configuration."})),
            )
                .into_response();
        }
        crate::server::remote::HostControl::Standalone => {}
    }

    if state
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
        .is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    remote.set_configured_host_mode(host_mode).await;
    Json(json!({
        "host_mode": host_mode,
        "bind_address": before.bind_address.to_string(),
        "effective_host_mode": before.effective_host_mode,
        "restart_required": host_mode != before.effective_host_mode,
    }))
    .into_response()
}

// --- Discovery Sound Space -----------------------------------------------

// --- Sportify bulk + status resolution endpoints ------------

// --- Sportify discovery read endpoints ----------------------

// --- POST /api/radio/start ---------------------------------------------------
//
// Atomically builds a radio queue from a seed track, inserting library tracks
// directly and non-library Last.fm results as pending rows, then spawns
// background resolvers bounded by RESOLVER_POOL_SIZE.

/// POST /api/playback/queue/play-item - jump playback to a specific queue row.
async fn play_queue_item(
    State(state): State<SharedState>,
    Json(payload): Json<PlayQueueItemRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    command_response(
        &state,
        transport_command::play_queue_item(&state, payload.queue_item_id).await,
    )
    .await
}

// -- MusicBrainz enrichment -------------------------------------------------

async fn get_playback_state(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let snapshot = build_live_playback_snapshot(&state)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({
        "state": snapshot.state,
        "queue": snapshot.queue,
        "queue_revision": snapshot.queue_revision
    })))
}

async fn get_playback_runtime(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let state = state.read().await;
    let dj_engine_enabled = state
        .db
        .with_conn(queries::is_dj_engine_enabled)
        .unwrap_or(false);
    let runtime = state.playback_runtime_info.as_ref().map(|info| {
        json!({
            "device_name": info.device_name,
            "sample_rate": info.sample_rate,
            "channels": info.channels,
            "active_track_id": info.active_track_id,
            "last_error": info.last_error,
            "exclusive_engaged": info.exclusive_engaged,
            "exclusive_transport_format": info.exclusive_transport_format,
            "dj_engine_enabled": dj_engine_enabled,
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
    // Queue snapshots are always read from persisted queue rows.
    let snapshot = overlay_snapshot_with_external_track(&state, snapshot).await;
    Ok(Json(json!({
        "queue": snapshot.queue,
        "queue_revision": snapshot.queue_revision
    })))
}

async fn play_track(
    State(state): State<SharedState>,
    Json(payload): Json<PlaybackTrackRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    command_response(
        &state,
        transport_command::play(&state, payload.track_id).await,
    )
    .await
}

/// HTTP adapter for a transport command result.
async fn command_response(
    state: &SharedState,
    result: Result<transport_command::Outcome, transport_command::CommandError>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    match result {
        Ok(transport_command::Outcome::Settled(snapshot)) => Ok(Json(json!({
            "state": snapshot.state,
            "queue": snapshot.queue,
            "queue_revision": snapshot.queue_revision
        }))),
        Ok(transport_command::Outcome::Current) => current_playback_snapshot_json(state).await,
        Err(error) => Err(command_error_response(state, error)),
    }
}

fn command_error_response(
    state: &SharedState,
    error: transport_command::CommandError,
) -> (StatusCode, Json<Value>) {
    use transport_command::CommandError;
    match error {
        CommandError::StateUpdate(message) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "status": "playback_state_update_failed",
                "message": message,
            })),
        ),
        CommandError::Start {
            error,
            track_id,
            stream_context,
            runtime_message,
        } => start_error_response(state, error, track_id, stream_context, runtime_message),
        CommandError::UnplayableAdvance(message) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "status": "playback_runtime_failed",
                "message": message,
            })),
        ),
        CommandError::InvalidTrackId(track_id) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "invalid_track_id",
                "message": "play_track requires a positive library track id.",
                "track_id": track_id,
            })),
        ),
        CommandError::TrackLookupFailed(track_id) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "status": "track_lookup_failed",
                "message": "Failed to load track before playback.",
                "track_id": track_id,
            })),
        ),
        CommandError::TrackNotFound(track_id) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "status": "track_not_found",
                "message": "Track not found.",
                "track_id": track_id,
            })),
        ),
        CommandError::PlaybackStartFailed(track_id) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "status": "playback_start_failed",
                "message": "Failed to start playback.",
                "track_id": track_id,
            })),
        ),
        CommandError::SnapshotUnavailable => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "Playback snapshot unavailable" })),
        ),
        CommandError::QueueItemNotFound => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "queue item not found" })),
        ),
    }
}

/// HTTP response for a failed start. `stream_context` is the message used for
/// stream failures; `runtime_message`, when set, replaces every runtime
/// acquisition error with a 502 carrying that message (the behaviour callers
/// had when they wrapped runtime errors themselves).
fn start_error_response(
    state: &SharedState,
    error: StartError,
    track_id: i64,
    stream_context: &str,
    runtime_message: Option<&str>,
) -> (StatusCode, Json<Value>) {
    match error {
        StartError::LocalUnsupported => (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({
                "status": "local_playback_not_supported",
                "message": "Local-library playback is not wired into the host audio runtime yet.",
                "track_id": track_id,
            })),
        ),
        StartError::Stream(error) => tidal_playback_error_response(track_id, error, stream_context),
        StartError::Runtime(error) => match runtime_message {
            Some(message) => (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "status": "playback_runtime_unavailable",
                    "message": message,
                    "track_id": track_id,
                })),
            ),
            None => runtime_unavailable_response(error, track_id),
        },
        StartError::Dispatch { dispatch, error } => {
            let verb = match dispatch {
                Dispatch::Play => "start",
                Dispatch::Switch => "switch",
            };
            let message = format!("Failed to {verb} host audio playback: {error}");
            report_playback_failure(state, &message);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "status": "playback_runtime_failed",
                    "message": message,
                    "track_id": track_id,
                })),
            )
        }
        // Callers return the current snapshot on Superseded before mapping;
        // this arm only keeps the match exhaustive.
        StartError::Superseded => (
            StatusCode::CONFLICT,
            Json(json!({
                "status": "playback_superseded",
                "message": "A newer playback command took over.",
                "track_id": track_id,
            })),
        ),
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

pub(crate) async fn pause_playback(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let snapshot = transport_toggle::pause(&state)
        .await
        .map_err(toggle_error_status)?;
    Ok(Json(json!({ "state": snapshot.state })))
}

/// Drop the WASAPI exclusive device immediately so the WebView can play a
/// TIDAL video's audio in shared mode. The frontend hits this when a video
/// starts. No-op when there's no runtime or exclusive mode is off; the runtime
/// re-grabs exclusive on the next Resume/Play. Returns ok even on a soft miss
/// so video startup never blocks on it.
async fn release_exclusive_playback(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    transport_toggle::release_exclusive(&state)
        .await
        .map_err(toggle_error_status)?;
    Ok(Json(json!({ "ok": true })))
}

pub(crate) async fn resume_playback(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let snapshot = transport_toggle::resume(&state)
        .await
        .map_err(toggle_error_status)?;
    Ok(Json(json!({ "state": snapshot.state })))
}

fn toggle_error_status(error: transport_toggle::ToggleError) -> StatusCode {
    match error {
        transport_toggle::ToggleError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        transport_toggle::ToggleError::Runtime(error) => runtime_unavailable_response(error, 0).0,
        transport_toggle::ToggleError::RecoveryFailed => StatusCode::BAD_GATEWAY,
    }
}

// --- Pending-row resolution --------------------------------------------------
//
// Both the lazy (next_track caller) and background-eager (radio_start) paths
// share the same scoring constants. The lazy path also closes the
// playback_state NULL window after promotion; the background path does not.

// Scoring weights (two-field, no album metadata available from Last.fm).
// Three-field variant (0.55/0.35/0.10) applies when pending_album is stored:
// not yet in schema; constants named here to make the future wiring obvious.

// How many search hits to consider when resolving a pending row. Heavily
// remixed songs push the plain studio cut well down TIDAL's relevance order, so
// pulling only the top few can leave the original out of the candidate set
// entirely. Ten is enough headroom without materially changing latency.

#[cfg(test)]
mod version_match_tests {
    use super::*;
    use std::collections::HashMap;

    fn track(
        id: i64,
        title: &str,
        artist: &str,
        version: Option<&str>,
        quality: &str,
    ) -> TidalSearchTrack {
        let mut extra = HashMap::new();
        if let Some(v) = version {
            extra.insert(
                "version".to_string(),
                serde_json::Value::String(v.to_string()),
            );
        }
        TidalSearchTrack {
            id,
            title: title.to_string(),
            duration: 200,
            artist_name: Some(artist.to_string()),
            audio_quality: Some(quality.to_string()),
            extra,
            ..Default::default()
        }
    }

    fn pick(artist: &str, title: &str, results: Vec<TidalSearchTrack>) -> Option<i64> {
        select_best_tidal_match(artist, title, results).map(|(_, t)| t.id)
    }

    #[test]
    fn clean_request_prefers_original_over_remix_regardless_of_order() {
        // The bug: both share base title "American Pie", so title+artist tie at
        // 1.0 and listing order decided the winner.
        let original = || track(1, "American Pie", "Don McLean", None, "LOSSLESS");
        let remix = || {
            track(
                2,
                "American Pie",
                "Don McLean",
                Some("L'Tric Remix"),
                "LOSSLESS",
            )
        };
        assert_eq!(
            pick("Don McLean", "American Pie", vec![original(), remix()]),
            Some(1)
        );
        assert_eq!(
            pick("Don McLean", "American Pie", vec![remix(), original()]),
            Some(1)
        );
    }

    #[test]
    fn remix_only_results_resolve_as_fallback() {
        let results = vec![track(
            2,
            "American Pie",
            "Don McLean",
            Some("L'Tric Remix"),
            "LOSSLESS",
        )];
        assert_eq!(pick("Don McLean", "American Pie", results), Some(2));
    }

    #[test]
    fn explicit_variant_request_takes_the_variant_not_the_original() {
        let results = vec![
            track(1, "Layla", "Eric Clapton", None, "LOSSLESS"),
            track(2, "Layla", "Eric Clapton", Some("Acoustic"), "LOSSLESS"),
        ];
        assert_eq!(pick("Eric Clapton", "Layla (Acoustic)", results), Some(2));
    }

    #[test]
    fn named_remix_request_prefers_the_matching_name() {
        let results = vec![
            track(1, "Song", "Artist", Some("Someone Else Remix"), "LOSSLESS"),
            track(2, "Song", "Artist", Some("L'Tric Remix"), "LOSSLESS"),
            track(3, "Song", "Artist", None, "LOSSLESS"),
        ];
        assert_eq!(pick("Artist", "Song (L'Tric Remix)", results), Some(2));
    }

    #[test]
    fn remaster_is_not_demoted() {
        // Remaster is the same performance; for a clean request with only a
        // remaster and a remix available, the remaster must win.
        let results = vec![
            track(1, "Heroes", "David Bowie", Some("2017 Remaster"), "HI_RES"),
            track(2, "Heroes", "David Bowie", Some("Club Mix"), "LOSSLESS"),
        ];
        assert_eq!(pick("David Bowie", "Heroes", results), Some(1));
    }

    #[test]
    fn live_version_does_not_leak_into_a_clean_request() {
        let results = vec![
            track(
                1,
                "Wish You Were Here",
                "Pink Floyd",
                Some("Live"),
                "LOSSLESS",
            ),
            track(2, "Wish You Were Here", "Pink Floyd", None, "LOSSLESS"),
        ];
        assert_eq!(pick("Pink Floyd", "Wish You Were Here", results), Some(2));
    }

    #[test]
    fn variant_in_title_is_detected_without_a_version_field() {
        let results = vec![
            track(1, "Get Lucky (Radio Edit)", "Daft Punk", None, "LOSSLESS"),
            track(2, "Get Lucky", "Daft Punk", None, "LOSSLESS"),
        ];
        assert_eq!(pick("Daft Punk", "Get Lucky", results), Some(2));
    }

    #[test]
    fn unrecognized_parenthetical_is_treated_as_title_not_variant() {
        let results = vec![track(
            1,
            "Shine On You Crazy Diamond (Pt. 1)",
            "Pink Floyd",
            None,
            "LOSSLESS",
        )];
        assert_eq!(
            pick("Pink Floyd", "Shine On You Crazy Diamond (Pt. 1)", results),
            Some(1)
        );
    }

    #[test]
    fn wrong_artist_below_threshold_is_rejected() {
        let results = vec![track(1, "American Pie", "Madonna", None, "LOSSLESS")];
        assert_eq!(pick("Don McLean", "American Pie", results), None);
    }
}

async fn next_track(
    State(state): State<SharedState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    command_response(&state, transport_command::next(&state).await).await
}

async fn previous_track(
    State(state): State<SharedState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    command_response(&state, transport_command::previous(&state).await).await
}

async fn set_playback_position(
    State(state): State<SharedState>,
    Json(payload): Json<PositionRequest>,
) -> Result<(StatusCode, Json<Value>), StatusCode> {
    match transport_toggle::seek(&state, payload.position_ms, payload.allow_segment_seek)
        .await
        .map_err(toggle_error_status)?
    {
        transport_toggle::SeekResult::Accepted(snapshot) => Ok((
            StatusCode::ACCEPTED,
            Json(json!({ "state": snapshot.state })),
        )),
        transport_toggle::SeekResult::Rejected(snapshot) => Ok((
            StatusCode::CONFLICT,
            Json(json!({ "state": snapshot.state })),
        )),
    }
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
    let update = state_guard
        .db
        .with_conn(|conn| player::set_shuffle_mode(conn, mode))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Ephemeral TIDAL mix rows are real queue rows now, so `set_shuffle_mode`
    // above already reordered them along with the rest of the queue.

    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
    drop(state_guard);
    let snapshot = overlay_snapshot_with_external_track(&state, update.snapshot).await;
    Ok(Json(json!({
        "state": snapshot.state,
        "queue": snapshot.queue,
        "queue_revision": snapshot.queue_revision,
        "shuffle_debug": update.debug
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
            // "Include new" is the one switch for picks from outside the
            // library; allow_external from older clients folds into it
            // (migration 075 merged the two).
            if let Some(dn) = payload.discover_new.or(payload.allow_external) {
                automix::set_automix_discover_new(conn, dn)?;
                automix::set_automix_allow_external(conn, false)?;
            }
            if let Some(use_learning) = payload.use_learning {
                automix::set_automix_use_learning(conn, use_learning)?;
            }
            automix::set_automix_enabled(conn, payload.enabled)
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);

    drop(state_guard);
    let snapshot = overlay_snapshot_with_external_track(&state, snapshot).await;
    Ok(Json(json!({
        "state": snapshot.state,
        "queue": snapshot.queue,
        "queue_revision": snapshot.queue_revision
    })))
}

async fn status() -> Json<Value> {
    Json(json!({
        "name": "NOOR",
        "version": env!("CARGO_PKG_VERSION"),
        "status": "running"
    }))
}

// --- TIDAL Endpoints --------------------------------------

// --- TIDAL Search -------------------------------------------------------------

// --- TIDAL Playlist Search + Tracks -------------------------------------------

async fn current_playback_snapshot_json(
    state: &SharedState,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let snapshot = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(player::load_snapshot)
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "status": "playback_state_load_failed",
                        "message": "Failed to load the current playback state.",
                    })),
                )
            })?
    };
    let snapshot = overlay_snapshot_with_external_track(state, snapshot).await;
    Ok(Json(json!({
        "state": snapshot.state,
        "queue": snapshot.queue,
        "queue_revision": snapshot.queue_revision,
    })))
}

pub(crate) async fn ensure_playback_runtime_for_track(
    state: &SharedState,
    track: &crate::db::models::Track,
) -> Result<playback_runtime::PlaybackRuntimeHandle, (StatusCode, Json<Value>)> {
    super::transport::runtime::ensure_for_track(state)
        .await
        .map_err(|error| runtime_unavailable_response(error, track.id))
}

fn runtime_unavailable_response(
    error: RuntimeUnavailable,
    track_id: i64,
) -> (StatusCode, Json<Value>) {
    match error {
        RuntimeUnavailable::NotConnected => (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "status": "not_connected",
                "message": "Connect TIDAL in Settings before playing.",
                "track_id": track_id,
            })),
        ),
        RuntimeUnavailable::SpawnFailed(message) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({
                "status": "playback_runtime_unavailable",
                "message": message,
                "track_id": track_id,
            })),
        ),
        RuntimeUnavailable::Missing => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "status": "playback_runtime_unavailable",
                "message": "Playback runtime was not available after initialization.",
                "track_id": track_id,
            })),
        ),
    }
}

pub(crate) enum NearEndPreparationOutcome {
    Idle,
    Attempted,
    SkippedUnavailable,
}

/// Remove only the captured upcoming queue row. A late network failure must
/// never remove a replacement row, a healed TIDAL id, or the active track.
pub(crate) fn remove_unavailable_upcoming_row(
    state: &mut crate::AppState,
    current_track_id: i64,
    generation: u64,
    expected_pair: &crate::playback::dj_lookahead::DjLookaheadPair,
    next: &crate::db::models::Track,
    reason: &str,
) -> anyhow::Result<bool> {
    if current_playback_generation(state) != generation
        || state
            .playback_runtime_info
            .as_ref()
            .and_then(|info| info.active_track_id)
            != Some(current_track_id)
        || expected_pair.current_queue_item_id.is_none()
        || expected_pair.next_queue_item_id.is_none()
        || expected_pair.current_queue_item_id == expected_pair.next_queue_item_id
        || next.id == current_track_id
    {
        return Ok(false);
    }
    let cleared = recently_cleared(state);
    let removed = state.db.with_conn(|conn| {
        if player::current_track_id(conn)? != Some(current_track_id) {
            return Ok(false);
        }
        let still_next = player::peek_next_track(conn, cleared)?;
        let pair = crate::playback::dj_lookahead::load_dj_lookahead_pair(conn)?;
        if pair != *expected_pair
            || pair
                .next
                .as_ref()
                .and_then(|media_ref| media_ref.track_id())
                != Some(next.id)
            || !still_next
                .is_some_and(|track| track.id == next.id && track.tidal_id == next.tidal_id)
        {
            return Ok(false);
        }
        let outcome = player::remove_queue_item_and_reconcile(
            conn,
            expected_pair
                .next_queue_item_id
                .expect("validated upcoming row"),
        )?;
        debug_assert!(!outcome.removed_current);
        Ok(true)
    })?;
    if removed {
        state.pending_stream_display = None;
        let _ = state.event_tx.send(AppEvent::TrackSkipped {
            track_id: next.id,
            title: next.title.clone(),
            reason: reason.to_string(),
        });
        let _ = state.event_tx.send(AppEvent::QueueUpdated);
        tracing::info!(target: "noor.playback.advance", event = "skip_unavailable_next",
            track_id = next.id, queue_item_id = expected_pair.next_queue_item_id,
            generation, "Skipped unavailable upcoming track; current playback continues");
    }
    Ok(removed)
}

pub(crate) const RUNTIME_TRACK_RETRY_MARKER: &str = "retrying transient playback failure";

pub(crate) fn sqlite_database_locked(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        if let Some(rusqlite::Error::SqliteFailure(sqlite_error, _)) =
            cause.downcast_ref::<rusqlite::Error>()
        {
            return matches!(
                sqlite_error.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            );
        }
        let message = cause.to_string();
        message.contains("database is locked") || message.contains("database table is locked")
    })
}

/// Returns `Some(new_tidal_id)` when the row was healed with a fresh, verified
/// id; `None` when no better id was found. Errors only on infrastructure
/// failures (DB, no TIDAL session), never on "couldn't find a match".
pub(crate) async fn reresolve_tidal_id(
    state: &SharedState,
    track_id: i64,
) -> anyhow::Result<Option<i64>> {
    let db = {
        let s = state.read().await;
        s.db.clone()
    };
    let row = db.with_conn(|conn| {
        let found = conn
            .query_row(
                "SELECT t.tidal_id, t.title, a.name
                 FROM tracks t JOIN artists a ON a.id = t.artist_id
                 WHERE t.id = ?1",
                [track_id],
                |r| {
                    Ok((
                        r.get::<_, Option<i64>>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        Ok(found)
    })?;
    let Some((old_tidal_id, title, artist)) = row else {
        return Ok(None);
    };

    let (tokens, http) = {
        let persisted = load_persisted_tidal_tokens(state).await.ok().flatten();
        let s = state.read().await;
        match s.tidal.tokens().or(persisted) {
            Some(tokens) => (tokens, s.tidal_http_client.clone()),
            None => return Ok(None),
        }
    };

    let client = TidalClient::for_session(state.read().await.tidal.clone(), &tokens.country_code)
        .with_metadata_store(state.read().await.db.clone());
    let query = format!("{artist} {title}");
    let results = client.search(&query, TIDAL_RESOLVE_POOL).await?;
    let Some((_score, candidate)) = select_best_tidal_match(&artist, &title, results) else {
        return Ok(None);
    };
    // Same id that just failed, or a match we can't distinguish: nothing to heal.
    if Some(candidate.id) == old_tidal_id {
        return Ok(None);
    }

    // Don't swap one dead id for another: confirm the candidate actually streams
    // before rewriting the row.
    let request = tidal_stream::StreamRequest::new(candidate.id, "LOSSLESS");
    if tidal_stream::resolve_stream(&http, &tokens.access_token, &request)
        .await
        .is_err()
    {
        return Ok(None);
    }

    let full = client.get_track(candidate.id).await?;
    let verified = db.with_conn(|conn| {
        let incoming = crate::library::duplicates::IncomingTrack {
            tidal_id: full.id,
            title: &full.title,
            artist_name: &full.artist.name,
            isrc: full.isrc.as_deref(),
            duration_ms: full.duration * 1000,
            version: full.extra.get("version").and_then(|v| v.as_str()),
            explicit: full.extra.get("explicit").and_then(|v| v.as_bool()),
        };
        let candidates = crate::library::duplicates::fetch_import_candidates(
            conn,
            full.id,
            full.artist.id,
            full.isrc.as_deref(),
            incoming.duration_ms,
        )?;
        let same = candidates
            .into_iter()
            .filter(|c| c.track_id == track_id)
            .collect::<Vec<_>>();
        Ok(matches!(
            crate::library::duplicates::decide_import(&incoming, &same),
            crate::library::duplicates::ImportDecision::LinkAlias { .. }
        ))
    })?;
    if !verified {
        return Ok(None);
    }
    // A transient asset failure does not prove that a catalogue release was withdrawn.
    let unavailable = if let Some(old) = old_tidal_id {
        match client.get_track(old).await {
            Ok(track) => {
                track.stream_ready == Some(false)
                    || track.extra.get("allowStreaming").and_then(|v| v.as_bool()) == Some(false)
            }
            Err(error) => error
                .to_string()
                .to_lowercase()
                .starts_with("tidal api error 404 "),
        }
    } else {
        false
    };
    let new_id = full.id;
    let updated = db.with_conn(|conn| {
        let tx = conn.unchecked_transaction()?;
        if crate::db::catalogue::track_id(&tx, new_id)?.is_some_and(|id| id != track_id) {
            return Ok(false);
        }
        crate::db::catalogue::record_track(&tx, track_id, &full, false, None)?;
        crate::db::catalogue::observe(&tx, new_id, "available", "stream_check")?;
        if unavailable && let Some(old) = old_tidal_id {
            crate::db::catalogue::observe(&tx, old, "unavailable", "metadata_check")?;
        }
        let selected: Option<i64> =
            tx.query_row("SELECT tidal_id FROM tracks WHERE id=?1", [track_id], |r| {
                r.get(0)
            })?;
        tx.commit()?;
        Ok(selected == Some(new_id))
    })?;

    Ok(updated.then_some(new_id))
}

pub(crate) fn effective_crossfade_for_exclusive(
    exclusive: bool,
    dj_engine_enabled: bool,
    configured: i32,
) -> i32 {
    if exclusive && !dj_engine_enabled {
        0
    } else {
        configured.max(0)
    }
}

// ----- Audio output settings ------------------------------------------------
//
// `GET /api/audio/devices`: enumerate cpal output devices
// `GET /api/audio/settings`: current persisted AudioSettings
// `PUT /api/audio/settings`: persist and live-swap when output settings change
// `POST /api/audio/exclusive/retry`: force a fresh DeviceSwap to retry exclusive grab

// Returns `[]` in steady state: the Tidal v1 endpoints we use don't expose
// genre fields. Kept for free in case Tidal adds them later. See
// docs/tidal-genre-source-investigation.md (2026-04-30).

// --- TIDAL: Your Mixes -------------------------------------------------------

async fn scrobbling_backfill(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let listens = {
        let s = state.read().await;
        s.db.with_conn(|conn| crate::services::scrobbling::recent_eligible_listens(conn, 30))
            .map_err(|error| {
                warn!("Failed to load backfill listens: {error:#}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?
    };
    let eligible = listens.len();
    let provider_count = crate::services::scrobbling::enabled_provider_count(&state).await;
    let mut queued = 0usize;
    if provider_count > 0 {
        for payload in listens {
            queued += crate::services::scrobbling::enqueue_backfill(state.clone(), payload).await;
        }
    }
    let status = if queued > 0 {
        "queued"
    } else if provider_count > 0 {
        "up_to_date"
    } else if eligible > 0 {
        "not_ready"
    } else {
        "empty"
    };
    Ok(Json(json!({
        "status": status,
        "days": 30,
        "eligible": eligible,
        "providers": provider_count,
        "queued": queued
    })))
}

// -- Spotify Config & Enrichment ----------------------------------------------

#[cfg(test)]
pub(super) mod tests;

#[cfg(test)]
mod drop_preview_tests;
