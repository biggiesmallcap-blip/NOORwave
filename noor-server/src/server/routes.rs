use crate::db::queries;
use crate::metadata::discogs::DiscogsClient;
use crate::metadata::lastfm::LastFmClient;
use crate::playback::{automix, pending, player, queue, runtime as playback_runtime};
use crate::services::discovery::{
    DiscoveryCandidateSeed, DiscoveryProvider, TidalDiscoveryProvider,
};
use crate::services::learning as discovery_learning;
use crate::services::tidal::{
    auth as tidal_auth,
    client::{TidalClient, TidalSearchCatalog, TidalSearchTrack, TidalSearchVideo, TidalTrack},
    import as tidal_import, mutations as tidal_mutations, stream as tidal_stream,
};
use crate::smart::external_discovery as external_discovery_engine;
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
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

mod analytics_routes;
mod audio_analysis_routes;
pub(crate) mod catalog_routes;
mod chart_routes;
mod discovery_routes;
mod discovery_space_routes;
mod dj_routes;
mod download_routes;
mod duplicates_routes;
mod enrichment_routes;
mod genre_routes;
pub(crate) mod home_routes;
mod library_batch_routes;
mod playlist_routes;
mod search_routes;
mod sportify_routes;
mod tidal_home_routes;
mod tidal_sync_routes;
pub use tidal_sync_routes::trigger_auto_sync;

type TidalPlaylistTracksCache = Arc<Mutex<HashMap<String, (Instant, Vec<TidalTrack>)>>>;
type DropPreviewArmKey = (i64, i64, u64);

const TIDAL_PLAYLIST_TRACKS_CACHE_TTL: Duration = Duration::from_secs(60 * 60);
const EPHEMERAL_DJ_LOOKAHEAD_DEADLINE_SAMPLES: u64 = 48_000 * 30;
const DROP_PREVIEW_DURATION_MS: u32 = 16_000;
const DROP_PREVIEW_ARM_RETRY_SECS: u64 = 60 * 60;
const PLAYBACK_FINISH_DB_LOCK_RETRY_LIMIT: usize = 60;
const PLAYBACK_FINISH_DB_LOCK_RETRY_DELAY_SECS: u64 = 2;
const PLAYBACK_ADVANCE_PENDING_SKIP_LIMIT: usize = 8;
const PLAYBACK_PENDING_BUSY_RETRY_LIMIT: usize = 5;
const PLAYBACK_PENDING_BUSY_RETRY_DELAY_MS: u64 = 200;
const TIDAL_SEARCH_UPSTREAM_TIMEOUT_SECS: u64 = 8;

static DROP_PREVIEW_ARM_ATTEMPTS: OnceLock<Mutex<HashMap<DropPreviewArmKey, Instant>>> =
    OnceLock::new();

async fn queue_missing_dj_profiles_after_pair_change(state: SharedState, context: &'static str) {
    if let Err(status) = dj_routes::queue_missing_dj_profiles_for_current_pair(state).await {
        warn!(
            ?status,
            context, "DJ profile queueing failed after pair change"
        );
    }
}

fn active_dj_lookahead_start_for_state(
    state: &crate::AppState,
) -> Option<player::DjLookaheadStart> {
    state
        .db
        .with_conn(|conn| {
            if !queries::is_dj_engine_enabled(conn)? {
                return Ok(None);
            }
            let pair = active_dj_pair_for_state_and_conn(state, conn)?;
            Ok(player::dj_lookahead_start_from_pair(
                pair,
                EPHEMERAL_DJ_LOOKAHEAD_DEADLINE_SAMPLES,
            ))
        })
        .ok()
        .flatten()
}

async fn start_dj_lookahead_and_queue_profiles_after_pair_change(
    state: SharedState,
    handle: playback_runtime::PlaybackRuntimeHandle,
    context: &'static str,
) {
    let lookahead = {
        let state_guard = state.read().await;
        active_dj_lookahead_start_for_state(&state_guard)
    };
    if let Some(lookahead) = lookahead {
        let _ = lookahead.dispatch(&handle);
        spawn_drop_preview_scheduler(state.clone(), handle, lookahead);
    }
    queue_missing_dj_profiles_after_pair_change(state, context).await;
}

fn spawn_drop_preview_scheduler(
    state: SharedState,
    handle: playback_runtime::PlaybackRuntimeHandle,
    lookahead: player::DjLookaheadStart,
) {
    tokio::spawn(async move {
        if let Err(error) = schedule_drop_preview_for_pair(state, handle, lookahead).await {
            warn!("Drop preview scheduling skipped: {error:?}");
        }
    });
}

async fn schedule_drop_preview_for_pair(
    state: SharedState,
    handle: playback_runtime::PlaybackRuntimeHandle,
    lookahead: player::DjLookaheadStart,
) -> anyhow::Result<()> {
    let Some(current_ref) = lookahead.current.clone() else {
        return Ok(());
    };
    let Some(next_ref) = lookahead.next.clone() else {
        return Ok(());
    };
    let (Some(current_track_id), Some(next_track_id)) =
        (current_ref.track_id(), next_ref.track_id())
    else {
        return Ok(());
    };
    if !claim_drop_preview_arm(current_track_id, next_track_id, lookahead.queue_generation) {
        return Ok(());
    }

    let (next, runtime_info, user_quality) = {
        let state_guard = state.read().await;
        let active_id = state_guard
            .playback_runtime_info
            .as_ref()
            .and_then(|info| info.active_track_id);
        if !state_guard.db.with_conn(queries::is_dj_engine_enabled)? {
            return Ok(());
        }
        if active_id != Some(current_track_id)
            || current_playback_generation(&state_guard) != lookahead.queue_generation
        {
            return Ok(());
        }
        let current = state_guard
            .db
            .with_conn(|conn| queue::get_track_by_id(conn, current_track_id))?
            .context("drop preview current track missing")?;
        let next = state_guard
            .db
            .with_conn(|conn| queue::get_track_by_id(conn, next_track_id))?
            .context("drop preview next track missing")?;
        let plan = state_guard.db.with_conn(|conn| {
            dj_routes::drop_preview_plan_for_pair(
                conn,
                &current_ref,
                &next_ref,
                current.duration_ms,
            )
        })?;
        let Some(plan) = plan else {
            return Ok(());
        };
        let info = state_guard
            .playback_runtime_info
            .clone()
            .context("playback runtime info missing")?;
        let position_ms = handle.get_position_ms(info.sample_rate, info.channels);
        if position_ms >= plan.planned_fire_ms {
            return Ok(());
        }
        (
            next,
            (info.sample_rate, info.channels, plan),
            current_user_audio_quality_locked(&state_guard),
        )
    };

    let stream_request = match player::build_tidal_stream_request(&next, user_quality.clone()) {
        Some(request) => request,
        None => return Ok(()),
    };
    let stream_info = match resolve_tidal_playback_stream(&state, &next, &stream_request).await {
        Ok(info) => Some(info),
        Err(error) => {
            warn!(
                "Skipping drop preview for next track {}: {}",
                next.id,
                describe_tidal_playback_error(&error)
            );
            return Ok(());
        }
    };

    let (sample_rate, channels, plan) = runtime_info;
    let engine = {
        let state_guard = state.read().await;
        crate::playback::dj_engine::DjEngine::new(state_guard.db.clone())
    };
    let Some(program) = engine.plan_drop_preview(
        &current_ref,
        &next_ref,
        stream_info
            .as_ref()
            .and_then(|info| info.sample_rate_hz())
            .unwrap_or(sample_rate),
        channels,
        DROP_PREVIEW_DURATION_MS,
    )?
    else {
        return Ok(());
    };

    let mut job = player::build_playback_preparation(&next, stream_info.as_ref(), 0, user_quality)
        .with_generation(lookahead.queue_generation)
        .with_dj_media_ref(next_ref.clone())
        .with_prepared_transition(player::PreparedTransitionProgram {
            program,
            transition_event_id: None,
            fire_ahead_ms: 0,
            queue_generation: lookahead.queue_generation,
            current_queue_item_id: lookahead.current_queue_item_id,
            next_queue_item_id: lookahead.next_queue_item_id,
        });
    job.gapless = crate::playback::gapless::GaplessPlan::disabled();

    {
        let state_guard = state.read().await;
        let active_id = state_guard
            .playback_runtime_info
            .as_ref()
            .and_then(|info| info.active_track_id);
        if !state_guard.db.with_conn(queries::is_dj_engine_enabled)? {
            return Ok(());
        }
        if active_id != Some(current_track_id)
            || current_playback_generation(&state_guard) != lookahead.queue_generation
        {
            return Ok(());
        }
        let info = state_guard
            .playback_runtime_info
            .as_ref()
            .context("playback runtime info missing")?;
        let position_ms = handle.get_position_ms(info.sample_rate, info.channels);
        if position_ms >= plan.planned_fire_ms {
            return Ok(());
        }
    }

    let trigger_position_samples =
        samples_from_ms_for_runtime(plan.planned_fire_ms, sample_rate, channels);
    handle.prepare_drop_preview(job)?;
    handle.arm_drop_preview(
        current_track_id,
        lookahead.queue_generation,
        trigger_position_samples,
    )?;
    info!(
        current_track_id,
        next_track_id = next.id,
        planned_fire_ms = plan.planned_fire_ms,
        incoming_drop_ms = plan.incoming_drop_ms,
        source = %plan.source,
        "Drop preview armed"
    );
    Ok(())
}

fn current_user_audio_quality_locked(
    state: &crate::AppState,
) -> Option<crate::db::audio_settings::AudioQuality> {
    state
        .db
        .with_conn(|conn| crate::db::audio_settings::load(conn).map_err(anyhow::Error::from))
        .ok()
        .map(|settings| settings.quality)
}

fn samples_from_ms_for_runtime(ms: i64, sample_rate: u32, channels: u16) -> u64 {
    let ms = ms.max(0) as u64;
    ms.saturating_mul(sample_rate.max(1) as u64)
        .saturating_mul(channels.max(1) as u64)
        / 1000
}

fn claim_drop_preview_arm(current_track_id: i64, next_track_id: i64, generation: u64) -> bool {
    let attempts = DROP_PREVIEW_ARM_ATTEMPTS.get_or_init(|| Mutex::new(HashMap::new()));
    let now = Instant::now();
    let mut attempts = attempts.lock().unwrap_or_else(|error| error.into_inner());
    claim_drop_preview_arm_at(
        &mut attempts,
        current_track_id,
        next_track_id,
        generation,
        now,
    )
}

fn claim_drop_preview_arm_at(
    attempts: &mut HashMap<DropPreviewArmKey, Instant>,
    current_track_id: i64,
    next_track_id: i64,
    generation: u64,
    now: Instant,
) -> bool {
    let retry_after = Duration::from_secs(DROP_PREVIEW_ARM_RETRY_SECS);
    attempts.retain(|_, last_attempt| now.duration_since(*last_attempt) < retry_after);
    let key = (current_track_id, next_track_id, generation);
    if let Some(last_attempt) = attempts.get(&key)
        && now.duration_since(*last_attempt) < retry_after
    {
        return false;
    }
    attempts.insert(key, now);
    true
}

async fn refresh_dj_after_queue_change(state: SharedState, context: &'static str) {
    let runtime = {
        let state_guard = state.read().await;
        state_guard
            .playback_runtime
            .as_ref()
            .map(|runtime| runtime.handle.clone())
    };
    if let Some(runtime) = runtime {
        start_dj_lookahead_and_queue_profiles_after_pair_change(state, runtime, context).await;
    } else {
        queue_missing_dj_profiles_after_pair_change(state, context).await;
    }
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
    #[serde(default)]
    shuffle_mode: Option<String>,
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
    // Display + identity metadata, used only when folding a TIDAL pick into a live
    // mix as an ephemeral row so the queued row renders with art/album/duration and
    // keeps clickable artist/album links. Ignored on the library/pending paths.
    #[serde(default)]
    album_title: Option<String>,
    #[serde(default)]
    artwork_url: Option<String>,
    #[serde(default)]
    duration_ms: Option<i64>,
    #[serde(default)]
    artist_tidal_id: Option<i64>,
    #[serde(default)]
    album_tidal_id: Option<i64>,
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

#[derive(Debug)]
enum PlaylistFromQueueSource {
    Local(i64),
    Tidal(tidal_import::ImportTrackMetadata),
}

#[derive(Debug, Deserialize)]
pub struct TrackFavoriteRequest {
    track_id: i64,
    favorite: bool,
}

#[derive(Debug, Deserialize)]
pub struct AlbumFavoriteRequest {
    album_id: i64,
    favorite: bool,
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
        // Library endpoints
        .route("/api/tracks", get(catalog_routes::get_tracks))
        .route("/api/tracks/count", get(catalog_routes::get_track_count))
        .route("/api/history", get(catalog_routes::get_history))
        .route("/api/albums", get(catalog_routes::get_albums))
        .route(
            "/api/albums/{id}/tracks",
            get(catalog_routes::get_album_tracks),
        )
        .route(
            "/api/albums/{id}/spotify-stats",
            get(catalog_routes::get_album_spotify_stats),
        )
        .route("/api/artists", get(catalog_routes::get_artists))
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
        .route("/api/playlists", get(playlist_routes::get_playlists))
        .route(
            "/api/playlists/{id}/tracks",
            get(playlist_routes::get_playlist_tracks)
                .post(playlist_routes::add_tracks_to_playlist_route),
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
        // Sportify-based discovery resolver - single, bulk, and cache-only status poll.
        .route("/api/resolve/tidal/track", get(resolve_tidal_track))
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
        .route("/api/tidal/playlists/search", get(tidal_playlist_search))
        .route(
            "/api/tidal/playlists/{uuid}/tracks",
            get(tidal_playlist_tracks),
        )
        .route("/api/tidal/play", post(play_tidal_ephemeral))
        .route("/api/tidal/artists/{tidal_id}", get(tidal_artist_profile))
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
        // Status
        .route("/api/status", get(status))
        // Home page discovery endpoints
        .route("/api/home/releases", get(home_routes::get_home_releases))
        .route("/api/home/picks", get(home_routes::get_home_picks))
        .route(
            "/api/home/recommendations",
            get(home_routes::get_home_recommendations),
        )
        .route("/api/home/articles", get(home_routes::get_home_articles))
        .route("/api/home/news", get(home_routes::get_home_news))
        // TIDAL "Your Mixes" - drives the home Your Mixes shelf above Trending.
        .route("/api/tidal/mixes", get(tidal_home_routes::get_tidal_mixes))
        .route(
            "/api/tidal/mixes/{id}/tracks",
            get(tidal_home_routes::get_tidal_mix_tracks),
        )
        .route("/api/tidal/play-mix", post(play_tidal_mix))
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

    let port = crate::server::noor_port();
    let bind_address = if host_mode {
        format!("0.0.0.0:{port}")
    } else {
        format!("127.0.0.1:{port}")
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

    let port = crate::server::noor_port();
    let bind_address = if host_mode {
        format!("0.0.0.0:{port}")
    } else {
        format!("127.0.0.1:{port}")
    };
    Ok(Json(
        json!({ "host_mode": host_mode, "bind_address": bind_address }),
    ))
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
    let user_quality = current_user_audio_quality(&state).await;
    let stream_request = tidal_stream::StreamRequest::new(
        parse_provider_track_id(&payload.provider_track_id)?,
        requested_tidal_quality(user_quality.clone(), payload.audio_quality.as_deref()),
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
    let job =
        player::build_playback_preparation(&track, Some(&stream_info), crossfade_ms, user_quality)
            .with_generation(playback_generation);
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
        // DSP harmonic post-scoring - apply the shared harmonic multiplier to
        // every row that has audio features on both sides. Rows without
        // features are left untouched (never penalised for being unanalyzed).
        let seed_features = state
            .db
            .with_conn(|conn| queries::get_audio_dsp_features(conn, seed_track_id))
            .ok()
            .flatten();

        if let Some(seed) = seed_features.as_ref() {
            // Batch the candidate harmonic-key lookups into one query instead of a
            // serialized per-candidate round trip under the DB mutex.
            let cand_ids: Vec<i64> = rows.iter().map(|r| r.track_id).collect();
            let cand_keys = state
                .db
                .with_conn(|conn| queries::get_dsp_harmonic_keys_batch(conn, &cand_ids))
                .unwrap_or_default();
            for row in rows.iter_mut() {
                if let Some((cand_camelot, cand_bpm)) = cand_keys.get(&row.track_id) {
                    let mult = crate::services::audio_analysis::compute_harmonic_multiplier(
                        seed.camelot_key.as_deref(),
                        cand_camelot.as_deref(),
                        seed.bpm,
                        *cand_bpm,
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
            .with_conn(queries::get_selected_discovery_embedding_model)
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
    let (db, event_tx, running, busy) = {
        let s = state.read().await;
        let busy = crate::services::radio_similarity::busy_reason(&s, &s.db);
        (
            s.db.clone(),
            s.event_tx.clone(),
            s.radio_similarity_running.clone(),
            busy,
        )
    };

    // A rebuild owns SQLite's single writer slot for minutes. The manual route
    // gates on the same idle check as the auto path â€” clicking the button does
    // not justify failing an in-flight sync or listen-history write.
    if let Some(reason) = busy {
        return Ok(Json(json!({
            "status": "busy",
            "message": format!("Can't rebuild while {reason} is active. Try again once it's finished.")
        })));
    }

    // Shared single-flight + isolated-connection rebuild path: a manual click
    // and an auto-rebuild can never run the multi-minute job twice, and the
    // job never holds the shared connection mutex.
    if crate::services::radio_similarity::try_spawn_rebuild(db, event_tx, running) {
        Ok(Json(json!({
            "status": "computation_started",
            "message": "Similarity computation running in background. This may take a few minutes for large libraries."
        })))
    } else {
        Ok(Json(json!({
            "status": "already_running",
            "message": "Similarity computation is already in progress."
        })))
    }
}

/// Status of the radio similarity index: row count + last-built timestamp.
/// Powers the Settings "Build radio similarity index" panel â€” the frontend
/// polls this after triggering a compute to detect completion. `built_at`
/// comes from `server_config`, not the table's rows, so a legitimate zero-row
/// rebuild still reads as built.
async fn radio_similarity_status(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let db = {
        let s = state.read().await;
        s.db.clone()
    };
    let row_count = db.with_conn(queries::count_track_similarity).unwrap_or(0);
    let built_at = db
        .with_conn(queries::get_radio_similarity_built_at)
        .ok()
        .flatten();
    Ok(Json(json!({
        "row_count": row_count,
        "built_at": built_at,
    })))
}

// --- Discovery Sound Space -----------------------------------------------

#[derive(Debug, Deserialize)]
struct ResolveTidalTrackQuery {
    spotify_id: String,
    /// When true, ignore any cached resolution and re-run the matcher.
    #[serde(default)]
    refresh: bool,
}

/// Resolve one Spotify (Sportify) track to TIDAL for playback. Reads the
/// Spotify->TIDAL map cache first; on miss, fetches Sportify metadata and
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

// --- Sportify bulk + status resolution endpoints ------------

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
    // -> upstream call; failures fall through as `unresolved` rather than
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

// --- Sportify discovery read endpoints ----------------------

/// Resolve the first `eager_n` Spotify tracks against TIDAL inline (so the
/// top of the response is instantly playable) and spawn a background task
/// for the remainder. Both paths persist into `sportify_track_map` /
/// `sportify_unresolved`, so a follow-up `enrich_tracks_with_tidal_cache`
/// call reflects the inline resolutions in the response.
///
/// Returns the list of spotify_ids spawned for lazy resolution - surfaced in
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
    // so the UI shows them as such - better than persisting bogus failures.
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
            for batch in background_resolution_batches(&lazy_owned, concurrency) {
                let outcomes = resolver::resolve_many(&client, &batch, concurrency).await;
                let _ = db_lazy.with_conn(|conn| {
                    persist_outcomes(conn, &outcomes);
                    Ok::<_, anyhow::Error>(())
                });
            }
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
        let concurrency = resolve_cfg.bulk_concurrency;
        for batch in background_resolution_batches(&needs_resolve, concurrency) {
            let outcomes = resolver::resolve_many(&client, &batch, concurrency).await;
            let _ = db.with_conn(|conn| {
                persist_outcomes(conn, &outcomes);
                Ok::<_, anyhow::Error>(())
            });
        }
    });

    pending_ids
}

fn background_resolution_batches(
    tracks: &[(String, crate::services::sportify::models::SportifyTrack)],
    max_batch_size: usize,
) -> Vec<Vec<(String, crate::services::sportify::models::SportifyTrack)>> {
    tracks
        .chunks(max_batch_size.max(1))
        .map(|chunk| chunk.to_vec())
        .collect()
}

fn persist_outcomes(
    conn: &rusqlite::Connection,
    outcomes: &[(
        String,
        crate::services::sportify::resolver::ResolutionOutcome,
    )],
) {
    for (id, outcome) in outcomes {
        persist_outcome(conn, id, outcome);
    }
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

pub(super) fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": e.to_string() })),
    )
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

    let (db, lastfm, lastfm_similar_cache) = {
        let g = state.read().await;
        g.user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let lastfm = crate::metadata::lastfm::LastFmClient::load(g.http_client.clone(), &g.db);
        (g.db.clone(), lastfm, g.lastfm_similar_cache.clone())
    };

    let queue = crate::services::radio::orchestrate_song(
        &db,
        lastfm.as_ref(),
        Some(&lastfm_similar_cache),
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

// --- POST /api/radio/start ---------------------------------------------------
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

pub(super) async fn build_radio_queue_and_spawn_resolvers(
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

    spawn_pending_resolvers_for_queue_items(state, db, pending_item_ids, context).await;

    {
        let s = state.read().await;
        let _ = s.event_tx.send(AppEvent::QueueUpdated);
    }

    Ok((first_playable, pending_count))
}

pub(super) async fn spawn_pending_resolvers_for_queue_items(
    state: &SharedState,
    db: &crate::db::Database,
    pending_item_ids: Vec<i64>,
    context: &'static str,
) {
    if pending_item_ids.is_empty() {
        return;
    }

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
            let state_bg = state.clone();
            tokio::spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                if resolve_pending_row(db_bg, tok, item_id, tx, http).await {
                    refresh_dj_after_queue_change(state_bg, context).await;
                }
            });
        }
    } else {
        tracing::warn!(
            "{context}: Tidal tokens unavailable - pending rows will rely on lazy resolution"
        );
    }
}

pub(super) async fn start_first_radio_queue_item(
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

    snapshot = resolve_or_skip_pending_current(
        state,
        snapshot,
        playback_generation,
        "start_first_radio_queue_item",
    )
    .await
    .map_err(|error| {
        tracing::error!(
            target: "noor.playback.advance",
            event = "radio_start_pending_advance_failed",
            error = %error,
            "failed to resolve or skip first radio queue item"
        );
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "status": "playback_state_update_failed",
                "message": "Failed to start the radio queue.",
            })),
        )
    })?;

    let play_track = snapshot.state.current_track.clone();

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

    let (db, lastfm, lastfm_similar_cache) = {
        let g = state.read().await;
        // User-driven radio start; reset post-clear suppression so the
        // freshly-built queue gets normal automix gating downstream.
        g.user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let lastfm = crate::metadata::lastfm::LastFmClient::load(g.http_client.clone(), &g.db);
        (g.db.clone(), lastfm, g.lastfm_similar_cache.clone())
    };

    let radio_queue = crate::services::radio::orchestrate_song(
        &db,
        lastfm.as_ref(),
        Some(&lastfm_similar_cache),
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
                let state_bg = state.clone();
                tokio::spawn(async move {
                    let _permit = sem.acquire_owned().await.ok();
                    if resolve_pending_row(db_bg, tok, item_id, tx, http).await {
                        refresh_dj_after_queue_change(state_bg, "radio_start_pending_resolved")
                            .await;
                    }
                });
            }
        } else {
            tracing::warn!(
                "radio_start: Tidal tokens unavailable - pending rows will rely on lazy resolution"
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

    let (db, lastfm, lastfm_similar_cache) = {
        let g = state.read().await;
        g.user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let lastfm = crate::metadata::lastfm::LastFmClient::load(g.http_client.clone(), &g.db);
        (g.db.clone(), lastfm, g.lastfm_similar_cache.clone())
    };

    let queue = crate::services::radio::orchestrate_album(
        &db,
        lastfm.as_ref(),
        Some(&lastfm_similar_cache),
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

    let (db, lastfm, lastfm_similar_cache) = {
        let g = state.read().await;
        g.user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let lastfm = crate::metadata::lastfm::LastFmClient::load(g.http_client.clone(), &g.db);
        (g.db.clone(), lastfm, g.lastfm_similar_cache.clone())
    };

    let queue = crate::services::radio::orchestrate_artist(
        &db,
        lastfm.as_ref(),
        Some(&lastfm_similar_cache),
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

pub(super) fn normalize_discovery_mode(mode: Option<&str>) -> String {
    match mode.unwrap_or("mood").trim() {
        "reference" => "reference".to_string(),
        "dj" => "dj".to_string(),
        "word-cloud" => "word-cloud".to_string(),
        _ => "mood".to_string(),
    }
}

pub(super) fn normalize_discovery_services(services: Option<Vec<String>>) -> Vec<String> {
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

pub(super) fn normalize_external_provider(provider: &str) -> Option<&'static str> {
    match provider.trim().to_ascii_lowercase().as_str() {
        "tidal" => Some("tidal"),
        "soundcloud" => Some("soundcloud"),
        "bandcamp" => Some("bandcamp"),
        "ytmusic" => Some("ytmusic"),
        _ => None,
    }
}

pub(super) fn internal_discovery_error(error: anyhow::Error) -> (StatusCode, Json<Value>) {
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

pub(super) fn discovery_upstream_error(error: anyhow::Error) -> (StatusCode, Json<Value>) {
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

pub(super) async fn tidal_discovery_provider(
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

                // Link the new track to an album row when we have a stable
                // TIDAL album id. Without an id we previously inserted a stub
                // row keyed on (artist_id, LOWER(title)), which collides on a
                // later sync that upserts the real album by tidal_id â€” the
                // result is two album rows for the same release and the
                // discovery-injected track hanging off the orphan stub.
                //
                // The fixed shape: look up by `tidal_id`; if missing, insert
                // with the tidal_id set so the next sync finds it. When the
                // candidate has no album tidal id (unknown source), leave
                // `album_id` NULL â€” the artwork falls back to track.artwork_url
                // via the existing render path and a future sync can attach
                // it cleanly. No more orphan stubs.
                let album_id: Option<i64> = match (
                    candidate.album_tidal_id,
                    candidate.album_title.as_deref(),
                ) {
                    (Some(album_tidal_id), Some(album_title)) if !album_title.is_empty() => {
                        match conn.query_row(
                            "SELECT id FROM albums WHERE tidal_id = ?1",
                            rusqlite::params![album_tidal_id],
                            |row| row.get::<_, i64>(0),
                        ) {
                            Ok(id) => {
                                if let Some(url) = candidate.artwork_url.as_deref() {
                                    let _ = conn.execute(
                                        "UPDATE albums SET artwork_url = ?1
                                         WHERE id = ?2 AND (artwork_url IS NULL OR artwork_url = '')",
                                        rusqlite::params![url, id],
                                    );
                                }
                                Some(id)
                            }
                            Err(_) => {
                                if conn
                                    .execute(
                                        "INSERT INTO albums (tidal_id, title, artist_id, artwork_url, source)
                                         VALUES (?1, ?2, ?3, ?4, 'tidal')
                                         ON CONFLICT(tidal_id) DO NOTHING",
                                        rusqlite::params![
                                            album_tidal_id,
                                            album_title,
                                            artist_id,
                                            candidate.artwork_url.as_deref()
                                        ],
                                    )
                                    .is_ok()
                                {
                                    // The conflict path means another writer just
                                    // inserted the same album â€” fetch its id instead
                                    // of relying on last_insert_rowid (which would be
                                    // 0 on conflict).
                                    conn.query_row(
                                        "SELECT id FROM albums WHERE tidal_id = ?1",
                                        rusqlite::params![album_tidal_id],
                                        |row| row.get::<_, i64>(0),
                                    )
                                    .ok()
                                } else {
                                    None
                                }
                            }
                        }
                    }
                    _ => None,
                };

                if conn
                    .execute(
                        // Transient discovery injection: is_library stays 0 so
                        // these never surface in the library, even when they
                        // attach to a favorited album by tidal_id. See MIGRATION_052.
                        "INSERT INTO tracks (tidal_id, title, artist_id, album_id, duration_ms, best_quality, best_source, fidelity_score, is_favorite, source, is_library)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'tidal', ?7, 0, 'tidal', 0)
                         ON CONFLICT(tidal_id) DO NOTHING",
                        rusqlite::params![
                            tidal_id, candidate.title, artist_id, album_id,
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

pub(super) async fn load_external_discovery_context(
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

pub(super) async fn existing_candidate_tidal_ids(
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

pub(super) fn discovery_provider_capabilities()
-> Vec<crate::db::models::DiscoveryProviderCapability> {
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

pub(super) async fn augment_connection_queries_with_lastfm(
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

pub(super) async fn augment_search_queries_with_lastfm(
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

pub(super) async fn enrich_candidates_with_metadata(
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
        artist_tidal_id: None,
        album_tidal_id: None,
        // Discovery play carries no artist/album tidal ids yet; links fall back to
        // the frontend metadata cache for user-launched discovery plays.
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

pub(super) fn discovery_request_to_trail_item(
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
    state_guard.prepared_ephemeral_tidal_next = None;
    if clear_mix_queue {
        let _ = state_guard.db.with_conn(|conn| {
            Ok::<_, anyhow::Error>(queue::delete_all_ephemeral_tidal_rows(conn)?)
        });
    }
}

// Takes `conn` rather than opening its own `with_conn`: every caller already
// holds the DB lock (inside `active_dj_pair_for_state_and_conn` and the
// rebuild-profile route), and `Mutex<Connection>` is non-reentrant, so a nested
// `with_conn` here deadlocks forever.
pub(crate) fn active_ephemeral_tidal_mix_dj_pair(
    state: &crate::AppState,
    conn: &rusqlite::Connection,
) -> Option<crate::playback::dj_lookahead::DjLookaheadPair> {
    let current = state.ephemeral_tidal_track.as_ref()?;
    // Ordered next, not the raw lowest ephemeral row: a regular row in front means
    // the mix continuation is not what plays next, so there is no ephemeral pair.
    let pending = next_advance_ephemeral_track(conn)
        .ok()
        .flatten()
        .into_iter()
        .collect::<Vec<_>>();
    crate::playback::dj_lookahead::build_ephemeral_tidal_mix_pair(current, pending.as_slice())
}

pub(crate) fn active_ephemeral_tidal_mix_dj_labels(
    state: &crate::AppState,
) -> Vec<(
    crate::db::models::AudioDjProfileKey,
    (String, Option<String>),
)> {
    let mut labels = Vec::new();
    if let Some(current) = state.ephemeral_tidal_track.as_ref()
        && let Some(media_ref) = crate::playback::dj_lookahead::tidal_media_ref_for_track(current)
    {
        labels.push((
            media_ref.profile_key(),
            (current.title.clone(), current.artist_name.clone()),
        ));
    }
    let pending = state
        .db
        .with_conn(queue::peek_ephemeral_tidal_tracks)
        .unwrap_or_default();
    for track in pending.iter() {
        let media_ref = crate::playback::dj_lookahead::DjMediaRef::TidalTrack {
            tidal_id: track.tidal_track_id,
            track_id: None,
        };
        labels.push((
            media_ref.profile_key(),
            (track.title.clone(), track.artist_name.clone()),
        ));
    }
    labels
}

pub(crate) fn active_dj_pair_for_state_and_conn(
    state: &crate::AppState,
    conn: &rusqlite::Connection,
) -> anyhow::Result<crate::playback::dj_lookahead::DjLookaheadPair> {
    if let Some(pair) = active_ephemeral_tidal_mix_dj_pair(state, conn)
        && pair.next.is_some()
    {
        return Ok(pair);
    }
    let external_current = state
        .ephemeral_tidal_track
        .as_ref()
        .or(state.external_playback_track.as_ref());
    if let Some(current) = external_current
        && let Some(pair) =
            crate::playback::dj_lookahead::build_external_current_queue_pair(conn, current)?
    {
        return Ok(pair);
    }
    crate::playback::dj_lookahead::load_dj_lookahead_pair(conn)
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
    // now that has no DB record. The rest of the mix is already present in
    // snapshot.queue as real ephemeral TIDAL rows (loaded by load_queue), so no
    // overlay painting is needed - the queue you see is the queue that is.
    if let Some(ephemeral) = &state_guard.ephemeral_tidal_track {
        snapshot.state.current_track = Some(ephemeral.clone());
    } else if snapshot.state.current_track.is_none()
        && let Some(track) = state_guard.external_playback_track.as_ref()
    {
        snapshot.state.current_track = Some(track.clone());
    }
    drop(state_guard);
    snapshot
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

    // Update local DB immediately - Tidal sync happens in the background.
    // When liking for the first time, bump date_added so the track sorts to top of the library.
    {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| {
                // Liking a track promotes it into the library (covers an
                // explicit like of a previously-transient import); unliking
                // never demotes is_library, so an intentionally-unstarred
                // genuine track stays visible. See MIGRATION_052.
                conn.execute(
                    "UPDATE tracks SET is_favorite = ?1, \
                     is_library = CASE WHEN ?1 = 1 THEN 1 ELSE is_library END, \
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

    if payload.favorite && state_changed {
        crate::services::scrobbling::enqueue_favorite_love(state.clone(), &track).await;
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
                        // Token expired - refresh and retry once, matching the pattern
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

/// Toggle an album's favorite ("liked") state. Mirrors `set_track_favorite`:
/// the local `albums.is_favorite` flag flips immediately (which also counts
/// the album's tracks as library via `favorite_only`), and the TIDAL favorite
/// is synced in the background with a one-shot auth recovery. Unliking never
/// demotes anything beyond the favorite flag itself.
async fn set_album_favorite(
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

    let (tidal_tokens, http_client) = {
        let s = state.read().await;
        (s.tidal_tokens.clone(), s.http_client.clone())
    };
    let tidal_tokens = if tidal_tokens.is_none() {
        load_persisted_tidal_tokens(&state).await.ok().flatten()
    } else {
        tidal_tokens
    };

    {
        let s = state.read().await;
        s.db.with_conn(|conn| {
            conn.execute(
                "UPDATE albums SET is_favorite = ?1 WHERE id = ?2",
                rusqlite::params![if payload.favorite { 1 } else { 0 }, payload.album_id],
            )?;
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

    if let (Some(tidal_id), Some(tokens)) = (tidal_id, tidal_tokens) {
        if state_changed {
            let favorite = payload.favorite;
            let state_for_sync = state.clone();
            tokio::spawn(async move {
                let result = if favorite {
                    tidal_mutations::add_favorite_album(
                        &http_client,
                        &tokens.access_token,
                        &tokens.user_id,
                        tidal_id,
                        &tokens.country_code,
                    )
                    .await
                } else {
                    tidal_mutations::remove_favorite_album(
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
                        match recover_tidal_session(&state_for_sync, &http_client, &tokens).await {
                            Ok(refreshed) => {
                                let retry = if favorite {
                                    tidal_mutations::add_favorite_album(
                                        &http_client,
                                        &refreshed.access_token,
                                        &refreshed.user_id,
                                        tidal_id,
                                        &refreshed.country_code,
                                    )
                                    .await
                                } else {
                                    tidal_mutations::remove_favorite_album(
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
                                        "Failed to sync {} favorite for tidal album {tidal_id} after session refresh: {e2}",
                                        if favorite { "set" } else { "clear" },
                                    );
                                }
                            }
                            Err(re) => {
                                error!(
                                    "Session refresh failed while syncing {} favorite for tidal album {tidal_id}: {re}",
                                    if favorite { "set" } else { "clear" },
                                );
                            }
                        }
                    } else {
                        warn!(
                            "Failed to background-sync {} favorite for tidal album {tidal_id}: {error}",
                            if favorite { "set" } else { "clear" },
                        );
                    }
                }
            });
        }
    } else if tidal_id.is_some() && state_changed {
        warn!(
            "Album {} has tidal_id but no tokens available for sync",
            payload.album_id
        );
    }

    Ok(Json(json!({
        "album_id": payload.album_id,
        "tidal_id": tidal_id,
        "favorite": payload.favorite,
        "updated": state_changed
    })))
}

// -- MusicBrainz enrichment -------------------------------------------------

/// Build a `PlaybackSnapshot` whose `state.position_ms`, `state.buffered_ms`,
/// and `state.is_playing` reflect the live audio runtime (not just the DB
/// snapshot). Used by `GET /api/playback/state` and the route-side seek ack
/// in `POST /api/playback/position` so both responses carry a mutually
/// consistent view: returning a 409 body built from the raw DB snapshot
/// while the rejection decision was made from a live `buffered_samples`
/// read would be exactly the inconsistency the codex review flagged.
async fn build_live_playback_snapshot(
    state: &SharedState,
) -> Result<player::PlaybackSnapshot, StatusCode> {
    let (
        live_position_ms,
        live_buffered_ms,
        live_buffered_start_ms,
        ephemeral_playing,
        audio_active,
    ) = {
        let state_guard = state.read().await;
        let pair = state_guard
            .playback_runtime
            .as_ref()
            .zip(state_guard.playback_runtime_info.as_ref());
        let live_pos =
            pair.map(|(rt, info)| rt.handle.get_position_ms(info.sample_rate, info.channels));
        let live_buf =
            pair.map(|(rt, info)| rt.handle.get_buffered_ms(info.sample_rate, info.channels));
        let live_buf_start = pair.map(|(rt, info)| {
            rt.handle
                .get_buffered_start_ms(info.sample_rate, info.channels)
        });
        let ephemeral = state_guard.ephemeral_tidal_track.is_some();
        let active = state_guard
            .audio_active
            .load(std::sync::atomic::Ordering::Relaxed);
        (live_pos, live_buf, live_buf_start, ephemeral, active)
    };

    let snapshot = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(player::load_snapshot)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    let mut snapshot =
        overlay_snapshot_with_external_track_and_position(state, snapshot, live_position_ms).await;

    if let Some(buf) = live_buffered_ms {
        snapshot.state.buffered_ms = buf;
    }
    if let Some(buf_start) = live_buffered_start_ms {
        snapshot.state.buffered_start_ms = buf_start;
    }

    // Correct a stale is_playing flag before sending to the frontend:
    // - no runtime at all (server restarted, runtime crashed), OR
    // - runtime exists but CPAL buffer hasn't started draining yet (buffering phase).
    // Ephemeral TIDAL tracks bypass this check: they set is_playing themselves.
    if (!audio_active || live_position_ms.is_none()) && !ephemeral_playing {
        snapshot.state.is_playing = false;
    }

    Ok(snapshot)
}

async fn build_live_playback_snapshot_json(
    state: &SharedState,
) -> Result<player::PlaybackSnapshot, (StatusCode, Json<Value>)> {
    build_live_playback_snapshot(state).await.map_err(|status| {
        (
            status,
            Json(json!({ "error": "Playback snapshot unavailable" })),
        )
    })
}

async fn get_playback_state(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let snapshot = build_live_playback_snapshot(&state).await?;
    Ok(Json(json!({
        "state": snapshot.state,
        "queue": snapshot.queue
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

    // Was this track already a row in the queue BEFORE play_track_now (which
    // appends it if absent)? An in-queue jump (e.g. clicking a library row that
    // is sitting alongside a TIDAL mix continuation) must NOT wipe the ephemeral
    // rows: the user is moving within one Up Next list, not abandoning the mix.
    // We only clear the continuation when starting a track that is genuinely
    // outside the current queue. The "Play all / load queue" path already does a
    // full `DELETE FROM queue` in replace_queue, so by the time it reaches here
    // the played row is in the fresh queue and there are no ephemeral rows left
    // to clear anyway.
    let track_already_queued = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(|conn| {
                Ok::<bool, anyhow::Error>(
                    first_queue_item_id_for_track(conn, payload.track_id)?.is_some(),
                )
            })
            .unwrap_or(false)
    };

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
    // Always drop the in-memory ephemeral/external overlay (we're switching to a
    // library track), but only delete the persisted mix continuation rows when
    // this is not an in-queue jump. See `track_already_queued` above.
    clear_ephemeral_playback_markers(&state, !track_already_queued).await;

    let user_quality = current_user_audio_quality(&state).await;
    let stream_request = match player::build_tidal_stream_request(&track, user_quality.clone()) {
        Some(request) => request,
        None => {
            let paused_snapshot = {
                let state_guard = state.read().await;
                state_guard.db.with_conn(player::pause).ok()
            };
            // Flush the prior TIDAL session before bailing, otherwise the
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
        Err(error) if error.is_track_unplayable() => {
            // The track the user picked is a dead TIDAL asset. Rather than fail
            // the whole action, hand off to the skip-aware runtime switch, which
            // advances past it (and any further dead rows) and starts the next
            // playable track. Return the state it settles on.
            switch_runtime_to_snapshot_current(&state, &snapshot, playback_generation)
                .await
                .map_err(|error| {
                    let message = format!("Failed to advance past an unplayable track: {error}");
                    report_playback_failure(&state, &message);
                    (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({
                            "status": "playback_runtime_failed",
                            "message": message,
                        })),
                    )
                })?;
            return current_playback_snapshot_json(&state).await;
        }
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
    if !playback_generation_is_current(&state, playback_generation).await {
        return current_playback_snapshot_json(&state).await;
    }
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
    // Fire-and-forget play event: session health + artist attribution
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
                automix::ensure_automix_queue_depth(conn, automix::AUTOMIX_MIN_UPCOMING, false)
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

impl TidalPlaybackError {
    /// True when the failure is specific to *this track's asset*, so the right
    /// response is to skip past it and keep the queue moving. TIDAL's
    /// `4005 / "Asset is not ready"` and hard stream rejections mean this id
    /// won't play right now. Everything else (not connected, session refresh,
    /// network, rate-limit) is systemic or transient and must NOT burn through
    /// the queue one dead row at a time.
    fn is_track_unplayable(&self) -> bool {
        match self {
            TidalPlaybackError::StreamResolve(err) => {
                err.is_asset_not_ready() || err.is_stream_rejected()
            }
            _ => false,
        }
    }

    /// The narrower `4005 / asset-not-ready` case. A track that played fine
    /// before and now returns this usually had its catalog id rotated by TIDAL,
    /// so it's worth a background id re-resolve. A plain stream rejection
    /// (region lock, takedown) is not.
    fn is_asset_not_ready(&self) -> bool {
        matches!(self, TidalPlaybackError::StreamResolve(err) if err.is_asset_not_ready())
    }
}

#[cfg(test)]
mod unplayable_classification_tests {
    use super::*;
    use crate::services::tidal::stream::StreamResolveError;

    fn stream_resolve(err: StreamResolveError) -> TidalPlaybackError {
        TidalPlaybackError::StreamResolve(err)
    }

    #[test]
    fn asset_not_ready_4005_is_skippable_and_reresolvable() {
        let err = stream_resolve(StreamResolveError::StreamRejected {
            message: r#"TIDAL rejected playback request with 401 Unauthorized: {"status":401,"subStatus":4005,"userMessage":"Asset is not ready for playback"}"#.to_string(),
        });
        assert!(err.is_track_unplayable());
        assert!(err.is_asset_not_ready());
    }

    #[test]
    fn stream_rejected_without_4005_skips_but_does_not_reresolve() {
        let err = stream_resolve(StreamResolveError::StreamRejected {
            message: "TIDAL rejected playback request with 403 Forbidden".to_string(),
        });
        assert!(err.is_track_unplayable());
        assert!(!err.is_asset_not_ready());
    }

    #[test]
    fn network_and_session_failures_are_not_skippable() {
        // A transient network error must not burn through the queue one row at a time.
        let network = stream_resolve(StreamResolveError::RequestFailed {
            message: "error sending request: dns error".to_string(),
        });
        assert!(!network.is_track_unplayable());
        assert!(!network.is_asset_not_ready());

        assert!(!TidalPlaybackError::NotConnected.is_track_unplayable());
        assert!(
            !TidalPlaybackError::SessionRefreshFailed("boom".to_string()).is_track_unplayable()
        );
    }
}

fn runtime_stream_resolver(state: SharedState) -> playback_runtime::RuntimeStreamResolver {
    let state = Arc::downgrade(&state);
    Arc::new(move |request| {
        let state = state.clone();
        Box::pin(async move {
            let state = state
                .upgrade()
                .ok_or_else(|| anyhow::anyhow!("server state is no longer available"))?;
            resolve_tidal_runtime_stream(&state, request).await
        })
    })
}

async fn resolve_tidal_runtime_stream(
    state: &SharedState,
    request: tidal_stream::StreamRequest,
) -> anyhow::Result<tidal_stream::StreamInfo> {
    let tokens = {
        let state_guard = state.read().await;
        state_guard.tidal_tokens.clone()
    }
    .ok_or_else(|| anyhow::anyhow!("TIDAL is not connected."))?;

    let http = {
        let state_guard = state.read().await;
        state_guard.http_client.clone()
    };

    match tidal_stream::resolve_stream(&http, &tokens.access_token, &request).await {
        Ok(info) => Ok(info),
        Err(error) if error.is_session_expired() => {
            tracing::warn!(
                target: "noor.playback.tidal",
                event = "runtime_stream_session_expired",
                track_id = request.track_id,
                error = %error,
                "TIDAL session expired in playback decoder; refreshing session"
            );
            let refreshed = recover_tidal_session(state, &http, &tokens).await?;
            match tidal_stream::resolve_stream(&http, &refreshed.access_token, &request).await {
                Ok(info) => Ok(info),
                Err(retry_error) if retry_error.is_session_expired() => {
                    let _ = clear_tidal_session(state).await;
                    Err(anyhow::anyhow!(
                        "TIDAL session expired after refresh while resolving runtime stream: {retry_error}"
                    ))
                }
                Err(retry_error) => Err(anyhow::Error::from(retry_error)),
            }
        }
        Err(error) => Err(anyhow::Error::from(error)),
    }
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
                    // Do NOT clear the session here. A transient network error during
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
                    // Still expired after a successful refresh: TIDAL revoked the account.
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
    let _playback_generation = bump_playback_generation(&state).await;

    if let Some(runtime_handle) = current_playback_runtime(&state).await
        && let Err(error) = runtime_handle.pause()
    {
        let message = format!("Failed to pause host audio playback: {error}");
        report_playback_failure(&state, &message);
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    // Opt-in: free the exclusive WASAPI device on an explicit pause so other
    // apps can take the DAC without waiting out the idle-release grace. No-op
    // when exclusive mode is off (the runtime guards on current_exclusive) or
    // the setting is disabled. Re-grabbed automatically on the next Resume/Play.
    let release_on_pause = {
        let guard = state.read().await;
        guard
            .db
            .with_conn(|conn| crate::db::audio_settings::load(conn).map_err(Into::into))
            .map(|s| s.exclusive_release_on_pause)
            .unwrap_or(false)
    };
    if release_on_pause && let Some(runtime_handle) = current_playback_runtime(&state).await {
        let _ = runtime_handle.release_exclusive_now();
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
    // start a new session. resume_session_after_snapshot will reopen one
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

/// Drop the WASAPI exclusive device immediately so the WebView can play a
/// TIDAL video's audio in shared mode. The frontend hits this when a video
/// starts. No-op when there's no runtime or exclusive mode is off; the runtime
/// re-grabs exclusive on the next Resume/Play. Returns ok even on a soft miss
/// so video startup never blocks on it.
async fn release_exclusive_playback(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    if let Some(runtime_handle) = current_playback_runtime(&state).await
        && let Err(error) = runtime_handle.release_exclusive_now()
    {
        tracing::warn!(
            target = "noor.playback",
            event = "exclusive_release_failed",
            "Failed to request exclusive release: {error}"
        );
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    Ok(Json(json!({ "ok": true })))
}

async fn resume_playback(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let _playback_generation = bump_playback_generation(&state).await;

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

// --- Pending-row resolution --------------------------------------------------
//
// Both the lazy (next_track caller) and background-eager (radio_start) paths
// share the same scoring constants. The lazy path also closes the
// playback_state NULL window after promotion; the background path does not.

const MATCH_QUALITY_THRESHOLD: f64 = 0.85;
const RESOLVER_POOL_SIZE: usize = 4;

// Scoring weights (two-field, no album metadata available from Last.fm).
// Three-field variant (0.55/0.35/0.10) applies when pending_album is stored:
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

// How many search hits to consider when resolving a pending row. Heavily
// remixed songs push the plain studio cut well down TIDAL's relevance order, so
// pulling only the top few can leave the original out of the candidate set
// entirely. Ten is enough headroom without materially changing latency.
const TIDAL_RESOLVE_POOL: i32 = 10;

/// How a TIDAL track relates to the plain studio recording, inferred from its
/// `version` field (authoritative) or a trailing descriptor in the title.
///
/// `Original` is the canonical performance: no version tag, or a marker that
/// only describes mastering/format (remaster, mono, deluxe edition...) which is
/// the *same* recording and stays eligible. Every other class is a different
/// recording and gets demoted unless the request explicitly asked for it. This
/// version axis is the only thing separating "American Pie" from "American Pie
/// (L'Tric Remix)": they share a base title and artist, so title+artist scoring
/// alone ties them at 1.0 and the remix can win by listing order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VersionClass {
    Original,
    Remix,
    Live,
    Acoustic,
    Instrumental,
    Cover,
    SpedSlowed,
    Edit,
    OtherVariant,
}

fn normalize_version_text(s: &str) -> String {
    s.to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn version_text_has_word(normalized: &str, word: &str) -> bool {
    normalized.split(' ').any(|tok| tok == word)
}

/// Classify a free-text version descriptor. Returns `None` when nothing is
/// recognized, so callers decide what an unknown tag means in context: a
/// populated TIDAL `version` field is a deliberate variant flag, while a bare
/// title parenthetical like "(Pt. 1)" is probably just part of the title.
fn classify_version_descriptor(descriptor: &str) -> Option<VersionClass> {
    let d = normalize_version_text(descriptor);
    if d.is_empty() {
        return None;
    }
    // Mastering / format / explicitly-original markers describe the same
    // performance, so they resolve to Original. Checked first so "Original Mix"
    // and "Deluxe Edition" never fall through to the "mix"/"edit" branches.
    const MASTERING: &[&str] = &[
        "original mix",
        "original version",
        "album version",
        "single version",
        "original",
        "remaster",
        "remastered",
        "mono",
        "stereo",
        "deluxe",
        "anniversary",
        "expanded",
        "reissue",
        "edition",
        "bonus",
    ];
    if MASTERING.iter().any(|m| d.contains(m)) {
        return Some(VersionClass::Original);
    }
    const REMIX: &[&str] = &[
        "remix", "rmx", "bootleg", "rework", "flip", "vip", "mashup", "mash up", "club mix", "dub",
    ];
    if REMIX.iter().any(|m| d.contains(m)) {
        return Some(VersionClass::Remix);
    }
    if version_text_has_word(&d, "live") || d.contains("in concert") {
        return Some(VersionClass::Live);
    }
    if d.contains("acoustic") || d.contains("unplugged") {
        return Some(VersionClass::Acoustic);
    }
    if d.contains("instrumental") || d.contains("karaoke") {
        return Some(VersionClass::Instrumental);
    }
    if d.contains("cover")
        || d.contains("originally performed")
        || d.contains("made famous")
        || d.contains("tribute")
    {
        return Some(VersionClass::Cover);
    }
    if d.contains("sped up")
        || d.contains("spedup")
        || d.contains("slowed")
        || d.contains("nightcore")
    {
        return Some(VersionClass::SpedSlowed);
    }
    if version_text_has_word(&d, "edit") || d.contains("extended") {
        return Some(VersionClass::Edit);
    }
    None
}

/// Split a trailing variant descriptor off a title. Only the last bracketed
/// group or a " - " tail is considered:
/// "American Pie (L'Tric Remix)" -> ("American Pie", Some("L'Tric Remix")).
fn split_title_descriptor(title: &str) -> (String, Option<String>) {
    let t = title.trim();
    if let Some(open) = t.rfind(['(', '[']) {
        let want_close = if t.as_bytes()[open] == b'(' {
            b')'
        } else {
            b']'
        };
        if t.as_bytes().last() == Some(&want_close) {
            let inner = t[open + 1..t.len() - 1].trim();
            let base = t[..open].trim();
            if !inner.is_empty() && !base.is_empty() {
                return (base.to_string(), Some(inner.to_string()));
            }
        }
    }
    if let Some(idx) = t.rfind(" - ") {
        let desc = t[idx + 3..].trim();
        let base = t[..idx].trim();
        if !desc.is_empty() && !base.is_empty() {
            return (base.to_string(), Some(desc.to_string()));
        }
    }
    (t.to_string(), None)
}

/// Base title (for fuzzy scoring) plus version class for a search candidate. The
/// `version` field wins; a populated-but-unrecognized version still means "not
/// the plain original" and demotes. Without a version field we read a title
/// descriptor, where an unrecognized parenthetical is kept as part of the title.
fn classify_candidate(track: &TidalSearchTrack) -> (String, VersionClass) {
    let version = track
        .extra
        .get("version")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(version) = version {
        let class = classify_version_descriptor(version).unwrap_or(VersionClass::OtherVariant);
        return (track.title.clone(), class);
    }
    classify_title_field(&track.title)
}

/// Base title plus version class for a free title string with no separate
/// version field (e.g. a Last.fm suggestion). Unrecognized descriptors stay
/// Original so genuine titles like "Shine On You Crazy Diamond (Pt. 1)" match.
fn classify_title_field(title: &str) -> (String, VersionClass) {
    let (base, desc) = split_title_descriptor(title);
    match desc.as_deref().and_then(classify_version_descriptor) {
        Some(class) => (base, class),
        None => (title.trim().to_string(), VersionClass::Original),
    }
}

fn version_quality_rank(quality: Option<&str>) -> u8 {
    match quality.map(str::to_ascii_uppercase).as_deref() {
        Some("HI_RES_LOSSLESS") | Some("HI_RES") => 3,
        Some("LOSSLESS") => 2,
        Some("HIGH") => 1,
        _ => 0,
    }
}

/// Pick the best TIDAL search result for a pending `(artist, title)`, preferring
/// the version the request actually implies. Pure (no network) so it can be unit
/// tested against synthetic candidate sets.
///
/// 1. Score every candidate on base-title + artist Jaro-Winkler (variant
///    descriptors stripped first), keeping those that clear the threshold.
/// 2. Partition by whether the candidate's version class matches the request.
///    Prefer the matching set; fall back to the rest only when nothing matched,
///    so a song that exists *only* as a remix still resolves instead of stalling.
/// 3. Within the chosen set, rank by score, then descriptor closeness when a
///    specific variant was named (so a named remix beats a different one), then
///    audio quality as a hi-fi-friendly final tiebreak.
fn select_best_tidal_match(
    pending_artist: &str,
    pending_title: &str,
    results: Vec<TidalSearchTrack>,
) -> Option<(f64, TidalSearchTrack)> {
    let (pending_base, pending_class) = classify_title_field(pending_title);
    let pending_desc_norm = split_title_descriptor(pending_title)
        .1
        .map(|d| normalize_version_text(&d))
        .unwrap_or_default();

    struct Scored {
        score: f64,
        class: VersionClass,
        desc_sim: f64,
        quality: u8,
        track: TidalSearchTrack,
    }

    let mut scored: Vec<Scored> = results
        .into_iter()
        .filter_map(|track| {
            let (cand_base, class) = classify_candidate(&track);
            let score = score_tidal_candidate(
                track.artist_name.as_deref().unwrap_or(""),
                &cand_base,
                pending_artist,
                &pending_base,
            );
            if score < MATCH_QUALITY_THRESHOLD {
                return None;
            }
            let cand_desc_norm = track
                .extra
                .get("version")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .or_else(|| split_title_descriptor(&track.title).1)
                .map(|d| normalize_version_text(&d))
                .unwrap_or_default();
            let desc_sim = if pending_desc_norm.is_empty() {
                0.0
            } else {
                strsim::jaro_winkler(&pending_desc_norm, &cand_desc_norm)
            };
            Some(Scored {
                score,
                class,
                desc_sim,
                quality: version_quality_rank(track.audio_quality.as_deref()),
                track,
            })
        })
        .collect();

    if scored.is_empty() {
        return None;
    }

    // Prefer candidates whose version class matches the request; only keep the
    // mismatched ones if nothing matched at all (the fallback).
    if scored
        .iter()
        .any(|s| version_intent_matches(pending_class, s.class))
    {
        scored.retain(|s| version_intent_matches(pending_class, s.class));
    }

    let want_desc = !pending_desc_norm.is_empty();
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                if want_desc {
                    b.desc_sim
                        .partial_cmp(&a.desc_sim)
                        .unwrap_or(std::cmp::Ordering::Equal)
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then(b.quality.cmp(&a.quality))
    });

    scored.into_iter().next().map(|s| (s.score, s.track))
}

/// A candidate satisfies the request when its version class is the same. A clean
/// request (`Original`) only accepts originals; a request that named a variant
/// (remix, acoustic...) only accepts that same kind.
fn version_intent_matches(pending: VersionClass, candidate: VersionClass) -> bool {
    pending == candidate
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
    let results = client.search(&query, TIDAL_RESOLVE_POOL).await?;
    Ok(
        select_best_tidal_match(pending_artist, pending_title, results)
            .map(|(score, track)| (score, import_metadata_from_search_track(track))),
    )
}

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

/// Atomically promote a pending queue row to a resolved library row, then
/// broadcast `QueueUpdated` if this caller won the race.
///
/// Wraps [`pending::promote`] so both resolver paths funnel through one
/// event-emission contract: any successful promotion broadcasts exactly
/// once.
fn promote_pending_row_emit(
    db: &crate::db::Database,
    event_tx: &tokio::sync::broadcast::Sender<AppEvent>,
    queue_item_id: i64,
    local_track_id: i64,
    score_stored: i32,
) -> bool {
    let promoted = db
        .with_conn(move |conn| pending::promote(conn, queue_item_id, local_track_id, score_stored))
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
/// does **not** update `playback_state.current_track_id`. The playing row
/// may not be the one being resolved.
async fn resolve_pending_row(
    db: crate::db::Database,
    tokens: crate::services::tidal::auth::TidalTokens,
    queue_item_id: i64,
    event_tx: tokio::sync::broadcast::Sender<AppEvent>,
    http: reqwest::Client,
) -> bool {
    let row = db
        .with_conn(move |conn| pending::read_identity(conn, queue_item_id))
        .unwrap_or(None);

    let (pending_artist, pending_title, tidal_id_hint) = match row {
        Some(r) => r,
        None => return false,
    };

    let claimed = db
        .with_conn(move |conn| pending::try_claim(conn, queue_item_id))
        .unwrap_or(false);
    if !claimed {
        return false;
    }

    let release = |db: &crate::db::Database, qid: i64| {
        let _ = db.with_conn(move |conn| {
            pending::release(conn, qid);
            Ok(())
        });
    };

    let client = TidalClient::with_http(
        http.clone(),
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
            return false;
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
            return false;
        }
    };

    let artist_tidal_id = metadata.artist_tidal_id;
    let imported = crate::services::tidal::import::import_track_from_metadata(&db, metadata).await;

    let (local_id, artist_local_id) = match imported {
        Ok(imp) => (imp.local_id, imp.artist_id),
        Err(e) => {
            tracing::warn!(queue_item_id, error = %e, "background resolver: import failed");
            release(&db, queue_item_id);
            return false;
        }
    };

    // Fire-and-forget: backfill artist photo when TIDAL track payload didn't
    // include one. Independent of promotion success â€” the artist row now
    // exists either way.
    if let Some(tid) = artist_tidal_id {
        let db_bg = db.clone();
        let http_bg = http.clone();
        let tok_bg = tokens.clone();
        tokio::spawn(async move {
            crate::services::tidal::artist_photo::ensure_photo_url(
                http_bg,
                tok_bg,
                db_bg,
                artist_local_id,
                tid,
            )
            .await;
        });
    }

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
    promoted
}

/// Attempts to resolve the current pending queue item to a Tidal track.
/// Called when the current queue item is a pending row: track_id IS NULL.
/// Claims ownership via resolving_at, searches Tidal with combined Jaro-Winkler scoring
/// (0.60xartist + 0.40xtitle, threshold 0.85), imports the match via
/// import_track_from_metadata, and atomically promotes the queue row.
///
/// Returns the resolved Track on success, or None if no acceptable match or on error.
/// On failure the resolving_at ownership lock is always released.
async fn resolve_pending_current_queue_item(
    state: &SharedState,
    expected_generation: u64,
) -> Option<crate::db::models::Track> {
    let db = {
        let s = state.read().await;
        s.db.clone()
    };

    let (queue_item_id, pending_artist, pending_title, tidal_id_hint) = db
        .with_conn(|conn| pending::current_pending(conn))
        .ok()
        .flatten()?;

    if !playback_generation_is_current(state, expected_generation).await {
        return None;
    }

    tracing::debug!(
        target: "noor.playback.resolve",
        event = "pending_current_resolve_start",
        queue_item_id,
        tidal_id_hint,
        "resolving current pending queue row"
    );

    // Claim ownership; bail if another resolver already claimed this row.
    let claimed = db
        .with_conn(|conn| pending::try_claim(conn, queue_item_id))
        .unwrap_or(false);
    if !claimed {
        tracing::debug!(
            target: "noor.playback.resolve",
            event = "pending_current_resolve_claim_skipped",
            queue_item_id,
            "current pending row is already being resolved"
        );
        return None;
    }

    let release_lock = |db: &crate::db::Database, qid: i64| {
        let _ = db.with_conn(move |conn| {
            pending::release(conn, qid);
            Ok(())
        });
    };

    let (tokens, tidal_http_client) = {
        let persisted = match load_persisted_tidal_tokens(state).await.ok().flatten() {
            Some(t) => t,
            None => {
                tracing::warn!(
                    target: "noor.playback.resolve",
                    event = "pending_current_resolve_no_tokens",
                    queue_item_id,
                    "TIDAL tokens unavailable for current pending queue row"
                );
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
            Err(error) => {
                tracing::warn!(
                    target: "noor.playback.resolve",
                    event = "pending_current_resolve_api_failed",
                    queue_item_id,
                    error = %error,
                    "TIDAL lookup failed for current pending queue row"
                );
                release_lock(&db, queue_item_id);
                return None;
            }
        };

    let (score, metadata) = match resolved {
        Some(pair) => pair,
        None => {
            tracing::debug!(
                target: "noor.playback.resolve",
                event = "pending_current_resolve_no_match",
                queue_item_id,
                "no acceptable TIDAL match for current pending queue row"
            );
            release_lock(&db, queue_item_id);
            return None;
        }
    };

    let artist_tidal_id = metadata.artist_tidal_id;
    let imported = crate::services::tidal::import::import_track_from_metadata(&db, metadata).await;

    let (local_id, artist_local_id) = match imported {
        Ok(imp) => (imp.local_id, imp.artist_id),
        Err(error) => {
            tracing::warn!(
                target: "noor.playback.resolve",
                event = "pending_current_import_failed",
                queue_item_id,
                error = %error,
                "import failed for current pending queue row"
            );
            release_lock(&db, queue_item_id);
            return None;
        }
    };

    if !playback_generation_is_current(state, expected_generation).await {
        release_lock(&db, queue_item_id);
        return None;
    }

    if let Some(tid) = artist_tidal_id {
        let db_bg = db.clone();
        let http_bg = state.read().await.http_client.clone();
        let tok_bg = tokens.clone();
        tokio::spawn(async move {
            crate::services::tidal::artist_photo::ensure_photo_url(
                http_bg,
                tok_bg,
                db_bg,
                artist_local_id,
                tid,
            )
            .await;
        });
    }

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
    let state_updated = db
        .with_conn(move |conn| {
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = ?1
                 WHERE id = 1 AND current_queue_item_id = ?2",
                rusqlite::params![local_id, queue_item_id],
            )
            .map_err(anyhow::Error::from)
        })
        .unwrap_or(0);
    if state_updated == 0 || !playback_generation_is_current(state, expected_generation).await {
        return None;
    }
    let _ = event_tx.send(AppEvent::TrackChanged { track_id: local_id });
    let _ = event_tx.send(AppEvent::PlaybackStateChanged);

    tracing::info!(
        target: "noor.playback.resolve",
        event = "pending_current_resolve_success",
        queue_item_id,
        local_id,
        score,
        "current pending queue row resolved"
    );

    db.with_conn(move |conn| queue::get_track_by_id(conn, local_id))
        .ok()
        .flatten()
}

async fn load_persisted_playback_snapshot(
    state: &SharedState,
) -> anyhow::Result<player::PlaybackSnapshot> {
    let state_guard = state.read().await;
    state_guard.db.with_conn(player::load_snapshot)
}

async fn next_persisted_playback_snapshot(
    state: &SharedState,
) -> anyhow::Result<player::PlaybackSnapshot> {
    let state_guard = state.read().await;
    let cleared = recently_cleared(&state_guard);
    state_guard
        .db
        .with_conn(|conn| player::next_track(conn, cleared))
}

async fn previous_persisted_playback_snapshot(
    state: &SharedState,
) -> anyhow::Result<player::PlaybackSnapshot> {
    let state_guard = state.read().await;
    state_guard.db.with_conn(player::previous_track)
}

#[derive(Clone, Copy)]
enum PendingAdvanceDirection {
    Next,
    Previous,
}

impl PendingAdvanceDirection {
    fn as_str(self) -> &'static str {
        match self {
            Self::Next => "next",
            Self::Previous => "previous",
        }
    }
}

async fn step_persisted_playback_snapshot(
    state: &SharedState,
    direction: PendingAdvanceDirection,
) -> anyhow::Result<player::PlaybackSnapshot> {
    match direction {
        PendingAdvanceDirection::Next => next_persisted_playback_snapshot(state).await,
        PendingAdvanceDirection::Previous => previous_persisted_playback_snapshot(state).await,
    }
}

async fn adopt_resolved_current_queue_item(
    state: &SharedState,
    queue_item_id: i64,
    generation: u64,
) -> anyhow::Result<Option<player::PlaybackSnapshot>> {
    if !playback_generation_is_current(state, generation).await {
        return Ok(None);
    }

    let (db, event_tx) = {
        let state_guard = state.read().await;
        (state_guard.db.clone(), state_guard.event_tx.clone())
    };

    let adopted_track_id = db.with_conn(move |conn| {
        let track_id: Option<i64> = conn
            .query_row(
                "SELECT track_id FROM queue WHERE id = ?1 AND track_id IS NOT NULL",
                params![queue_item_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(track_id) = track_id else {
            return Ok(None);
        };
        let updated = conn.execute(
            "UPDATE playback_state
             SET current_track_id = ?1, position_ms = 0
             WHERE id = 1
               AND current_track_id IS NULL
               AND current_queue_item_id = ?2",
            params![track_id, queue_item_id],
        )?;
        Ok(if updated == 1 { Some(track_id) } else { None })
    })?;

    let Some(track_id) = adopted_track_id else {
        return Ok(None);
    };
    if !playback_generation_is_current(state, generation).await {
        return Ok(None);
    }

    let _ = event_tx.send(AppEvent::TrackChanged { track_id });
    let _ = event_tx.send(AppEvent::PlaybackStateChanged);
    let snapshot = load_persisted_playback_snapshot(state).await?;
    if snapshot.state.current_track.is_some() {
        tracing::info!(
            target: "noor.playback.resolve",
            event = "pending_current_adopted_background_resolution",
            queue_item_id,
            track_id,
            "adopted pending row resolved by background resolver"
        );
        return Ok(Some(snapshot));
    }
    Ok(None)
}

async fn pending_current_resolver_is_busy(state: &SharedState, queue_item_id: i64) -> bool {
    let db = {
        let state_guard = state.read().await;
        state_guard.db.clone()
    };
    db.with_conn(move |conn| pending::has_fresh_resolver_lock(conn, queue_item_id))
        .unwrap_or(false)
}

async fn stop_persisted_playback_after_advance_failure(
    state: &SharedState,
    context: &'static str,
) -> anyhow::Result<player::PlaybackSnapshot> {
    tracing::warn!(
        target: "noor.playback.advance",
        event = "advance_stopped_after_pending_skip_limit",
        context,
        "stopping playback after pending queue rows failed to resolve"
    );
    let state_guard = state.read().await;
    state_guard.db.with_conn(|conn| {
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = NULL,
                 current_queue_item_id = NULL,
                 position_ms = 0,
                 is_playing = 0
             WHERE id = 1",
            [],
        )?;
        player::load_snapshot(conn)
    })
}

async fn resolve_or_skip_pending_current(
    state: &SharedState,
    snapshot: player::PlaybackSnapshot,
    generation: u64,
    context: &'static str,
) -> anyhow::Result<player::PlaybackSnapshot> {
    resolve_or_skip_pending_current_in_direction(
        state,
        snapshot,
        generation,
        context,
        PendingAdvanceDirection::Next,
    )
    .await
}

async fn resolve_or_skip_pending_current_previous(
    state: &SharedState,
    snapshot: player::PlaybackSnapshot,
    generation: u64,
    context: &'static str,
) -> anyhow::Result<player::PlaybackSnapshot> {
    resolve_or_skip_pending_current_in_direction(
        state,
        snapshot,
        generation,
        context,
        PendingAdvanceDirection::Previous,
    )
    .await
}

async fn resolve_or_skip_pending_current_in_direction(
    state: &SharedState,
    mut snapshot: player::PlaybackSnapshot,
    generation: u64,
    context: &'static str,
    direction: PendingAdvanceDirection,
) -> anyhow::Result<player::PlaybackSnapshot> {
    let mut skipped = 0usize;
    let mut busy_waits = 0usize;

    loop {
        if snapshot.state.current_track.is_some() || snapshot.state.current_queue_item_id.is_none()
        {
            return Ok(snapshot);
        }

        let Some(queue_item_id) = snapshot.state.current_queue_item_id else {
            return Ok(snapshot);
        };
        if resolve_pending_current_queue_item(state, generation)
            .await
            .is_some()
        {
            snapshot = load_persisted_playback_snapshot(state).await?;
            if snapshot.state.current_track.is_some() {
                return Ok(snapshot);
            }
        } else {
            let reloaded = load_persisted_playback_snapshot(state).await?;
            if reloaded.state.current_track.is_some()
                || reloaded.state.current_queue_item_id != Some(queue_item_id)
            {
                snapshot = reloaded;
                busy_waits = 0;
                continue;
            }
        }

        if !playback_generation_is_current(state, generation).await {
            return load_persisted_playback_snapshot(state).await;
        }

        if let Some(adopted_snapshot) =
            adopt_resolved_current_queue_item(state, queue_item_id, generation).await?
        {
            return Ok(adopted_snapshot);
        }

        if busy_waits < PLAYBACK_PENDING_BUSY_RETRY_LIMIT
            && pending_current_resolver_is_busy(state, queue_item_id).await
        {
            busy_waits += 1;
            tracing::debug!(
                target: "noor.playback.advance",
                event = "pending_current_busy_wait",
                context,
                queue_item_id,
                busy_waits,
                direction = direction.as_str(),
                "current pending row is still resolving; waiting before skip"
            );
            tokio::time::sleep(Duration::from_millis(PLAYBACK_PENDING_BUSY_RETRY_DELAY_MS)).await;
            snapshot = load_persisted_playback_snapshot(state).await?;
            continue;
        }

        skipped += 1;
        busy_waits = 0;
        tracing::warn!(
            target: "noor.playback.advance",
            event = "pending_current_skipped",
            context,
            queue_item_id,
            skipped,
            direction = direction.as_str(),
            "current pending row did not resolve; stepping over queue item"
        );

        if skipped > PLAYBACK_ADVANCE_PENDING_SKIP_LIMIT {
            return stop_persisted_playback_after_advance_failure(state, context).await;
        }

        snapshot = step_persisted_playback_snapshot(state, direction).await?;
    }
}

async fn advance_ephemeral_next_if_needed(
    state: &SharedState,
) -> Result<Option<Json<Value>>, (StatusCode, Json<Value>)> {
    let has_ephemeral = {
        let state_guard = state.read().await;
        state_guard.ephemeral_tidal_track.is_some()
    };

    // Ordered advance decision. Pop the mix continuation only when its row is
    // genuinely next in queue order: a regular row in front of it (library
    // "Play next" during a mix) falls through to the normal queue advance
    // instead of being silently skipped. The pop also fires when the live
    // track is NOT ephemeral (a play-next interlude just played) so the mix
    // resumes behind the interlude's anchor.
    let next = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(pop_next_ephemeral_if_due)
            .ok()
            .flatten()
    };

    if !has_ephemeral && next.is_none() {
        return Ok(None);
    }

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

fn automix_discover_new_fallback_seed(
    snapshot: &crate::playback::player::PlaybackSnapshot,
) -> Option<crate::db::models::Track> {
    if !snapshot.state.automix_discover_new {
        return None;
    }
    let current_track_id = snapshot.state.current_track.as_ref().map(|track| track.id);
    let current_pos = snapshot
        .state
        .current_queue_item_id
        .and_then(|qid| {
            snapshot
                .queue
                .iter()
                .find(|item| item.id == qid)
                .filter(|item| Some(item.track.id) == current_track_id)
                .map(|item| item.position)
        })
        .or_else(|| {
            snapshot.state.current_track.as_ref().and_then(|track| {
                snapshot
                    .queue
                    .iter()
                    .find(|item| item.track.id == track.id)
                    .map(|item| item.position)
            })
        })
        .unwrap_or(0);
    let automix_new_upcoming = snapshot
        .queue
        .iter()
        .filter(|item| item.position > current_pos && item.source == "automix-new")
        .count();
    if automix_new_upcoming < 2 {
        snapshot.state.current_track.clone()
    } else {
        None
    }
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
    snapshot =
        resolve_or_skip_pending_current(&state, snapshot, playback_generation, "manual_next_track")
            .await
            .map_err(|error| {
                tracing::error!(
                    target: "noor.playback.advance",
                    event = "manual_next_pending_advance_failed",
                    error = %error,
                    "failed to resolve or skip pending row while advancing playback"
                );
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "status": "playback_state_update_failed",
                        "message": "Failed to advance playback state.",
                    })),
                )
            })?;

    if !playback_generation_is_current(&state, playback_generation).await {
        return current_playback_snapshot_json(&state).await;
    }

    record_transition_if_changed(&state, previous_track_id, &snapshot, "queue", true).await;

    // When "Include New" is enabled, search TIDAL for genre/artist-matched tracks and
    // inject any that aren't already in the library. Runs as a detached background task
    // so the next_track response returns immediately without blocking on TIDAL API calls.
    if let Some(track) = automix_discover_new_fallback_seed(&snapshot) {
        let bg_state = state.clone();
        tokio::spawn(async move {
            inject_discovery_tracks(&bg_state, &track).await;
        });
    }

    let end_reason = if snapshot.state.current_track.is_some() {
        Some(player::ListenSessionEndReason::Replaced)
    } else {
        Some(player::ListenSessionEndReason::QueueEnded)
    };
    sync_session_after_snapshot(&state, &snapshot, end_reason).await;

    let play_track = snapshot.state.current_track.as_ref();

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
        let stream_info = match resolve_tidal_playback_stream(&state, track, &stream_request).await
        {
            Ok(info) => info,
            Err(error) if error.is_track_unplayable() => {
                // Dead asset: hand off to the skip-aware runtime switch, which
                // advances past this row (and any further dead ones) and starts
                // the next playable track. Return the state it settles on.
                switch_runtime_to_snapshot_current(&state, &snapshot, playback_generation)
                    .await
                    .map_err(|error| {
                        let message =
                            format!("Failed to advance past an unplayable track: {error}");
                        report_playback_failure(&state, &message);
                        (
                            StatusCode::BAD_GATEWAY,
                            Json(json!({
                                "status": "playback_runtime_failed",
                                "message": message,
                            })),
                        )
                    })?;
                return current_playback_snapshot_json(&state).await;
            }
            Err(error) => {
                return Err(tidal_playback_error_response(
                    track.id,
                    error,
                    "TIDAL stream could not be resolved while advancing playback.",
                ));
            }
        };
        if !playback_generation_is_current(&state, playback_generation).await {
            return current_playback_snapshot_json(&state).await;
        }
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
    let event_track_id = snapshot.state.current_track.as_ref().map(|t| t.id);
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

/// During a live TIDAL mix, "previous" restarts the current track from the top
/// instead of running the persistent-queue previous logic. A mix is forward-only:
/// played rows are deleted, so there is no earlier row to return to, and the DB
/// anchor is NULL. Without this guard, `player::previous_track`'s "nothing playing
/// -> jump to first item" fallback re-anchors onto the next mix row and marks it
/// playing while the runtime keeps playing the ephemeral track, corrupting state
/// ("previous loads them back in"). Returns `Some(response)` when handled.
async fn restart_ephemeral_current_if_needed(
    state: &SharedState,
) -> Result<Option<Json<Value>>, (StatusCode, Json<Value>)> {
    let has_ephemeral = {
        let state_guard = state.read().await;
        state_guard.ephemeral_tidal_track.is_some()
    };
    if !has_ephemeral {
        return Ok(None);
    }

    let handle = {
        let state_guard = state.read().await;
        state_guard
            .playback_runtime
            .as_ref()
            .map(|rt| rt.handle.clone())
    };
    if let Some(handle) = handle {
        // Segment-aware restart to 0, same path the seek route uses.
        let _ = tokio::task::spawn_blocking(move || handle.seek_to_segment_aware(0, true)).await;
    }
    {
        let state_guard = state.read().await;
        let _ = state_guard.db.with_conn(|conn| {
            conn.execute("UPDATE playback_state SET position_ms = 0 WHERE id = 1", [])?;
            Ok::<_, anyhow::Error>(())
        });
    }

    let snapshot = build_live_playback_snapshot_json(state).await?;
    {
        let state_guard = state.read().await;
        let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    }
    Ok(Some(Json(json!({
        "state": snapshot.state,
        "queue": snapshot.queue
    }))))
}

async fn previous_track(
    State(state): State<SharedState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if let Some(response) = restart_ephemeral_current_if_needed(&state).await? {
        return Ok(response);
    }

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
    snapshot = resolve_or_skip_pending_current_previous(
        &state,
        snapshot,
        playback_generation,
        "manual_previous_track",
    )
    .await
    .map_err(|error| {
        tracing::error!(
            target: "noor.playback.advance",
            event = "manual_previous_pending_advance_failed",
            error = %error,
            "failed to resolve or skip pending row while moving to previous playback item"
        );
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "status": "playback_state_update_failed",
                "message": "Failed to move to the previous track.",
            })),
        )
    })?;

    if !playback_generation_is_current(&state, playback_generation).await {
        return current_playback_snapshot_json(&state).await;
    }

    record_transition_if_changed(&state, previous_track_id, &snapshot, "user", false).await;

    sync_session_after_snapshot(
        &state,
        &snapshot,
        Some(player::ListenSessionEndReason::Replaced),
    )
    .await;

    let play_track = snapshot.state.current_track.as_ref();

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
        if !playback_generation_is_current(&state, playback_generation).await {
            return current_playback_snapshot_json(&state).await;
        }
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
    let event_track_id = snapshot.state.current_track.as_ref().map(|t| t.id);
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
) -> Result<(StatusCode, Json<Value>), StatusCode> {
    // Option C: route is a dumb dispatcher. The runtime's SeekTo handler
    // decides in-buffer / segment-restart / reject; we just translate the
    // outcome to a status code and return a snapshot.
    //
    // No runtime active (pre-first-play boot, or runtime crashed): silent OK
    // with the current snapshot. A seek with no runtime is a UI race we don't
    // need to fail the response over.
    let handle = {
        let g = state.read().await;
        g.playback_runtime.as_ref().map(|rt| rt.handle.clone())
    };
    let outcome = match handle {
        Some(handle) => {
            let allow = payload.allow_segment_seek;
            let pos = payload.position_ms;
            // `recv_timeout` inside seek_to_segment_aware blocks; run it on a
            // blocking pool so it doesn't park an async executor thread.
            tokio::task::spawn_blocking(move || handle.seek_to_segment_aware(pos, allow))
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        }
        None => playback_runtime::SeekToOutcome::RejectedOutOfBuffer,
    };

    let snapshot = build_live_playback_snapshot(&state).await?;
    let _ = {
        let g = state.read().await;
        g.event_tx.send(AppEvent::PlaybackStateChanged)
    };

    match outcome {
        playback_runtime::SeekToOutcome::DispatchedCrossfadeSuppressed => {
            if let Err(error) =
                mark_armed_dj_transition_manual_seek_suppressed_if_needed(&state).await
            {
                warn!("Failed to suppress armed DJ transition after seek: {error}");
            }
            Ok((
                StatusCode::ACCEPTED,
                Json(json!({ "state": snapshot.state })),
            ))
        }
        playback_runtime::SeekToOutcome::Dispatched => Ok((
            StatusCode::ACCEPTED,
            Json(json!({ "state": snapshot.state })),
        )),
        playback_runtime::SeekToOutcome::RejectedOutOfBuffer => Ok((
            StatusCode::CONFLICT,
            Json(json!({ "state": snapshot.state })),
        )),
        playback_runtime::SeekToOutcome::Failed => Err(StatusCode::INTERNAL_SERVER_ERROR),
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
            if let Some(dn) = payload.discover_new {
                automix::set_automix_discover_new(conn, dn)?;
            }
            if let Some(use_learning) = payload.use_learning {
                automix::set_automix_use_learning(conn, use_learning)?;
            }
            if let Some(allow_external) = payload.allow_external {
                automix::set_automix_allow_external(conn, allow_external)?;
            }
            automix::set_automix_enabled(conn, payload.enabled)
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
    let response = {
        let state_guard = state.read().await;
        // User-driven enqueue; clear the post-clear suppression window.
        state_guard
            .user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
        state_guard
            .db
            .with_conn(|conn| {
                let queue = player::enqueue_track(conn, payload.track_id, "user")?;
                let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
                Ok(Json(json!({ "queue": queue })))
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    refresh_dj_after_queue_change(state, "add_queue_track").await;
    Ok(response)
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

/// Build an ephemeral TIDAL insert from a queue request, but only when it names a
/// streamable TIDAL track (kind=tidal with a positive tidal_id and a title).
/// Returns `None` for library/external requests, which can't stream by tidal_id,
/// so callers fall back to the normal pending/library insert path. Used to fold a
/// "Play next" / "Add to queue" of a TIDAL track into a live mix continuation.
fn ephemeral_tidal_insert(
    payload: &QueueExternalRequest,
) -> Option<queue::EphemeralTidalInsert<'_>> {
    if payload.kind != QueueExternalKind::Tidal {
        return None;
    }
    let tidal_id = payload.tidal_id.filter(|id| *id > 0)?;
    let title = payload
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    Some(queue::EphemeralTidalInsert {
        tidal_id,
        title,
        artist: payload
            .artist
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty()),
        album_title: payload.album_title.as_deref(),
        artwork_url: payload.artwork_url.as_deref(),
        duration_ms: payload.duration_ms,
        artist_tidal_id: payload.artist_tidal_id,
        album_tidal_id: payload.album_tidal_id,
    })
}

/// Owned form of [`queue::EphemeralTidalInsert`]. Needed because a library "Play
/// next" during a mix has to be looked up from the DB (the request carries only a
/// `track_id`), and the borrowed insert can't outlive that temporary lookup.
struct OwnedEphemeralInsert {
    tidal_id: i64,
    title: String,
    artist: Option<String>,
    album_title: Option<String>,
    artwork_url: Option<String>,
    duration_ms: Option<i64>,
    artist_tidal_id: Option<i64>,
    album_tidal_id: Option<i64>,
}

impl OwnedEphemeralInsert {
    fn as_insert(&self) -> queue::EphemeralTidalInsert<'_> {
        queue::EphemeralTidalInsert {
            tidal_id: self.tidal_id,
            title: &self.title,
            artist: self.artist.as_deref(),
            album_title: self.album_title.as_deref(),
            artwork_url: self.artwork_url.as_deref(),
            duration_ms: self.duration_ms,
            artist_tidal_id: self.artist_tidal_id,
            album_tidal_id: self.album_tidal_id,
        }
    }

    fn from_ephemeral_insert(insert: &queue::EphemeralTidalInsert<'_>) -> Self {
        OwnedEphemeralInsert {
            tidal_id: insert.tidal_id,
            title: insert.title.to_string(),
            artist: insert.artist.map(str::to_string),
            album_title: insert.album_title.map(str::to_string),
            artwork_url: insert.artwork_url.map(str::to_string),
            duration_ms: insert.duration_ms,
            artist_tidal_id: insert.artist_tidal_id,
            album_tidal_id: insert.album_tidal_id,
        }
    }

    /// Build an ephemeral insert from a resolved library track. Returns `None`
    /// when the track has no positive `tidal_id` (unplayable in a mix, which
    /// streams strictly by tidal id).
    fn from_track(track: &crate::db::models::Track) -> Option<Self> {
        let tidal_id = track.tidal_id.filter(|id| *id > 0)?;
        let title = track.title.trim();
        if title.is_empty() {
            return None;
        }
        Some(OwnedEphemeralInsert {
            tidal_id,
            title: title.to_string(),
            artist: track
                .artist_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            album_title: track.album_title.clone(),
            artwork_url: track.artwork_url.clone(),
            duration_ms: track.duration_ms,
            artist_tidal_id: track.artist_tidal_id,
            album_tidal_id: track.album_tidal_id,
        })
    }
}

/// During a live mix, resolve a queue request into an ephemeral (consumed-on-play)
/// insert. A TIDAL request uses its own metadata; a library request is looked up
/// so it, too, folds into the continuation and is popped when played instead of
/// lingering as a persistent `user_play_next` row (which the NULL-anchor advance
/// fallback re-selects, causing double-plays and "previous" re-loading it).
/// Returns `None` for external requests or library tracks without a tidal id, so
/// the caller falls back to the persistent insert path.
fn ephemeral_owned_for_request(
    conn: &rusqlite::Connection,
    payload: &QueueExternalRequest,
) -> anyhow::Result<Option<OwnedEphemeralInsert>> {
    if let Some(insert) = ephemeral_tidal_insert(payload) {
        return Ok(Some(OwnedEphemeralInsert::from_ephemeral_insert(&insert)));
    }
    if payload.kind == QueueExternalKind::Library
        && let Some(track_id) = payload.track_id.filter(|id| *id > 0)
        && let Some(track) = queue::get_track_by_id(conn, track_id)?
    {
        return Ok(OwnedEphemeralInsert::from_track(&track));
    }
    Ok(None)
}

/// Batch form of [`ephemeral_owned_for_request`], all-or-nothing: returns
/// `Some(inserts)` only when every item resolves to an ephemeral insert, so a
/// mixed batch (any external / non-streamable item) falls back to the persistent
/// path as one unit and stays in a single consumption model.
fn ephemeral_owned_for_requests(
    conn: &rusqlite::Connection,
    payloads: &[QueueExternalRequest],
) -> anyhow::Result<Option<Vec<OwnedEphemeralInsert>>> {
    let mut owned = Vec::with_capacity(payloads.len());
    for payload in payloads {
        match ephemeral_owned_for_request(conn, payload)? {
            Some(insert) => owned.push(insert),
            None => return Ok(None),
        }
    }
    Ok(Some(owned))
}

fn current_queue_position(conn: &rusqlite::Connection) -> anyhow::Result<Option<i32>> {
    let (current_queue_item_id, current_track_id): (Option<i64>, Option<i64>) = conn.query_row(
        "SELECT current_queue_item_id, current_track_id FROM playback_state WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    if let Some(queue_item_id) = current_queue_item_id {
        if queue_item_matches_current_track(conn, queue_item_id, current_track_id)?
            && let Some(position) = queue_item_position(conn, queue_item_id)?
        {
            return Ok(Some(position));
        }
    }

    if let Some(track_id) = current_track_id {
        if let Some((queue_item_id, position)) = first_queue_item_for_track(conn, track_id)? {
            conn.execute(
                "UPDATE playback_state SET current_queue_item_id = ?1 WHERE id = 1",
                params![queue_item_id],
            )?;
            return Ok(Some(position));
        }
    }

    if current_queue_item_id.is_some() {
        conn.execute(
            "UPDATE playback_state SET current_queue_item_id = NULL WHERE id = 1",
            [],
        )?;
    }

    Ok(None)
}

/// Classify what the next queue advance should consume: `Some(tidal_id)` when
/// the first upcoming row (after the current anchor, or the queue front during a
/// live mix where the anchor is NULL) is a playable ephemeral TIDAL row, `None`
/// when it is a regular/pending row or the queue is exhausted. The ephemeral
/// advance paths used to pop the lowest ephemeral row unconditionally, which
/// silently skipped a library "Play next" row sitting in front of the mix
/// continuation. Source list mirrors queue::EPHEMERAL_TIDAL_SOURCES.
fn next_advance_ephemeral_tidal_id(conn: &rusqlite::Connection) -> anyhow::Result<Option<i64>> {
    let anchor = current_queue_position(conn)?;
    let front: Option<(bool, Option<i64>)> = conn
        .query_row(
            "SELECT (track_id IS NULL
                     AND source IN ('tidal_mix','tidal_album','tidal_playlist')
                     AND tidal_id_hint IS NOT NULL),
                    tidal_id_hint
             FROM queue
             WHERE ?1 IS NULL OR position > ?1
             ORDER BY position ASC, id ASC
             LIMIT 1",
            params![anchor],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(front.and_then(|(is_ephemeral, hint)| if is_ephemeral { hint } else { None }))
}

/// Pop the next ephemeral mix row only when it is genuinely the next thing to
/// play. A regular row ahead of the continuation (library "Play next" during a
/// mix) must win the advance instead of being skipped.
fn pop_next_ephemeral_if_due(
    conn: &rusqlite::Connection,
) -> anyhow::Result<Option<crate::PendingEphemeralTidalTrack>> {
    if next_advance_ephemeral_tidal_id(conn)?.is_some() {
        queue::pop_next_ephemeral_tidal_track(conn)
    } else {
        Ok(None)
    }
}

/// The next ephemeral mix row in queue order, or `None` when a non-ephemeral row
/// is in front (so the continuation is not actually next) or the queue holds no
/// ephemeral row. Unlike a raw `peek_next_ephemeral_tidal_track` (lowest ephemeral
/// row, order-blind), this honours the same ordering as the advance paths, so the
/// DJ pre-buffer and pair display don't arm a crossfade into a skipped-over track.
fn next_advance_ephemeral_track(
    conn: &rusqlite::Connection,
) -> anyhow::Result<Option<crate::PendingEphemeralTidalTrack>> {
    match next_advance_ephemeral_tidal_id(conn)? {
        Some(tidal_id) => queue::find_ephemeral_tidal_track_by_tidal_id(conn, tidal_id),
        None => Ok(None),
    }
}

fn queue_item_position(
    conn: &rusqlite::Connection,
    queue_item_id: i64,
) -> anyhow::Result<Option<i32>> {
    Ok(conn
        .query_row(
            "SELECT position FROM queue WHERE id = ?1",
            params![queue_item_id],
            |row| row.get(0),
        )
        .optional()?)
}

fn first_queue_item_for_track(
    conn: &rusqlite::Connection,
    track_id: i64,
) -> anyhow::Result<Option<(i64, i32)>> {
    Ok(conn
        .query_row(
            "SELECT id, position
             FROM queue
             WHERE track_id = ?1
             ORDER BY position ASC, id ASC
             LIMIT 1",
            params![track_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

fn first_queue_item_id_for_track(
    conn: &rusqlite::Connection,
    track_id: i64,
) -> anyhow::Result<Option<i64>> {
    Ok(first_queue_item_for_track(conn, track_id)?.map(|(id, _)| id))
}

fn preserve_only_queue_item(conn: &rusqlite::Connection, queue_item_id: i64) -> anyhow::Result<()> {
    conn.execute("DELETE FROM queue WHERE id != ?1", params![queue_item_id])?;
    conn.execute(
        "UPDATE playback_state SET current_queue_item_id = ?1 WHERE id = 1",
        params![queue_item_id],
    )?;
    Ok(())
}

fn preserve_current_track_queue_row(
    conn: &rusqlite::Connection,
    track_id: i64,
) -> anyhow::Result<()> {
    if let Some(queue_item_id) = first_queue_item_id_for_track(conn, track_id)? {
        preserve_only_queue_item(conn, queue_item_id)?;
    } else {
        queue::clear_queue(conn)?;
        conn.execute(
            "UPDATE playback_state SET current_queue_item_id = NULL WHERE id = 1",
            [],
        )?;
    }
    Ok(())
}

fn queue_item_track_id(
    conn: &rusqlite::Connection,
    queue_item_id: i64,
) -> anyhow::Result<Option<Option<i64>>> {
    Ok(conn
        .query_row(
            "SELECT track_id FROM queue WHERE id = ?1",
            params![queue_item_id],
            |row| row.get(0),
        )
        .optional()?)
}

fn queue_item_matches_current_track(
    conn: &rusqlite::Connection,
    queue_item_id: i64,
    current_track_id: Option<i64>,
) -> anyhow::Result<bool> {
    Ok(queue_item_track_id(conn, queue_item_id)?
        .map(|track_id| track_id == current_track_id)
        .unwrap_or(false))
}

fn repair_moved_queue_current_anchor(
    conn: &rusqlite::Connection,
    moved_queue_item_id: i64,
) -> anyhow::Result<bool> {
    let (current_track_id, current_queue_item_id): (Option<i64>, Option<i64>) = conn.query_row(
        "SELECT current_track_id, current_queue_item_id FROM playback_state WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if let Some(queue_item_id) = current_queue_item_id
        && queue_item_matches_current_track(conn, queue_item_id, current_track_id)?
    {
        return Ok(false);
    }

    let repaired_queue_item_id = match current_track_id {
        Some(track_id) => {
            let moved_track_id = queue_item_track_id(conn, moved_queue_item_id)?.flatten();
            if moved_track_id == Some(track_id) {
                Some(moved_queue_item_id)
            } else {
                first_queue_item_id_for_track(conn, track_id)?
            }
        }
        None => None,
    };

    if repaired_queue_item_id == current_queue_item_id {
        return Ok(false);
    }

    conn.execute(
        "UPDATE playback_state SET current_queue_item_id = ?1 WHERE id = 1",
        params![repaired_queue_item_id],
    )?;
    Ok(true)
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
    let state = state.clone();
    tokio::spawn(async move {
        if resolve_pending_row(db, tokens, queue_item_id, event_tx, tidal_http_client).await {
            refresh_dj_after_queue_change(state, "pending_queue_resolved").await;
        }
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
        // During a live TIDAL mix, append the pick (TIDAL or a library track with
        // a tidal id) onto the ephemeral continuation so it plays after the rest of
        // the mix and is popped when done, instead of stranding a persistent
        // user_queue row the mix-advance pipeline never reads (and which lingers to
        // corrupt the NULL-anchor advance). See EPHEMERAL_USER_TIDAL_SOURCE.
        let ephemeral_active = state_guard.ephemeral_tidal_track.is_some()
            || state_guard.external_playback_track.is_some();
        let event_tx = state_guard.event_tx.clone();
        state_guard
            .db
            .with_conn(|conn| {
                let ephemeral_owned = if ephemeral_active {
                    ephemeral_owned_for_request(conn, &payload)?
                } else {
                    None
                };
                if let Some(owned) = ephemeral_owned.as_ref() {
                    let insert = owned.as_insert();
                    let queue = queue::append_ephemeral_tidal_tracks(
                        conn,
                        std::slice::from_ref(&insert),
                        queue::EPHEMERAL_USER_TIDAL_SOURCE,
                    )?;
                    let _ = event_tx.send(AppEvent::QueueUpdated);
                    return Ok((queue, None));
                }
                let inserted = queue::append_external_track(conn, &insert)?;
                let queue = queue::load_queue(conn)?;
                let _ = event_tx.send(AppEvent::QueueUpdated);
                Ok((queue, Some(inserted)))
            })
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "failed to append queue item" })),
                )
            })?
    };

    let pending_count = matches!(inserted, Some(queue::InsertResult::Pending { .. })) as usize;
    tracing::info!(
        target: "noor.playback.queue",
        event = "queue_append",
        item_count = 1,
        pending_count,
        "appended queue item"
    );

    if let Some(queue::InsertResult::Pending { queue_id }) = inserted {
        spawn_pending_queue_resolver(&state, queue_id).await;
    }
    refresh_dj_after_queue_change(state, "queue_append").await;

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
        // See queue_append: during a live mix, append the batch onto the ephemeral
        // continuation, but only when every item resolves to an ephemeral insert
        // (TIDAL pick or library track with a tidal id).
        let ephemeral_active = state_guard.ephemeral_tidal_track.is_some()
            || state_guard.external_playback_track.is_some();
        let event_tx = state_guard.event_tx.clone();
        state_guard
            .db
            .with_conn(|conn| {
                let ephemeral_owned = if ephemeral_active {
                    ephemeral_owned_for_requests(conn, &payload.items)?
                } else {
                    None
                };
                if let Some(owned) = ephemeral_owned.as_ref() {
                    let eph: Vec<_> = owned.iter().map(OwnedEphemeralInsert::as_insert).collect();
                    let queue = queue::append_ephemeral_tidal_tracks(
                        conn,
                        &eph,
                        queue::EPHEMERAL_USER_TIDAL_SOURCE,
                    )?;
                    let _ = event_tx.send(AppEvent::QueueUpdated);
                    return Ok((queue, Vec::new()));
                }
                let inserted = queue::append_external_tracks(conn, &inserts)?;
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

    let pending_count = inserted
        .iter()
        .filter(|item| matches!(**item, queue::InsertResult::Pending { .. }))
        .count();
    tracing::info!(
        target: "noor.playback.queue",
        event = "queue_append_many",
        item_count = inserts.len(),
        pending_count,
        "appended queue items"
    );

    for item in inserted {
        if let queue::InsertResult::Pending { queue_id } = item {
            spawn_pending_queue_resolver(&state, queue_id).await;
        }
    }
    refresh_dj_after_queue_change(state, "queue_append_many").await;

    Ok(Json(json!({ "queue": queue })))
}

/// Decide the `after_position` for a "Play next" insert. Returns `Some(pos)` to
/// insert right after `pos`, or `None` to append at the end of the queue.
///
/// - `current_pos`: position of the currently-playing queue row, if any
///   (`current_queue_position`).
/// - `front_pos`: the lowest position in the queue (`queue::front_position`).
/// - `ephemeral_active`: whether a TIDAL mix/external overlay is the live track.
///   During a mix the playing track has no queue row and the DB anchor is NULL,
///   so `current_pos` is `None` even though something is playing; "after current"
///   then means the front of the remaining continuation (`front - 1`), not the
///   bottom of the queue.
fn play_next_after_position(
    current_pos: Option<i32>,
    front_pos: Option<i32>,
    ephemeral_active: bool,
) -> Option<i32> {
    match (current_pos, ephemeral_active, front_pos) {
        (Some(pos), _, _) => Some(pos),
        (None, true, Some(front)) => Some(front - 1),
        _ => None,
    }
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
        // During an ephemeral TIDAL mix the playing track has no queue row and the
        // DB anchor is NULL, so current_queue_position() returns None. Without this
        // flag "Play next" would fall through to append-at-bottom; instead we
        // insert at the front of the remaining continuation (the visible top of
        // Up Next during a mix).
        let ephemeral_active = state_guard.ephemeral_tidal_track.is_some()
            || state_guard.external_playback_track.is_some();
        // During a live TIDAL mix, fold the pick (TIDAL or a library track with a
        // tidal id) into the ephemeral continuation so the mix-advance pipeline
        // plays it next AND pops it when done. A persistent user_play_next row is
        // invisible to that pipeline and, worse, lingers after playing: with the
        // NULL anchor during ephemeral playback the "pick first" advance fallback
        // re-selects it (double-play; "previous" re-loads it).
        let event_tx = state_guard.event_tx.clone();
        state_guard
            .db
            .with_conn(|conn| {
                let after = play_next_after_position(
                    current_queue_position(conn)?,
                    queue::front_position(conn)?,
                    ephemeral_active,
                );
                let ephemeral_owned = if ephemeral_active {
                    ephemeral_owned_for_request(conn, &payload)?
                } else {
                    None
                };
                if let Some(owned) = ephemeral_owned.as_ref() {
                    let insert = owned.as_insert();
                    let slice = std::slice::from_ref(&insert);
                    let queue = match after {
                        Some(after) => queue::insert_ephemeral_tidal_tracks_after(
                            conn,
                            slice,
                            after,
                            queue::EPHEMERAL_USER_TIDAL_SOURCE,
                        )?,
                        None => queue::append_ephemeral_tidal_tracks(
                            conn,
                            slice,
                            queue::EPHEMERAL_USER_TIDAL_SOURCE,
                        )?,
                    };
                    let _ = event_tx.send(AppEvent::QueueUpdated);
                    return Ok((queue, None));
                }
                let inserted = match after {
                    Some(after) => queue::insert_external_track_after(conn, &insert, after)?,
                    None => queue::append_external_track(conn, &insert)?,
                };
                let queue = queue::load_queue(conn)?;
                let _ = event_tx.send(AppEvent::QueueUpdated);
                Ok((queue, Some(inserted)))
            })
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "failed to insert queue item" })),
                )
            })?
    };

    let pending_count = matches!(inserted, Some(queue::InsertResult::Pending { .. })) as usize;
    tracing::info!(
        target: "noor.playback.queue",
        event = "queue_play_next",
        item_count = 1,
        pending_count,
        "inserted queue item after current"
    );

    if let Some(queue::InsertResult::Pending { queue_id }) = inserted {
        spawn_pending_queue_resolver(&state, queue_id).await;
    }
    refresh_dj_after_queue_change(state, "queue_play_next").await;

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
        // See queue_play_next: insert at the front of the continuation during an
        // ephemeral mix instead of appending to the bottom.
        let ephemeral_active = state_guard.ephemeral_tidal_track.is_some()
            || state_guard.external_playback_track.is_some();
        // Fold into the live mix only when every item resolves to an ephemeral
        // insert (TIDAL pick or library track with a tidal id), so the batch
        // stays in order under one consumption model and none of them linger as
        // persistent rows. A mixed batch falls back to the pending/library path.
        let event_tx = state_guard.event_tx.clone();
        state_guard
            .db
            .with_conn(|conn| {
                let after = play_next_after_position(
                    current_queue_position(conn)?,
                    queue::front_position(conn)?,
                    ephemeral_active,
                );
                let ephemeral_owned = if ephemeral_active {
                    ephemeral_owned_for_requests(conn, &payload.items)?
                } else {
                    None
                };
                if let Some(owned) = ephemeral_owned.as_ref() {
                    let eph: Vec<_> = owned.iter().map(OwnedEphemeralInsert::as_insert).collect();
                    let queue = match after {
                        Some(after) => queue::insert_ephemeral_tidal_tracks_after(
                            conn,
                            &eph,
                            after,
                            queue::EPHEMERAL_USER_TIDAL_SOURCE,
                        )?,
                        None => queue::append_ephemeral_tidal_tracks(
                            conn,
                            &eph,
                            queue::EPHEMERAL_USER_TIDAL_SOURCE,
                        )?,
                    };
                    let _ = event_tx.send(AppEvent::QueueUpdated);
                    return Ok((queue, Vec::new()));
                }
                let inserted = match after {
                    Some(after) => queue::insert_external_tracks_after(conn, &inserts, after)?,
                    None => queue::append_external_tracks(conn, &inserts)?,
                };
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

    let pending_count = inserted
        .iter()
        .filter(|item| matches!(**item, queue::InsertResult::Pending { .. }))
        .count();
    tracing::info!(
        target: "noor.playback.queue",
        event = "queue_play_next_many",
        item_count = inserts.len(),
        pending_count,
        "inserted queue items after current"
    );

    for item in inserted {
        if let queue::InsertResult::Pending { queue_id } = item {
            spawn_pending_queue_resolver(&state, queue_id).await;
        }
    }
    refresh_dj_after_queue_change(state, "queue_play_next_many").await;

    Ok(Json(json!({ "queue": queue })))
}

async fn replace_playback_queue(
    State(state): State<SharedState>,
    Json(payload): Json<QueueReplaceRequest>,
) -> Result<Json<Value>, StatusCode> {
    let response = {
        let state_guard = state.read().await;
        state_guard
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
                let mut shuffle_debug = None;
                let final_queue = match payload.shuffle_mode.as_deref() {
                    Some(raw_mode) => {
                        let mode = queue::ShuffleMode::parse(raw_mode);
                        if mode == queue::ShuffleMode::Off {
                            crate::playback::queue::load_queue(conn)?
                        } else {
                            let seed = crate::playback::shuffle::generate_shuffle_seed();
                            let result = crate::playback::queue::apply_shuffle_with_seed(
                                conn,
                                mode,
                                None,
                                seed,
                                "queue_replace",
                            )?;
                            shuffle_debug = result.debug;
                            result.queue
                        }
                    }
                    None => crate::playback::queue::load_queue(conn)?,
                };
                let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
                Ok(Json(json!({
                    "queue": final_queue,
                    "shuffle_debug": shuffle_debug
                })))
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    refresh_dj_after_queue_change(state, "replace_playback_queue").await;
    Ok(response)
}

async fn remove_queue_track(
    State(state): State<SharedState>,
    Json(payload): Json<QueueRemoveRequest>,
) -> Result<Json<Value>, StatusCode> {
    let outcome = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(|conn| player::remove_queue_item_and_reconcile(conn, payload.queue_item_id))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    let include_playback_state = outcome.removed_current;
    let mut snapshot = outcome.snapshot;
    if outcome.removed_current && outcome.was_playing {
        let playback_generation = bump_playback_generation(&state).await;
        snapshot = resolve_or_skip_pending_current(
            &state,
            snapshot,
            playback_generation,
            "remove_queue_track",
        )
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let end_reason = if snapshot.state.current_track.is_some() {
            Some(player::ListenSessionEndReason::Replaced)
        } else {
            Some(player::ListenSessionEndReason::QueueEnded)
        };
        sync_session_after_snapshot(&state, &snapshot, end_reason).await;
        if snapshot.state.current_track.is_none()
            && let Some(runtime_handle) = current_playback_runtime(&state).await
        {
            let _ = runtime_handle.stop();
        }
        switch_runtime_to_snapshot_current(&state, &snapshot, playback_generation)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    } else if outcome.removed_current {
        let state_guard = state.read().await;
        let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
        let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
    } else {
        let state_guard = state.read().await;
        let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
    }

    refresh_dj_after_queue_change(state.clone(), "remove_queue_track").await;
    let snapshot = overlay_snapshot_with_external_track(&state, snapshot).await;
    if include_playback_state {
        Ok(Json(json!({
            "queue": snapshot.queue,
            "playback_state": snapshot.state
        })))
    } else {
        Ok(Json(json!({ "queue": snapshot.queue })))
    }
}

async fn move_queue_track(
    State(state): State<SharedState>,
    Json(payload): Json<QueueMoveRequest>,
) -> Result<Json<Value>, StatusCode> {
    let response = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(|conn| {
                queue::move_queue_item(conn, payload.item_id, payload.new_pos)?;
                let repaired_anchor = repair_moved_queue_current_anchor(conn, payload.item_id)?;
                let snapshot = if repaired_anchor {
                    Some(player::load_snapshot(conn)?)
                } else {
                    None
                };
                let queue = match &snapshot {
                    Some(snapshot) => snapshot.queue.clone(),
                    None => queue::load_queue(conn)?,
                };
                if repaired_anchor {
                    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
                }
                let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
                Ok(match snapshot {
                    Some(snapshot) => Json(json!({
                        "queue": queue,
                        "playback_state": snapshot.state
                    })),
                    None => Json(json!({ "queue": queue })),
                })
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    refresh_dj_after_queue_change(state, "move_queue_track").await;
    Ok(response)
}

async fn clear_queue_route(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let response = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(|conn| {
                let (current_track_id, current_queue_item_id): (Option<i64>, Option<i64>) =
                    conn.query_row(
                        "SELECT current_track_id, current_queue_item_id FROM playback_state WHERE id = 1",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )?;
                match (current_queue_item_id, current_track_id) {
                    (Some(qid), track_id) => {
                        if queue_item_matches_current_track(conn, qid, track_id)? {
                            preserve_only_queue_item(conn, qid)?;
                        } else if let Some(track_id) = track_id {
                            preserve_current_track_queue_row(conn, track_id)?;
                        } else {
                            queue::clear_queue(conn)?;
                            conn.execute(
                                "UPDATE playback_state SET current_queue_item_id = NULL WHERE id = 1",
                                [],
                            )?;
                        }
                    }
                    (None, Some(track_id)) => {
                        preserve_current_track_queue_row(conn, track_id)?;
                    }
                    (None, None) => {
                        queue::clear_queue(conn)?;
                    }
                }
                queue::delete_all_ephemeral_tidal_rows(conn)?;
                // Return the full PlaybackSnapshot ({state, queue}) so the UI can
                // refresh both at once - additive over the prior `{queue}` shape:
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
                state_guard
                    .user_cleared_at
                    .store(now_secs, std::sync::atomic::Ordering::Relaxed);
                let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
                Ok(Json(json!({
                    "queue": snapshot.queue,
                    "playback_state": snapshot.state,
                })))
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    refresh_dj_after_queue_change(state, "clear_queue_route").await;
    Ok(response)
}

fn non_empty_or_default(value: Option<String>, fallback: &str) -> String {
    value
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn optional_non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn visible_track_playlist_source(
    track: &crate::db::models::Track,
    include_tidal_only: bool,
) -> Option<PlaylistFromQueueSource> {
    if track.id > 0 && track.source != "tidal_ephemeral" {
        if include_tidal_only
            || (track.source.as_str() != "tidal_stream"
                && track.source.as_str() != "tidal_ephemeral")
        {
            return Some(PlaylistFromQueueSource::Local(track.id));
        }
    }

    if !include_tidal_only {
        return None;
    }

    track
        .tidal_id
        .filter(|tidal_id| *tidal_id > 0)
        .map(|tidal_id| {
            PlaylistFromQueueSource::Tidal(tidal_import::ImportTrackMetadata {
                tidal_id,
                title: non_empty_or_default(Some(track.title.clone()), "Unknown title"),
                artist_name: non_empty_or_default(track.artist_name.clone(), "Unknown artist"),
                album_title: optional_non_empty(track.album_title.clone()),
                album_artwork_url: track.artwork_url.clone(),
                duration_ms: track.duration_ms,
                ..Default::default()
            })
        })
}

fn load_persisted_queue_playlist_sources(
    conn: &rusqlite::Connection,
    include_tidal_only: bool,
) -> anyhow::Result<Vec<PlaylistFromQueueSource>> {
    let mut stmt = conn.prepare(
        "SELECT q.track_id, q.pending_artist, q.pending_title, q.tidal_id_hint,
                COALESCE(t.source, '')
         FROM queue q
         LEFT JOIN tracks t ON q.track_id = t.id
         ORDER BY q.position ASC, q.id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, Option<i64>>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;

    let mut sources = Vec::new();
    for row in rows {
        let (track_id, pending_artist, pending_title, tidal_id_hint, track_source) = row?;
        if let Some(track_id) = track_id.filter(|id| *id > 0) {
            if include_tidal_only
                || (track_source.as_str() != "tidal_stream"
                    && track_source.as_str() != "tidal_ephemeral")
            {
                sources.push(PlaylistFromQueueSource::Local(track_id));
            }
            continue;
        }

        if include_tidal_only && let Some(tidal_id) = tidal_id_hint.filter(|id| *id > 0) {
            sources.push(PlaylistFromQueueSource::Tidal(
                tidal_import::ImportTrackMetadata {
                    tidal_id,
                    title: non_empty_or_default(pending_title, "Unknown title"),
                    artist_name: non_empty_or_default(pending_artist, "Unknown artist"),
                    ..Default::default()
                },
            ));
        }
    }

    Ok(sources)
}

async fn resolve_playlist_source_ids(
    db: &crate::db::Database,
    sources: Vec<PlaylistFromQueueSource>,
) -> anyhow::Result<Vec<i64>> {
    let mut track_ids = Vec::with_capacity(sources.len());
    for source in sources {
        match source {
            PlaylistFromQueueSource::Local(track_id) => track_ids.push(track_id),
            PlaylistFromQueueSource::Tidal(meta) => {
                let imported = tidal_import::import_track_from_metadata(db, meta).await?;
                track_ids.push(imported.local_id);
            }
        }
    }
    Ok(track_ids)
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
    let (db, current_source) = {
        let state = state.read().await;
        // The currently-playing ephemeral track isn't a queue row; the rest of
        // the mix is, and load_persisted_queue_playlist_sources surfaces those
        // ephemeral rows via their tidal_id_hint below.
        let current = state
            .ephemeral_tidal_track
            .as_ref()
            .or(state.external_playback_track.as_ref())
            .and_then(|track| visible_track_playlist_source(track, include_tidal_only));
        (state.db.clone(), current)
    };

    let persisted_sources = db
        .with_conn(|conn| load_persisted_queue_playlist_sources(conn, include_tidal_only))
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Failed to read queue: {e}") })),
            )
        })?;
    let mut sources = Vec::new();
    sources.extend(current_source);
    sources.extend(persisted_sources);

    let track_ids = resolve_playlist_source_ids(&db, sources)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Failed to import queued TIDAL tracks: {e}") })),
            )
        })?;
    if track_ids.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Queue has no tracks that can be saved" })),
        ));
    }

    db.with_conn(|conn| {
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

// --- TIDAL Endpoints --------------------------------------

/// Start PKCE login flow. Returns a browser URL. The user must paste the
/// redirected TIDAL URL into the completion endpoint after signing in.
async fn tidal_login(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    let login = tidal_auth::start_pkce_login().map_err(|e| {
        tracing::error!("TIDAL PKCE login error: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Cancel any previous in-flight login polling
    {
        let mut s = state.write().await;
        s.tidal_login_cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        s.tidal_login_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    }

    Ok(Json(json!({
        "mode": "pkce",
        "verify_url": login.verify_url,
        "requires_redirect_url": true,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct TidalLoginCompletePayload {
    redirect_url: String,
}

async fn tidal_login_complete(
    State(state): State<SharedState>,
    Json(payload): Json<TidalLoginCompletePayload>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let http = {
        let s = state.read().await;
        s.http_client.clone()
    };

    let tokens = tidal_auth::complete_pkce_login(&http, &payload.redirect_url)
        .await
        .map_err(|e| {
            tracing::error!("TIDAL PKCE completion error: {}", e);
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("TIDAL login failed: {e}") })),
            )
        })?;

    persist_tidal_tokens(&state, &tokens).await.map_err(|e| {
        tracing::error!("TIDAL token persist error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "Failed to persist TIDAL login" })),
        )
    })?;
    {
        let mut s = state.write().await;
        s.tidal_tokens = Some(tokens.clone());
        let _ = s.event_tx.send(AppEvent::PlaybackStateChanged);
    }

    Ok(Json(json!({
        "status": "authenticated",
        "user_id": tokens.user_id,
        "country_code": tokens.country_code,
        "auth_flow": tokens.auth_flow,
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

pub(super) async fn load_persisted_tidal_tokens(
    state: &SharedState,
) -> anyhow::Result<Option<tidal_auth::TidalTokens>> {
    let (db, master_key) = {
        let s = state.read().await;
        (s.db.clone(), s.master_key.clone())
    };

    let loaded = db.with_conn(|conn| {
        let result = conn.query_row(
            "SELECT access_token_enc FROM service_auth WHERE service='tidal'",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        );

        Ok(match result {
            Ok(bytes) => tidal_auth::decode_persisted_tidal_tokens(&master_key, &bytes)?,
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(error) => return Err(error.into()),
        })
    })?;

    let Some(loaded) = loaded else {
        return Ok(None);
    };
    let needs_rewrite = loaded.needs_encrypted_rewrite();
    let tokens = loaded.into_tokens();
    if needs_rewrite {
        let blob = tidal_auth::encode_persisted_tidal_tokens(&master_key, &tokens)?;
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE service_auth SET access_token_enc = ?1 WHERE service = 'tidal'",
                params![blob],
            )?;
            Ok(())
        })?;
    }

    {
        let mut s = state.write().await;
        s.tidal_tokens = Some(tokens.clone());
    }

    Ok(Some(tokens))
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

    if let Some(mut tokens) = tokens {
        if !tokens.is_pkce() {
            tidal_auth::warn_if_fallback_client_credentials();
        }
        let mut expired = tidal_tokens_locally_expired(&state, &tokens)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!("Failed to inspect persisted TIDAL token expiry: {error}");
                false
            });
        if expired && !tokens.refresh_token.trim().is_empty() {
            let http_client = {
                let s = state.read().await;
                s.http_client.clone()
            };
            match recover_tidal_session(&state, &http_client, &tokens).await {
                Ok(refreshed) => {
                    tokens = refreshed;
                    expired = false;
                }
                Err(error) => {
                    tracing::warn!("Failed to refresh expired TIDAL session for status: {error}");
                }
            }
        }
        Json(tidal_status_payload(
            Some(&tokens),
            expired,
            tidal_auth::tidal_pkce_client_credential_source(),
            tidal_auth::tidal_client_credential_source(),
        ))
    } else {
        Json(tidal_status_payload(
            None,
            false,
            tidal_auth::tidal_pkce_client_credential_source(),
            tidal_auth::tidal_client_credential_source(),
        ))
    }
}

fn tidal_status_payload(
    tokens: Option<&tidal_auth::TidalTokens>,
    token_expired: bool,
    pkce_source: tidal_auth::TidalCredentialSource,
    legacy_source: tidal_auth::TidalCredentialSource,
) -> Value {
    let Some(tokens) = tokens else {
        return json!({ "connected": false });
    };
    let auth_flow = tokens.auth_flow.as_deref().unwrap_or("legacy");
    if token_expired {
        return json!({
            "connected": false,
            "reason": "token_expired",
            "user_id": tokens.user_id,
            "country_code": tokens.country_code,
            "auth_flow": auth_flow,
        });
    }
    let mut body = json!({
        "connected": true,
        "user_id": tokens.user_id,
        "country_code": tokens.country_code,
        "auth_flow": auth_flow,
    });
    if let Some(map) = body.as_object_mut() {
        if auth_flow == "pkce" {
            map.insert(
                "pkce_client_credential_source".to_string(),
                json!(pkce_source.as_str()),
            );
        } else {
            map.insert(
                "legacy_client_credential_source".to_string(),
                json!(legacy_source.as_str()),
            );
        }
    }
    body
}

async fn tidal_tokens_locally_expired(
    state: &SharedState,
    tokens: &tidal_auth::TidalTokens,
) -> anyhow::Result<bool> {
    let db = {
        let s = state.read().await;
        s.db.clone()
    };
    let record = db.with_conn(|conn| {
        let result = conn.query_row(
            "SELECT token_expiry, connected_at FROM service_auth WHERE service='tidal'",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        );
        match result {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error.into()),
        }
    })?;
    let Some((token_expiry, connected_at)) = record else {
        return Ok(false);
    };
    Ok(tidal_token_expired_at(
        token_expiry.as_deref(),
        connected_at.as_deref(),
        tokens.expires_in,
        chrono::Utc::now(),
    ))
}

fn tidal_token_expired_at(
    token_expiry: Option<&str>,
    connected_at: Option<&str>,
    expires_in: i64,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    let expiry = token_expiry.and_then(parse_service_auth_time).or_else(|| {
        let connected_at = connected_at.and_then(parse_service_auth_time)?;
        Some(connected_at + chrono::Duration::seconds(expires_in.max(0)))
    });
    expiry
        .map(|expiry| expiry <= now + chrono::Duration::seconds(60))
        .unwrap_or(false)
}

fn parse_service_auth_time(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(trimmed)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|dt| {
                    chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(dt, chrono::Utc)
                })
        })
}

/// Clear TIDAL session (logout).
async fn tidal_logout(State(state): State<SharedState>) -> Json<Value> {
    tracing::info!(target: "noor.sync.tidal", event = "session_logout", "TIDAL session cleared by user");
    let _ = clear_tidal_session(&state).await;
    Json(json!({ "status": "logged_out" }))
}

// --- TIDAL Search -------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TidalSearchParams {
    q: String,
    limit: Option<i32>,
    offset: Option<i32>,
}

const TIDAL_SEARCH_DEFAULT_LIMIT: i32 = 20;
const TIDAL_SEARCH_MAX_LIMIT: i32 = 50;

fn normalize_tidal_search_query(query: &str) -> Option<&str> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn normalize_tidal_search_limit(limit: Option<i32>) -> i32 {
    limit
        .unwrap_or(TIDAL_SEARCH_DEFAULT_LIMIT)
        .clamp(1, TIDAL_SEARCH_MAX_LIMIT)
}

fn empty_tidal_search_response() -> Json<Value> {
    Json(json!({
        "tracks": [],
        "albums": [],
        "artists": [],
        "videos": [],
    }))
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

async fn search_tidal_catalog_with_timeout(
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

async fn tidal_search(
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
        s.tidal_tokens.clone().or(persisted)
    };

    let Some(tokens) = tokens else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let limit = normalize_tidal_search_limit(params.limit);
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

    // Cache check - best-effort. A read failure must NOT block the upstream call.
    let cached = db
        .with_conn(|conn| {
            crate::services::tidal::cache::get_search(conn, &cache_cfg, query, limit, offset)
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
        let fetched = match search_tidal_catalog_with_timeout(&client, query, limit, offset).await {
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
                search_tidal_catalog_with_timeout(&retry_client, query, limit, offset)
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
        // Best-effort cache write - log and continue on failure.
        let to_cache = fetched.clone();
        let q_owned = query.to_string();
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

// --- TIDAL Playlist Search + Tracks -------------------------------------------

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

const TIDAL_VIDEO_MIX_ID_MAX_LEN: usize = 96;

fn normalize_tidal_video_mix_id(id: &str) -> Result<&str, StatusCode> {
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
    let (http_client, tidal_http_client) = {
        let s = state.read().await;
        (s.http_client.clone(), s.tidal_http_client.clone())
    };
    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let videos = match client.search_videos(query, limit, offset).await {
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
                .search_videos(query, limit, offset)
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

const TIDAL_VIDEO_SEARCH_DEFAULT_LIMIT: i32 = 20;
const TIDAL_VIDEO_SEARCH_MAX_LIMIT: i32 = 50;

fn normalize_tidal_video_search_query(query: &str) -> Option<&str> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn normalize_tidal_video_search_limit(limit: Option<i32>) -> i32 {
    limit
        .unwrap_or(TIDAL_VIDEO_SEARCH_DEFAULT_LIMIT)
        .clamp(1, TIDAL_VIDEO_SEARCH_MAX_LIMIT)
}

fn tidal_track_artwork_url(t: &TidalTrack, size: i32) -> Option<String> {
    t.album
        .as_ref()
        .and_then(|al| al.cover.as_ref())
        .and_then(|c| TidalClient::get_artwork_url(&Some(c.clone()), size))
}

pub(super) fn tidal_track_playable_json(
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

fn lookup_tidal_track_library_state(
    db: &crate::db::Database,
    tidal_id: i64,
) -> Option<queries::TidalTrackLibraryState> {
    db.with_conn(|conn| queries::get_tidal_track_library_states(conn, &[tidal_id]))
        .ok()
        .and_then(|states| states.get(&tidal_id).copied())
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
    if video_id <= 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Expected a positive TIDAL video id" })),
        ));
    }

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

    let (http_client, tidal_http_client) = {
        let s = state.read().await;
        (s.http_client.clone(), s.tidal_http_client.clone())
    };
    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let items = match client.get_video_mix_items(mix_id).await {
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
                .get_video_mix_items(mix_id)
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

async fn tidal_video_playlist_items(
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

    let (http_client, tidal_http_client) = {
        let s = state.read().await;
        (s.http_client.clone(), s.tidal_http_client.clone())
    };
    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let items = match client.get_playlist_video_items(uuid).await {
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
                .get_playlist_video_items(uuid)
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

const TIDAL_PLAYLIST_SEARCH_DEFAULT_LIMIT: i32 = 20;
const TIDAL_PLAYLIST_SEARCH_MAX_LIMIT: i32 = 50;
const TIDAL_PLAYLIST_UUID_MAX_LEN: usize = 96;

fn normalize_tidal_playlist_search_query(query: &str) -> Option<&str> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn normalize_tidal_playlist_search_limit(limit: Option<i32>) -> i32 {
    limit
        .unwrap_or(TIDAL_PLAYLIST_SEARCH_DEFAULT_LIMIT)
        .clamp(1, TIDAL_PLAYLIST_SEARCH_MAX_LIMIT)
}

fn normalize_tidal_playlist_uuid(uuid: &str) -> Result<&str, StatusCode> {
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

async fn tidal_playlist_search(
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
        s.tidal_tokens.clone().or(persisted)
    };
    let Some(tokens) = tokens else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let limit = normalize_tidal_playlist_search_limit(params.limit);
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
    let playlists = match client.search_playlists(query, limit, offset).await {
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
                .search_playlists(query, limit, offset)
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
        s.tidal_tokens.clone().or(persisted)
    };
    let Some(tokens) = tokens else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "TIDAL not connected" })),
        ));
    };

    let (http_client, tidal_http_client, playlist_tracks_cache) = {
        let s = state.read().await;
        (
            s.http_client.clone(),
            s.tidal_http_client.clone(),
            s.tidal_playlist_tracks_cache.clone(),
        )
    };
    let limit = 100;
    let offset = 0;
    let cache_key = tidal_playlist_tracks_cache_key(&tokens.country_code, uuid, limit, offset);
    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let tracks = match get_cached_tidal_playlist_tracks(&playlist_tracks_cache, &cache_key) {
        Some(cached) => cached,
        None => {
            let resp = match client.get_playlist_tracks(uuid, limit, offset).await {
                Ok(r) => r,
                Err(e) if error_looks_like_auth(&e) => {
                    let refreshed = recover_tidal_session(&state, &http_client, &tokens)
                        .await
                        .map_err(|re| {
                            (
                                StatusCode::BAD_GATEWAY,
                                Json(json!({
                                    "error": format!("TIDAL session refresh failed: {}", re)
                                })),
                            )
                        })?;
                    let retry_client = TidalClient::with_http(
                        tidal_http_client,
                        refreshed.access_token.clone(),
                        refreshed.country_code.clone(),
                    );
                    retry_client
                        .get_playlist_tracks(uuid, limit, offset)
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

fn tidal_playlist_tracks_cache_key(
    country_code: &str,
    uuid: &str,
    limit: i32,
    offset: i32,
) -> String {
    format!("{country_code}:{uuid}:{limit}:{offset}")
}

fn get_cached_tidal_playlist_tracks(
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

fn put_cached_tidal_playlist_tracks(
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

#[derive(Debug, serde::Deserialize)]
struct PlayTidalRequest {
    tidal_track_id: i64,
    title: String,
    artist_name: Option<String>,
    album_title: Option<String>,
    artwork_url: Option<String>,
    duration_ms: Option<i64>,
    // TIDAL artist/album ids so the synthetic now-playing track and queued
    // continuation rows keep clickable artist/album links. Sent by the frontend
    // for every mix/playlist/album launch; defaulted for older callers.
    #[serde(default)]
    artist_tidal_id: Option<i64>,
    #[serde(default)]
    album_tidal_id: Option<i64>,
}

async fn play_tidal_ephemeral(
    State(state): State<SharedState>,
    Json(body): Json<PlayTidalRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    // If the picked track is an ephemeral mix row, jump to it: drop rows before
    // it (and the row itself, which we're about to start) and leave the rest
    // queued. When it isn't part of the mix, clear the whole continuation.
    {
        let s = state.read().await;
        let tidal_id = body.tidal_track_id;
        let _ = s.db.with_conn(|conn| {
            let trimmed = queue::trim_ephemeral_tidal_rows_through_tidal_id(conn, tidal_id)?;
            if !trimmed {
                queue::delete_all_ephemeral_tidal_rows(conn)?;
            }
            Ok::<_, anyhow::Error>(())
        });
    }
    let track = crate::PendingEphemeralTidalTrack {
        tidal_track_id: body.tidal_track_id,
        title: body.title,
        artist_name: body.artist_name,
        album_title: body.album_title,
        artwork_url: body.artwork_url,
        duration_ms: body.duration_ms,
        artist_tidal_id: body.artist_tidal_id,
        album_tidal_id: body.album_tidal_id,
    };
    start_ephemeral_tidal_playback(&state, track).await?;
    let snapshot = build_live_playback_snapshot_json(&state).await?;
    {
        let s = state.read().await;
        let _ = s.event_tx.send(AppEvent::QueueUpdated);
    }
    Ok(Json(json!({
        "ok": true,
        "state": snapshot.state,
        "queue": snapshot.queue
    })))
}

#[derive(Debug, serde::Deserialize)]
struct PlayTidalMixRequest {
    tracks: Vec<PlayTidalRequest>,
    #[serde(default)]
    shuffle_mode: Option<String>,
    /// Collection kind for the queue rows: tidal_mix (default), tidal_album, or
    /// tidal_playlist. Drives the queue source label; behavior is identical.
    #[serde(default)]
    source: Option<String>,
}

/// Delete the persisted ephemeral TIDAL continuation rows. Used on stop and when
/// a failed mix start needs to leave no orphaned rows.
async fn clear_ephemeral_tidal_continuation(state: &SharedState) {
    let s = state.read().await;
    if let Err(error) =
        s.db.with_conn(|conn| Ok::<_, anyhow::Error>(queue::delete_all_ephemeral_tidal_rows(conn)?))
    {
        warn!(?error, "Failed to clear ephemeral TIDAL continuation rows");
    }
}

fn shuffle_tidal_mix_tracks(
    tracks: &mut [PlayTidalRequest],
    shuffle_mode: Option<&str>,
) -> Option<queue::ShuffleDebug> {
    let mode = queue::ShuffleMode::parse(shuffle_mode.unwrap_or("off"));
    if mode == queue::ShuffleMode::Off {
        return None;
    }

    let seed = crate::playback::shuffle::generate_shuffle_seed();
    let mut rng = crate::playback::shuffle::seeded_rng(seed, mode.as_str(), "tidal_mix");
    use rand::seq::SliceRandom;
    tracks.shuffle(&mut rng);
    Some(queue::ShuffleDebug {
        mode: mode.as_str().to_string(),
        seed,
        scope: "tidal_mix".to_string(),
        locked_count: 0,
        candidate_count: tracks.len(),
    })
}

async fn clear_persisted_queue_for_tidal_mix(
    state: &SharedState,
) -> Result<(), (StatusCode, Json<Value>)> {
    let state_guard = state.read().await;
    state_guard
        .db
        .with_conn(|conn| {
            queue::clear_queue(conn)?;
            // Drop any dangling play-head anchor so a reused queue rowid can't be
            // mistaken for the current item once new ephemeral rows are inserted.
            conn.execute(
                "UPDATE playback_state SET current_queue_item_id = NULL WHERE id = 1",
                [],
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .map_err(|error| {
            warn!(
                ?error,
                "Failed to clear stale persisted queue for TIDAL mix playback"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to clear stale playback queue" })),
            )
        })
}

/// Map a producer-supplied collection source to one of
/// [`queue::EPHEMERAL_TIDAL_SOURCES`], defaulting to `tidal_mix`.
fn normalize_ephemeral_tidal_source(source: Option<&str>) -> String {
    match source {
        Some(s) if queue::EPHEMERAL_TIDAL_SOURCES.contains(&s) => s.to_string(),
        _ => "tidal_mix".to_string(),
    }
}

/// Play the first track immediately and stash the rest in the pending
/// ephemeral queue so `handle_runtime_finished` can advance through them.
/// Used by the home Your Mixes shelf when a tile is clicked.
async fn play_tidal_mix(
    State(state): State<SharedState>,
    Json(body): Json<PlayTidalMixRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut tracks = body.tracks;
    if tracks.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Mix has no tracks" })),
        ));
    }
    let source = normalize_ephemeral_tidal_source(body.source.as_deref());
    let shuffle_debug = shuffle_tidal_mix_tracks(&mut tracks, body.shuffle_mode.as_deref());

    let first = crate::PendingEphemeralTidalTrack {
        tidal_track_id: tracks[0].tidal_track_id,
        title: tracks[0].title.clone(),
        artist_name: tracks[0].artist_name.clone(),
        album_title: tracks[0].album_title.clone(),
        artwork_url: tracks[0].artwork_url.clone(),
        duration_ms: tracks[0].duration_ms,
        artist_tidal_id: tracks[0].artist_tidal_id,
        album_tidal_id: tracks[0].album_tidal_id,
    };
    let first_tidal_id = first.tidal_track_id;

    // Wipe any prior queue + continuation first so a failed start leaves a clean
    // slate, then play the first track and persist the rest as real, mutable
    // ephemeral rows (not an in-memory deque).
    clear_persisted_queue_for_tidal_mix(&state).await?;
    if let Err(error) = start_ephemeral_tidal_playback(&state, first).await {
        clear_ephemeral_tidal_continuation(&state).await;
        return Err(error);
    }
    let rest_inserts: Vec<queue::EphemeralTidalInsert<'_>> = tracks[1..]
        .iter()
        .map(|t| queue::EphemeralTidalInsert {
            tidal_id: t.tidal_track_id,
            title: &t.title,
            artist: t.artist_name.as_deref(),
            album_title: t.album_title.as_deref(),
            artwork_url: t.artwork_url.as_deref(),
            duration_ms: t.duration_ms,
            artist_tidal_id: t.artist_tidal_id,
            album_tidal_id: t.album_tidal_id,
        })
        .collect();
    if !rest_inserts.is_empty() {
        let s = state.read().await;
        if let Err(error) = s.db.with_conn(|conn| {
            queue::append_ephemeral_tidal_tracks(conn, &rest_inserts, &source)?;
            Ok::<_, anyhow::Error>(())
        }) {
            // The first track is already playing; a continuation-persist failure
            // is soft - the mix just won't auto-advance. Don't abort playback.
            warn!(?error, "Failed to persist TIDAL mix continuation rows");
        }
    }

    let snapshot = build_live_playback_snapshot_json(&state).await?;
    {
        let s = state.read().await;
        let _ = s.event_tx.send(AppEvent::QueueUpdated);
    }
    Ok(Json(json!({
        "ok": true,
        "first_tidal_id": first_tidal_id,
        "shuffle_debug": shuffle_debug,
        "state": snapshot.state,
        "queue": snapshot.queue
    })))
}

fn requested_tidal_quality(
    user_quality: Option<crate::db::audio_settings::AudioQuality>,
    fallback_quality: Option<&str>,
) -> String {
    user_quality
        .map(|quality| quality.as_tidal_str().to_string())
        .or_else(|| fallback_quality.map(str::to_string))
        .unwrap_or_else(|| tidal_stream::DEFAULT_AUDIO_QUALITY.to_string())
}

fn build_ephemeral_tidal_stream_request(
    tidal_track_id: i64,
    user_quality: Option<crate::db::audio_settings::AudioQuality>,
) -> tidal_stream::StreamRequest {
    tidal_stream::StreamRequest::new(tidal_track_id, requested_tidal_quality(user_quality, None))
}

fn build_ephemeral_synthetic_track(
    track: &crate::PendingEphemeralTidalTrack,
    stream_info: &tidal_stream::StreamInfo,
    library_state: Option<queries::TidalTrackLibraryState>,
) -> crate::db::models::Track {
    let local_id = library_state
        .map(|state| state.local_id)
        .unwrap_or(-track.tidal_track_id);
    crate::db::models::Track {
        id: local_id,
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
        artist_tidal_id: track.artist_tidal_id,
        album_tidal_id: track.album_tidal_id,
        ytmusic_id: None,
        soundcloud_id: None,
        best_quality: Some(stream_info.audio_quality.clone()),
        best_source: Some("tidal".to_string()),
        fidelity_score: 0,
        is_favorite: library_state
            .map(|state| state.is_favorite)
            .unwrap_or(false),
        play_count: 0,
        last_played_at: None,
        date_added: None,
        source: "tidal_ephemeral".to_string(),
        artwork_url: track.artwork_url.clone(),
    }
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

    // Backstop for callers that don't ship full metadata. Some surfaces build a
    // playable with only id/title/artist: Spotify-resolved playlist tracks below
    // the fold never trigger the lazy IntersectionObserver so they arrive with
    // `artwork_url: null`, and several launch surfaces omit `album_title`
    // entirely. Either gap would leave the now-playing card blank. Look up the
    // TIDAL track once and fill whichever fields are missing from its album.
    let mut track = track;
    if track.artwork_url.is_none() || track.album_title.is_none() {
        let lookup_client = TidalClient::with_http(
            tidal_http_client.clone(),
            tokens.access_token.clone(),
            tokens.country_code.clone(),
        );
        if let Ok(t) = lookup_client.get_track(track.tidal_track_id).await {
            if track.artwork_url.is_none() {
                track.artwork_url = t
                    .album
                    .as_ref()
                    .and_then(|a| a.cover.as_ref())
                    .and_then(|c| TidalClient::get_artwork_url(&Some(c.clone()), 640));
            }
            if track.album_title.is_none() {
                track.album_title = t.album.as_ref().map(|a| a.title.clone());
            }
        }
    }

    let user_quality = current_user_audio_quality(state).await;
    let stream_req =
        build_ephemeral_tidal_stream_request(track.tidal_track_id, user_quality.clone());
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

    let library_state = {
        let s = state.read().await;
        lookup_tidal_track_library_state(&s.db, track.tidal_track_id)
    };
    let synthetic = build_ephemeral_synthetic_track(&track, &stream_info, library_state);

    let playback_generation = bump_playback_generation(state).await;
    let snapshot = {
        let mut state_guard = state.write().await;
        state_guard.external_playback_track = None;
        state_guard.ephemeral_tidal_track = Some(synthetic.clone());
        state_guard.prepared_ephemeral_tidal_next = None;
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
    let dj_engine_enabled = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(queries::is_dj_engine_enabled)
            .unwrap_or(false)
    };
    let mut job = player::build_playback_preparation(
        &synthetic,
        Some(&stream_info),
        crossfade_ms,
        user_quality,
    )
    .with_generation(playback_generation);
    if dj_engine_enabled
        && let Some(media_ref) =
            crate::playback::dj_lookahead::tidal_media_ref_for_track(&synthetic)
    {
        job = job.with_dj_media_ref(media_ref);
    }
    let runtime_handle = match ensure_playback_runtime_for_track(state, &synthetic).await {
        Ok(handle) => handle,
        Err(error) => {
            let state_guard = state.read().await;
            let _ = state_guard.db.with_conn(player::pause);
            return Err(error);
        }
    };
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
    if dj_engine_enabled {
        let lookahead = {
            let state_guard = state.read().await;
            active_dj_lookahead_start_for_state(&state_guard)
        };
        if let Some(lookahead) = lookahead {
            let _ = lookahead.dispatch(&runtime_handle);
            queue_missing_dj_profiles_after_pair_change(state.clone(), "ephemeral_tidal_mix_start")
                .await;
        }
    }

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
    if tidal_artist_id <= 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Expected a positive TIDAL artist id" })),
        ));
    }

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

    // Same rich payload the library `/api/artists/{id}/discography` route
    // builds, keyed straight off the TIDAL id (no local artist row). This is
    // what lets a non-library artist page render identically to a library one:
    // bio, similar artists, videos, and categorized releases instead of a bare
    // top-tracks-and-albums stub.
    let payload =
        catalog_routes::build_tidal_artist_payload(&state, &client, tidal_artist_id, &tokens).await;
    Ok(Json(payload))
}

pub(super) async fn recover_tidal_session(
    state: &SharedState,
    http: &reqwest::Client,
    tokens: &tidal_auth::TidalTokens,
) -> anyhow::Result<tidal_auth::TidalTokens> {
    if tokens.refresh_token.trim().is_empty() {
        anyhow::bail!("TIDAL session has no refresh token; reconnect TIDAL");
    }
    tracing::info!(
        target: "noor.sync.tidal",
        event = "session_refresh_start",
        user_id = %tokens.user_id,
        "Refreshing TIDAL session"
    );
    let mut refreshed =
        tidal_auth::refresh_token(http, &tokens.refresh_token, tokens.auth_flow.as_deref()).await?;
    if refreshed.user_id.is_empty() {
        refreshed.user_id = tokens.user_id.clone();
    }
    if refreshed.country_code.is_empty() {
        refreshed.country_code = tokens.country_code.clone();
    }
    if refreshed.auth_flow.is_none() {
        refreshed.auth_flow = tokens.auth_flow.clone();
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

/// Refresh the TIDAL session and hand back a client primed with the new access
/// token. Wraps `recover_tidal_session` + the `TidalClient::with_http` rebuild
/// so the handlers that retry after a 401 don't each re-implement it.
///
/// Single-flight re-check: TIDAL can rotate the refresh token on use, so a
/// burst of requests that all 401 at once must not each fire their own refresh
/// (the losers would hit `invalid_grant`). Before refreshing we re-read the
/// in-memory tokens; if the access token already changed, another request just
/// refreshed and we reuse that fresh token instead of calling TIDAL again.
pub(super) async fn recover_tidal_client(
    state: &SharedState,
    used_tokens: &tidal_auth::TidalTokens,
) -> anyhow::Result<TidalClient> {
    let (current_tokens, http_client, tidal_http_client) = {
        let s = state.read().await;
        (
            s.tidal_tokens.clone(),
            s.http_client.clone(),
            s.tidal_http_client.clone(),
        )
    };

    if let Some(current) = current_tokens
        && current.access_token != used_tokens.access_token
    {
        return Ok(TidalClient::with_http(
            tidal_http_client,
            current.access_token.clone(),
            current.country_code.clone(),
        ));
    }

    let refreshed = recover_tidal_session(state, &http_client, used_tokens).await?;
    Ok(TidalClient::with_http(
        tidal_http_client,
        refreshed.access_token.clone(),
        refreshed.country_code.clone(),
    ))
}

pub(super) fn error_looks_like_auth(err: &anyhow::Error) -> bool {
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

async fn bump_playback_generation(state: &SharedState) -> u64 {
    let state_guard = state.read().await;
    state_guard
        .playback_generation
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        + 1
}

async fn playback_generation_is_current(state: &SharedState, generation: u64) -> bool {
    let state_guard = state.read().await;
    current_playback_generation(&state_guard) == generation
}

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
    })))
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

        let dj_engine_enabled = state_guard
            .db
            .with_conn(queries::is_dj_engine_enabled)
            .unwrap_or(false);
        let config = playback_runtime::PlaybackRuntimeConfig::new(
            state_guard.http_client.clone(),
            access_token.clone(),
            state_guard.analysis_tx.clone(),
        )
        .with_stream_resolver(runtime_stream_resolver(state.clone()))
        .with_dj_analysis(dj_engine_enabled, state_guard.dj_analysis_tx.clone());
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

    if let Some(listener_handle) = spawned_handle.clone() {
        spawn_playback_runtime_listener(state.clone(), listener_handle);
    }

    if let Some(runtime_handle) = spawned_handle.as_ref() {
        apply_persisted_runtime_output_settings(state, runtime_handle).await;
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
                    // Track is no longer producing audio. Clear the flag before advancing.
                    state
                        .write()
                        .await
                        .audio_active
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    if let Err(error) =
                        handle_runtime_finished_with_retry(state.clone(), track_id, generation)
                            .await
                    {
                        let message =
                            format!("Failed to advance playback after track end: {error}");
                        report_playback_failure(&state, &message);
                        error!("{message}");
                    }
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::DjTransitionPromoted {
                    transition_event_id,
                    actual_start_ms,
                    timing_status,
                    runtime_rendered_dj_mixer,
                    runtime_renderer_status,
                    runtime_renderer_reason,
                    ..
                }) => {
                    let state_guard = state.read().await;
                    match state_guard.db.with_conn(|conn| {
                        queries::update_dj_transition_fire_timing(
                            conn,
                            transition_event_id,
                            actual_start_ms,
                            timing_status.as_str(),
                            runtime_rendered_dj_mixer,
                            runtime_renderer_status.as_str(),
                            runtime_renderer_reason.as_str(),
                        )
                    }) {
                        Ok(()) => {
                            info!(
                                transition_event_id,
                                actual_start_ms,
                                timing_status = %timing_status,
                                "Recorded DJ transition timing"
                            );
                        }
                        Err(error) => {
                            warn!("Failed to record DJ transition timing: {error}");
                        }
                    }
                    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::Error { message }) => {
                    handle_runtime_error(state.clone(), &message).await;
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::TrackError {
                    track_id,
                    generation,
                    message,
                }) => {
                    if let Err(error) =
                        handle_runtime_track_error(state.clone(), track_id, generation, &message)
                            .await
                    {
                        let message =
                            format!("Failed to advance playback after track error: {error}");
                        report_playback_failure(&state, &message);
                        error!("{message}");
                    }
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::PreparedTrackError {
                    track_id,
                    message,
                }) => {
                    handle_prepared_runtime_track_error(&state, track_id, &message).await;
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
                    let prev_exclusive_transport = if prev_exclusive {
                        state_guard
                            .playback_runtime_info
                            .as_ref()
                            .and_then(|i| i.exclusive_transport_format.clone())
                    } else {
                        None
                    };
                    state_guard.playback_runtime_info = Some(PlaybackRuntimeInfo {
                        device_name,
                        sample_rate,
                        channels,
                        active_track_id: None,
                        last_error,
                        exclusive_engaged: prev_exclusive,
                        exclusive_transport_format: prev_exclusive_transport,
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
                    // CPAL buffer threshold crossed. Samples are actually flowing now.
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
                    drop(state_guard);
                    start_dj_lookahead_and_queue_profiles_after_pair_change(
                        state.clone(),
                        handle.clone(),
                        "playback_started",
                    )
                    .await;
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::Paused { .. })
                | Ok(playback_runtime::PlaybackRuntimeEvent::Resumed { .. })
                | Ok(playback_runtime::PlaybackRuntimeEvent::Preparing { .. }) => {}
                Ok(playback_runtime::PlaybackRuntimeEvent::DropPreviewStarted {
                    track_id,
                    generation,
                    actual_start_ms,
                }) => {
                    let mut state_guard = state.write().await;
                    state_guard.last_drop_preview = Some(crate::DropPreviewRuntimeState {
                        track_id,
                        generation,
                        actual_fire_ms: actual_start_ms,
                    });
                    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
                }
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
                    state_guard.prepared_ephemeral_tidal_next = None;
                    state_guard.current_stream_display = None;
                    state_guard.pending_stream_display = None;
                    state_guard.next_prebuffer_inflight = None;
                    state_guard.last_drop_preview = None;
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
                    transport_format,
                }) => {
                    let mut state_guard = state.write().await;
                    if let Some(info) = state_guard.playback_runtime_info.as_mut() {
                        info.exclusive_engaged = true;
                        info.exclusive_transport_format = Some(transport_format.clone());
                    }
                    let _ = state_guard.event_tx.send(AppEvent::AudioExclusiveEngaged {
                        device: device_name,
                        transport_format,
                    });
                }
                Ok(playback_runtime::PlaybackRuntimeEvent::ExclusiveModeFailed {
                    reason,
                    device_name,
                }) => {
                    let mut state_guard = state.write().await;
                    if let Some(info) = state_guard.playback_runtime_info.as_mut() {
                        info.exclusive_engaged = false;
                        info.exclusive_transport_format = None;
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
                        info.exclusive_transport_format = None;
                    }
                    let _ = state_guard.event_tx.send(AppEvent::AudioExclusiveReleased {
                        device: device_name,
                    });
                    let retry = {
                        let settings = state_guard
                            .db
                            .with_conn(|conn| {
                                crate::db::audio_settings::load(conn).map_err(anyhow::Error::from)
                            })
                            .ok();
                        let is_playing = state_guard
                            .db
                            .with_conn(|conn| player::load_state(conn).map(|s| s.is_playing))
                            .unwrap_or(false);
                        let runtime = state_guard
                            .playback_runtime
                            .as_ref()
                            .map(|runtime| runtime.handle.clone());
                        settings.and_then(|settings| {
                            if should_retry_exclusive_release(is_playing, settings.exclusive_mode) {
                                runtime.map(|runtime| {
                                    (
                                        runtime,
                                        runtime_output_settings_from_audio_settings(&settings),
                                    )
                                })
                            } else {
                                None
                            }
                        })
                    };
                    drop(state_guard);
                    if let Some((runtime, output)) = retry
                        && let Err(error) = runtime.device_swap(
                            output.device,
                            output.exclusive_mode,
                            output.sample_rate_follow,
                            None,
                            output.exclusive_release_grace_secs,
                            output.exclusive_latency_mode,
                        )
                    {
                        warn!("Failed to recover released WASAPI exclusive stream: {error}");
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
async fn handle_ephemeral_tidal_near_end(
    state: SharedState,
    current_track_id: i64,
    generation: u64,
) -> anyhow::Result<bool> {
    let (current_track, pending_track, pending_tracks, handle, sample_rate, channels) = {
        let state_guard = state.read().await;
        let active_id = state_guard
            .playback_runtime_info
            .as_ref()
            .and_then(|info| info.active_track_id);
        if active_id != Some(current_track_id)
            || current_playback_generation(&state_guard) != generation
        {
            return Ok(true);
        }

        let Some(current_track) = state_guard.ephemeral_tidal_track.clone() else {
            return Ok(false);
        };
        if current_track.id != current_track_id {
            return Ok(false);
        }
        // Peek (don't pop) the next upcoming ephemeral row to pre-buffer it, in
        // queue order: if a regular row (library "Play next") sits in front of the
        // continuation, the mix is not what plays next, so bail and let the generic
        // near-end path pre-buffer that row instead of arming a crossfade into a
        // track we're about to skip over.
        let pending_tracks = state_guard
            .db
            .with_conn(next_advance_ephemeral_track)
            .ok()
            .flatten()
            .into_iter()
            .collect::<Vec<_>>();
        let Some(pending_track) = pending_tracks.first().cloned() else {
            return Ok(false);
        };
        let Some(handle) = state_guard
            .playback_runtime
            .as_ref()
            .map(|runtime| runtime.handle.clone())
        else {
            return Ok(true);
        };
        let Some(runtime_info) = state_guard.playback_runtime_info.as_ref() else {
            return Ok(true);
        };
        (
            current_track,
            pending_track,
            pending_tracks,
            handle,
            runtime_info.sample_rate,
            runtime_info.channels,
        )
    };

    let dj_engine_enabled = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(queries::is_dj_engine_enabled)
            .unwrap_or(false)
    };
    if !dj_engine_enabled {
        return Ok(false);
    }

    let user_quality = current_user_audio_quality(&state).await;
    let stream_request =
        build_ephemeral_tidal_stream_request(pending_track.tidal_track_id, user_quality.clone());
    let resolve_probe = crate::db::models::Track {
        id: -pending_track.tidal_track_id,
        title: pending_track.title.clone(),
        artist_id: 0,
        artist_name: pending_track.artist_name.clone(),
        album_id: None,
        album_title: pending_track.album_title.clone(),
        disc_number: None,
        track_number: None,
        duration_ms: pending_track.duration_ms,
        isrc: None,
        tidal_id: Some(pending_track.tidal_track_id),
        artist_tidal_id: pending_track.artist_tidal_id,
        album_tidal_id: pending_track.album_tidal_id,
        ytmusic_id: None,
        soundcloud_id: None,
        best_quality: None,
        best_source: Some("tidal".to_string()),
        fidelity_score: 0,
        is_favorite: false,
        play_count: 0,
        last_played_at: None,
        date_added: None,
        source: "tidal_ephemeral".to_string(),
        artwork_url: pending_track.artwork_url.clone(),
    };
    let stream_info =
        match resolve_tidal_playback_stream(&state, &resolve_probe, &stream_request).await {
            Ok(info) => info,
            Err(error) => {
                warn!(
                    "Skipping DJ pre-buffer for TIDAL mix track {}: {}",
                    pending_track.tidal_track_id,
                    describe_tidal_playback_error(&error)
                );
                return Ok(true);
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
                "Skipping DJ pre-buffer for TIDAL mix track {} after TIDAL session refresh",
                pending_track.tidal_track_id
            );
            return Ok(true);
        }
    }

    let library_state = {
        let state_guard = state.read().await;
        lookup_tidal_track_library_state(&state_guard.db, pending_track.tidal_track_id)
    };
    let synthetic = build_ephemeral_synthetic_track(&pending_track, &stream_info, library_state);
    let stream_display = crate::StreamDisplayInfo {
        audio_quality: stream_info.audio_quality.clone(),
        sample_rate: stream_info.sample_rate,
        bit_depth: stream_info.bit_depth,
    };

    let pair = crate::playback::dj_lookahead::build_ephemeral_tidal_mix_pair(
        &current_track,
        &pending_tracks,
    )
    .context("ephemeral TIDAL mix pair unavailable")?;
    let lookahead_start =
        player::dj_lookahead_start_from_pair(pair.clone(), EPHEMERAL_DJ_LOOKAHEAD_DEADLINE_SAMPLES);
    if lookahead_start.is_some() {
        queue_missing_dj_profiles_after_pair_change(state.clone(), "ephemeral_tidal_mix_prebuffer")
            .await;
    }
    let effective_crossfade = current_crossfade_ms(&state).await;
    let mut job = player::build_playback_preparation(
        &synthetic,
        Some(&stream_info),
        effective_crossfade,
        user_quality,
    )
    .with_generation(generation);
    if let Some(media_ref) = crate::playback::dj_lookahead::tidal_media_ref_for_track(&synthetic) {
        job = job.with_dj_media_ref(media_ref);
    }
    let engine = crate::playback::dj_engine::DjEngine::new({
        let state_guard = state.read().await;
        state_guard.db.clone()
    });
    job = player::attach_dj_transition_plan_for_pair_with_current_duration(
        &engine,
        job,
        pair,
        stream_info.sample_rate_hz().unwrap_or(sample_rate),
        channels,
        current_track.duration_ms,
    )?;

    {
        let state_guard = state.read().await;
        let active_id = state_guard
            .playback_runtime_info
            .as_ref()
            .and_then(|info| info.active_track_id);
        // Ordered re-check: a regular row inserted in front since the peek above
        // invalidates the prepared mix track even though it is still the lowest
        // ephemeral row.
        let pending_front = state_guard
            .db
            .with_conn(next_advance_ephemeral_tidal_id)
            .ok()
            .flatten();
        if active_id != Some(current_track_id)
            || current_playback_generation(&state_guard) != generation
            || state_guard
                .ephemeral_tidal_track
                .as_ref()
                .map(|track| track.id)
                != Some(current_track_id)
            || pending_front != Some(pending_track.tidal_track_id)
        {
            return Ok(true);
        }
    }

    if job.prepared_transition.is_some()
        && let Some(start) = lookahead_start
    {
        start.dispatch(&handle)?;
    }
    handle.prepare_next(job)?;
    {
        let mut state_guard = state.write().await;
        state_guard.prepared_ephemeral_tidal_next = Some(crate::PreparedEphemeralTidalNext {
            tidal_track_id: pending_track.tidal_track_id,
            synthetic_track: synthetic.clone(),
            stream_display: stream_display.clone(),
            generation,
        });
        state_guard.pending_stream_display = Some(stream_display);
    }
    info!(
        "DJ pre-buffering TIDAL mix track: {} (tidal id {})",
        synthetic.title, pending_track.tidal_track_id
    );
    Ok(true)
}

async fn handle_near_end(
    state: SharedState,
    current_track_id: i64,
    generation: u64,
) -> anyhow::Result<()> {
    if handle_ephemeral_tidal_near_end(state.clone(), current_track_id, generation).await? {
        return Ok(());
    }

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
    if matches!(
        handle.track_status(next.id, generation),
        playback_runtime::PlaybackTrackStatus::Active
            | playback_runtime::PlaybackTrackStatus::Prepared
    ) {
        return Ok(());
    }
    let prebuffer_key = crate::NextPrebufferKey {
        current_track_id,
        next_track_id: next.id,
        generation,
    };
    {
        let mut state_guard = state.write().await;
        if !claim_next_prebuffer_slot(&mut state_guard.next_prebuffer_inflight, prebuffer_key) {
            return Ok(());
        }
    }

    let result = handle_near_end_prebuffer_next(
        state.clone(),
        current_track_id,
        generation,
        next,
        handle,
        crossfade_ms,
    )
    .await;
    {
        let mut state_guard = state.write().await;
        release_next_prebuffer_slot(&mut state_guard.next_prebuffer_inflight, prebuffer_key);
    }
    result
}

async fn handle_near_end_prebuffer_next(
    state: SharedState,
    current_track_id: i64,
    generation: u64,
    next: crate::db::models::Track,
    handle: playback_runtime::PlaybackRuntimeHandle,
    crossfade_ms: i32,
) -> anyhow::Result<()> {
    // Resolve the stream URL for the next track (we need a live access token).
    let user_quality = current_user_audio_quality(&state).await;
    let stream_request = match player::build_tidal_stream_request(&next, user_quality.clone()) {
        Some(req) => req,
        None => return Ok(()), // local library: skip pre-buffer for now
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
        let external_current = state_guard
            .ephemeral_tidal_track
            .as_ref()
            .or(state_guard.external_playback_track.as_ref())
            .map(|track| track.id);
        let cleared = recently_cleared(&state_guard);
        let still_next = state_guard
            .db
            .with_conn(|conn| player::peek_next_track(conn, cleared))
            .ok()
            .flatten()
            .map(|track| track.id);
        if active_id != Some(current_track_id)
            || (db_current != Some(current_track_id) && external_current != Some(current_track_id))
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
                let current_bit_depth = state_guard
                    .current_stream_display
                    .as_ref()
                    .and_then(|display| display.bit_depth);
                if should_skip_prebuffer_for_sample_rate_follow_format_change(
                    settings.exclusive_mode,
                    settings.sample_rate_follow,
                    current_rate,
                    Some(next_rate),
                    current_bit_depth,
                    stream.bit_depth,
                ) {
                    let current_depth_label = current_bit_depth
                        .map(|depth| depth.to_string())
                        .unwrap_or_else(|| "unknown".to_string());
                    let next_depth_label = stream
                        .bit_depth
                        .map(|depth| depth.to_string())
                        .unwrap_or_else(|| "unknown".to_string());
                    info!(
                        "Skipping pre-buffer for next track {}: sample-rate-follow will switch native format from {} Hz/{} bit to {} Hz/{} bit at track start",
                        next.id, current_rate, current_depth_label, next_rate, next_depth_label
                    );
                    return Ok(());
                }
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
                        settings.exclusive_latency_mode,
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

    let effective_crossfade = effective_crossfade_ms(&state, crossfade_ms).await;
    let _gapless = crate::playback::gapless::plan_from_stream(
        stream_info.as_ref(),
        crate::playback::gapless::GaplessSettings::new(true, effective_crossfade),
    );
    let job = player::build_playback_preparation(
        &next,
        stream_info.as_ref(),
        effective_crossfade,
        user_quality,
    )
    .with_generation(generation);
    let (job, lookahead_start) = {
        let state_guard = state.read().await;
        let channels = state_guard
            .playback_runtime_info
            .as_ref()
            .map(|info| info.channels)
            .unwrap_or(2);
        let pair = state_guard
            .db
            .with_conn(|conn| active_dj_pair_for_state_and_conn(&state_guard, conn))?;
        let engine = crate::playback::dj_engine::DjEngine::new(state_guard.db.clone());
        let job = player::attach_dj_transition_plan_for_pair(
            &engine,
            job,
            pair,
            stream_info
                .as_ref()
                .and_then(|info| info.sample_rate_hz())
                .unwrap_or(48_000),
            channels,
        )?;
        let lookahead_start = if job.prepared_transition.is_some() {
            active_dj_lookahead_start_for_state(&state_guard)
        } else {
            None
        };
        (job, lookahead_start)
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
        let external_current = state_guard
            .ephemeral_tidal_track
            .as_ref()
            .or(state_guard.external_playback_track.as_ref())
            .map(|track| track.id);
        if active_id != Some(current_track_id)
            || (db_current != Some(current_track_id) && external_current != Some(current_track_id))
        {
            return Ok(());
        }
        if current_playback_generation(&state_guard) != generation {
            return Ok(());
        }
    }

    if job.prepared_transition.is_some()
        && let Some(start) = lookahead_start
    {
        let _ = start.dispatch(&handle);
        queue_missing_dj_profiles_after_pair_change(state.clone(), "prepared_next_transition")
            .await;
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

fn claim_next_prebuffer_slot(
    slot: &mut Option<crate::NextPrebufferKey>,
    key: crate::NextPrebufferKey,
) -> bool {
    if *slot == Some(key) {
        return false;
    }
    *slot = Some(key);
    true
}

fn release_next_prebuffer_slot(
    slot: &mut Option<crate::NextPrebufferKey>,
    key: crate::NextPrebufferKey,
) {
    if *slot == Some(key) {
        *slot = None;
    }
}

async fn try_adopt_prepared_ephemeral_tidal_next(
    state: &SharedState,
    finished_track_id: i64,
    generation: u64,
) -> anyhow::Result<bool> {
    let (prepared, handle) = {
        let state_guard = state.read().await;
        let Some(prepared) = state_guard.prepared_ephemeral_tidal_next.clone() else {
            return Ok(false);
        };
        if prepared.generation != generation
            || state_guard
                .ephemeral_tidal_track
                .as_ref()
                .map(|track| track.id)
                != Some(finished_track_id)
        {
            return Ok(false);
        }
        // Ordered check, not a bare ephemeral peek: a regular row inserted in
        // front of the continuation (library "Play next") invalidates the
        // prepared mix track even though it is still the lowest ephemeral row.
        let pending_front = state_guard
            .db
            .with_conn(next_advance_ephemeral_tidal_id)
            .ok()
            .flatten();
        if pending_front != Some(prepared.tidal_track_id) {
            return Ok(false);
        }
        let Some(handle) = state_guard
            .playback_runtime
            .as_ref()
            .map(|runtime| runtime.handle.clone())
        else {
            return Ok(false);
        };
        (prepared, handle)
    };

    let prepared_status = handle.track_status(prepared.synthetic_track.id, generation);
    if !matches!(
        prepared_status,
        playback_runtime::PlaybackTrackStatus::Active
            | playback_runtime::PlaybackTrackStatus::Prepared
    ) {
        return Ok(false);
    }

    let user_quality = current_user_audio_quality(state).await;
    let mut job = player::build_playback_preparation(
        &prepared.synthetic_track,
        None,
        current_crossfade_ms(state).await,
        user_quality,
    )
    .with_generation(generation);
    if let Some(media_ref) =
        crate::playback::dj_lookahead::tidal_media_ref_for_track(&prepared.synthetic_track)
    {
        job = job.with_dj_media_ref(media_ref);
    }
    handle.switch_to(job)?;

    {
        let mut state_guard = state.write().await;
        if current_playback_generation(&state_guard) != generation
            || state_guard
                .ephemeral_tidal_track
                .as_ref()
                .map(|track| track.id)
                != Some(finished_track_id)
        {
            return Ok(true);
        }
        // Consume the upcoming ephemeral row, but only if it's still the one we
        // prepared (a concurrent reorder/remove could have changed it).
        let popped = state_guard
            .db
            .with_conn(|conn| match next_advance_ephemeral_tidal_id(conn)? {
                Some(front) if front == prepared.tidal_track_id => {
                    queue::pop_next_ephemeral_tidal_track(conn)
                }
                _ => Ok(None),
            })
            .unwrap_or(None);
        if popped.is_none() {
            return Ok(true);
        }

        state_guard.external_playback_track = None;
        state_guard.ephemeral_tidal_track = Some(prepared.synthetic_track.clone());
        state_guard.current_stream_display = Some(prepared.stream_display.clone());
        state_guard.pending_stream_display = None;
        state_guard.prepared_ephemeral_tidal_next = None;
        if let Some(info) = state_guard.playback_runtime_info.as_mut() {
            info.active_track_id = Some(prepared.synthetic_track.id);
            info.last_error = None;
        }
        let _ = state_guard.event_tx.send(AppEvent::TrackChanged {
            track_id: prepared.synthetic_track.id,
        });
        let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    }

    let snapshot = build_live_playback_snapshot(state)
        .await
        .map_err(|status| anyhow::anyhow!("playback snapshot failed: {status}"))?;
    sync_session_after_snapshot(
        state,
        &snapshot,
        Some(player::ListenSessionEndReason::Replaced),
    )
    .await;
    Ok(true)
}

async fn switch_runtime_to_snapshot_current(
    state: &SharedState,
    snapshot: &player::PlaybackSnapshot,
    generation: u64,
) -> anyhow::Result<()> {
    // A track whose TIDAL asset won't resolve (pulled from the catalog, 4005
    // "asset not ready", or a hard rejection) used to wedge playback here: the
    // resolve error propagated up as fatal and the runtime sat frozen on a dead
    // row. Instead, skip past it and try the next queue item, bounded so a real
    // TIDAL outage doesn't chew silently through the whole queue.
    const MAX_UNPLAYABLE_SKIPS: u32 = 8;
    let mut snapshot = snapshot.clone();
    let mut unplayable_skips: u32 = 0;

    loop {
        let current_queue_item_id = snapshot.state.current_queue_item_id;
        let queue_len = snapshot.queue.len();

        let Some(track) = snapshot.state.current_track.clone() else {
            tracing::info!(
                target: "noor.playback.runtime",
                event = "runtime_snapshot_empty",
                generation,
                ?current_queue_item_id,
                queue_len,
                "snapshot has no current track; clearing runtime active track"
            );
            if unplayable_skips > 0 {
                sync_session_after_snapshot(
                    state,
                    &snapshot,
                    Some(player::ListenSessionEndReason::QueueEnded),
                )
                .await;
            }
            let mut state_guard = state.write().await;
            if let Some(info) = state_guard.playback_runtime_info.as_mut() {
                info.active_track_id = None;
            }
            let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
            let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
            return Ok(());
        };

        let user_quality = current_user_audio_quality(state).await;
        let runtime_handle = ensure_playback_runtime_for_track(state, &track)
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
                &track,
                None,
                effective_crossfade_ms(state, snapshot.state.crossfade_ms).await,
                user_quality,
            )
            .with_generation(generation);
            runtime_handle.switch_to(job).map_err(|error| {
                tracing::warn!(
                    target: "noor.playback.runtime",
                    event = "runtime_snapshot_switch_failed",
                    generation,
                    track_id = track.id,
                    ?current_queue_item_id,
                    queue_len,
                    runtime_track_status = ?prepared_status,
                    stream_resolved = false,
                    error = %error,
                    "failed to switch runtime to prepared snapshot current track"
                );
                error
            })?;
            {
                let mut state_guard = state.write().await;
                if let Some(info) = state_guard.playback_runtime_info.as_mut() {
                    info.active_track_id = Some(track.id);
                    info.last_error = None;
                }
                if let Some(pending) = state_guard.pending_stream_display.take() {
                    state_guard.current_stream_display = Some(pending);
                }
            }
            tracing::info!(
                target: "noor.playback.runtime",
                event = "runtime_snapshot_switch",
                generation,
                track_id = track.id,
                ?current_queue_item_id,
                queue_len,
                runtime_track_status = ?prepared_status,
                stream_resolved = false,
                "switched runtime to prepared snapshot current track"
            );
        } else {
            let Some(stream_request) =
                player::build_tidal_stream_request(&track, user_quality.clone())
            else {
                handle_runtime_error(
                    state.clone(),
                    "Local library playback is not wired into the host audio runtime yet.",
                )
                .await;
                return Ok(());
            };
            let stream_info =
                match resolve_tidal_playback_stream(state, &track, &stream_request).await {
                    Ok(info) => info,
                    Err(err)
                        if err.is_track_unplayable() && unplayable_skips < MAX_UNPLAYABLE_SKIPS =>
                    {
                        unplayable_skips += 1;
                        let reason = if err.is_asset_not_ready() {
                            "Not available on TIDAL right now"
                        } else {
                            "TIDAL wouldn't play this track"
                        };
                        tracing::warn!(
                            target: "noor.playback.advance",
                            event = "skip_unplayable_track",
                            generation,
                            track_id = track.id,
                            skip = unplayable_skips,
                            error = %describe_tidal_playback_error(&err),
                            "skipping unplayable track and advancing to the next queue row"
                        );
                        emit_track_skipped(state, track.id, &track.title, reason).await;
                        if err.is_asset_not_ready() {
                            spawn_tidal_id_reresolve(state, track.id);
                        }
                        // Advance the persisted queue past the dead row and retry.
                        let cleared = {
                            let s = state.read().await;
                            recently_cleared(&s)
                        };
                        let advanced = {
                            let s = state.read().await;
                            s.db.with_conn(|conn| player::next_track(conn, cleared))
                        }?;
                        snapshot = resolve_or_skip_pending_current(
                            state,
                            advanced,
                            generation,
                            "skip_unplayable",
                        )
                        .await?;
                        if !playback_generation_is_current(state, generation).await {
                            return Ok(());
                        }
                        continue;
                    }
                    Err(err) => {
                        return Err(anyhow::anyhow!(
                            "playback stream resolve failed: {}",
                            describe_tidal_playback_error(&err)
                        ));
                    }
                };
            let job = player::build_playback_preparation(
                &track,
                Some(&stream_info),
                effective_crossfade_ms(state, snapshot.state.crossfade_ms).await,
                user_quality,
            )
            .with_generation(generation);
            runtime_handle.switch_to(job).map_err(|error| {
                tracing::warn!(
                    target: "noor.playback.runtime",
                    event = "runtime_snapshot_switch_failed",
                    generation,
                    track_id = track.id,
                    ?current_queue_item_id,
                    queue_len,
                    runtime_track_status = ?prepared_status,
                    stream_resolved = true,
                    error = %error,
                    "failed to switch runtime after resolving snapshot stream"
                );
                error
            })?;
            {
                let mut state_guard = state.write().await;
                if let Some(info) = state_guard.playback_runtime_info.as_mut() {
                    info.active_track_id = Some(track.id);
                    info.last_error = None;
                }
                state_guard.current_stream_display = Some(crate::StreamDisplayInfo {
                    audio_quality: stream_info.audio_quality.clone(),
                    sample_rate: stream_info.sample_rate,
                    bit_depth: stream_info.bit_depth,
                });
                state_guard.pending_stream_display = None;
            }
            tracing::info!(
                target: "noor.playback.runtime",
                event = "runtime_snapshot_switch",
                generation,
                track_id = track.id,
                ?current_queue_item_id,
                queue_len,
                runtime_track_status = ?prepared_status,
                stream_resolved = true,
                "switched runtime after resolving snapshot stream"
            );
        }

        // If we skipped past dead rows, the caller's pre-switch session sync was
        // for a track we never played. Re-anchor the listen session onto the row
        // that actually started so completion + transition learning is correct.
        if unplayable_skips > 0 {
            sync_session_after_snapshot(
                state,
                &snapshot,
                Some(player::ListenSessionEndReason::Replaced),
            )
            .await;
        }

        let state_guard = state.read().await;
        let _ = state_guard
            .event_tx
            .send(AppEvent::TrackChanged { track_id: track.id });
        let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
        let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
        return Ok(());
    }
}

/// Start `first` and, on failure, skip forward through the remaining ephemeral
/// mix continuation (respecting queue order). Returns true once a track is
/// actually playing.
///
/// Skip-and-retry advance: a single TIDAL hiccup (especially a 429 rate-limit a
/// few tracks into a mix) used to nuke the entire remaining queue. 429 is
/// recoverable (sleep + retry the same track once); any other failure is
/// track-specific (skip to the next item). Only gives up when the continuation
/// is exhausted or MAX_CONSEC_FAILURES distinct tracks fail in a row.
async fn start_ephemeral_continuation_with_retry(
    state: &SharedState,
    first: crate::PendingEphemeralTidalTrack,
) -> bool {
    // Clear the previous live-track markers so the new track's
    // PlaybackStateChanged + Started events overwrite cleanly.
    {
        let mut state_guard = state.write().await;
        state_guard.external_playback_track = None;
        state_guard.ephemeral_tidal_track = None;
    }
    const MAX_CONSEC_FAILURES: u32 = 3;
    let mut current = first;
    let mut consecutive_failures: u32 = 0;
    loop {
        let mut result = start_ephemeral_tidal_playback(state, current.clone()).await;
        if let Err((status, _)) = &result
            && status.as_u16() == 429
        {
            tracing::warn!(
                "TIDAL 429 advancing mix to '{}': backing off 3s and retrying once",
                current.title
            );
            tokio::time::sleep(Duration::from_secs(3)).await;
            result = start_ephemeral_tidal_playback(state, current.clone()).await;
        }
        match result {
            Ok(()) => return true,
            Err((status, body)) => {
                consecutive_failures += 1;
                tracing::warn!(
                    "Failed to advance to '{}' ({status}, fail {consecutive_failures}/{MAX_CONSEC_FAILURES}): {}. Skipping",
                    current.title,
                    body.0
                );
                if consecutive_failures >= MAX_CONSEC_FAILURES {
                    tracing::warn!(
                        "Hit max consecutive failures advancing TIDAL mix; clearing remaining queue"
                    );
                    let s = state.read().await;
                    let _ = s.db.with_conn(|conn| {
                        Ok::<_, anyhow::Error>(queue::delete_all_ephemeral_tidal_rows(conn)?)
                    });
                    return false;
                }
                let popped = {
                    let s = state.read().await;
                    s.db.with_conn(pop_next_ephemeral_if_due).ok().flatten()
                };
                match popped {
                    Some(p) => current = p,
                    None => return false,
                }
            }
        }
    }
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
    tracing::info!(
        target: "noor.playback.advance",
        event = "runtime_finished",
        finished_track_id,
        generation,
        "runtime finished track; advancing queue"
    );
    if let Err(error) = mark_armed_dj_transition_missed_if_needed(&state).await {
        warn!("Failed to mark missed DJ transition timing: {error}");
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
        if try_adopt_prepared_ephemeral_tidal_next(&state, finished_track_id, generation).await? {
            return Ok(());
        }
        {
            let mut state_guard = state.write().await;
            state_guard.prepared_ephemeral_tidal_next = None;
            state_guard.pending_stream_display = None;
        }
        // Auto-advance through any queued ephemeral mix continuation before
        // tearing down. Pop the next ephemeral row out of the queue and start
        // it; if there are none, or a regular row (library "Play next") sits
        // in front of the continuation, fall through to the persisted-queue
        // advance below, which plays that row and keeps the mix queued.
        let next = {
            let s = state.read().await;
            s.db.with_conn(pop_next_ephemeral_if_due).ok().flatten()
        };
        if let Some(next) = next {
            if start_ephemeral_continuation_with_retry(&state, next).await {
                return Ok(());
            }
            // Fall through to teardown so the UI doesn't get stuck on a
            // ghost track.
        }
        let persisted_snapshot = next_persisted_playback_snapshot(&state).await?;
        let persisted_snapshot = resolve_or_skip_pending_current(
            &state,
            persisted_snapshot,
            generation,
            "runtime_finished_external",
        )
        .await?;
        if !playback_generation_is_current(&state, generation).await {
            return Ok(());
        }
        if persisted_snapshot.state.current_track.is_some() {
            {
                let mut state_guard = state.write().await;
                state_guard.external_playback_track = None;
                state_guard.ephemeral_tidal_track = None;
                state_guard.prepared_ephemeral_tidal_next = None;
                state_guard.pending_stream_display = None;
            }
            sync_session_after_snapshot(
                &state,
                &persisted_snapshot,
                Some(player::ListenSessionEndReason::Replaced),
            )
            .await;
            switch_runtime_to_snapshot_current(&state, &persisted_snapshot, generation).await?;
            return Ok(());
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

    enum FinishedAdvance {
        Mismatch,
        ResumeMix(crate::PendingEphemeralTidalTrack),
        Snapshot(player::PlaybackSnapshot),
    }
    let advance = {
        let state_guard = state.read().await;
        let cleared = recently_cleared(&state_guard);
        state_guard.db.with_conn(|conn| {
            let current_track_id = player::current_track_id(conn)?;
            let current_state = player::load_state(conn)?;
            if current_track_id != Some(finished_track_id) || !current_state.is_playing {
                return Ok(FinishedAdvance::Mismatch);
            }

            // A play-next interlude just finished with the mix continuation
            // queued behind its anchor: hand the front mix row to the
            // ephemeral starter. The library advance cannot play a
            // track_id-less row and would strand or trip over it. Repeat-one
            // keeps replaying the interlude, same as a plain library queue.
            if current_state.repeat_mode != "one"
                && let Some(next) = pop_next_ephemeral_if_due(conn)?
            {
                return Ok(FinishedAdvance::ResumeMix(next));
            }

            Ok(FinishedAdvance::Snapshot(player::next_track(
                conn, cleared,
            )?))
        })?
    };

    let snapshot = match advance {
        FinishedAdvance::Mismatch => None,
        FinishedAdvance::ResumeMix(next) => {
            if start_ephemeral_continuation_with_retry(&state, next).await {
                return Ok(());
            }
            // Continuation unplayable or exhausted: fall back to the normal
            // queue advance so playback doesn't hang on the finished track.
            Some(next_persisted_playback_snapshot(&state).await?)
        }
        FinishedAdvance::Snapshot(snapshot) => Some(snapshot),
    };

    let Some(snapshot) = snapshot else {
        // Track-id mismatch (e.g. user already advanced to another track via
        // play_track, or playback was paused). The runtime-finished session
        // is for `finished_track_id` and may still be sitting in
        // active_listen_session. Flush it before bailing so the partial
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
    let snapshot =
        resolve_or_skip_pending_current(&state, snapshot, generation, "runtime_finished").await?;
    if !playback_generation_is_current(&state, generation).await {
        return Ok(());
    }

    let end_reason = if snapshot.state.current_track.is_some() {
        Some(player::ListenSessionEndReason::Replaced)
    } else {
        Some(player::ListenSessionEndReason::QueueEnded)
    };
    sync_session_after_snapshot(&state, &snapshot, end_reason).await;
    switch_runtime_to_snapshot_current(&state, &snapshot, generation).await
}

async fn mark_armed_dj_transition_missed_if_needed(state: &SharedState) -> anyhow::Result<()> {
    let pair = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(|conn| active_dj_pair_for_state_and_conn(&state_guard, conn))?
    };
    let (Some(current), Some(next)) = (pair.current.as_ref(), pair.next.as_ref()) else {
        return Ok(());
    };
    let current_key = current.profile_key();
    let next_key = next.profile_key();
    let updated = {
        let state_guard = state.read().await;
        state_guard.db.with_conn(|conn| {
            queries::mark_dj_transition_timing_status_for_pair(
                conn,
                current_key.media_ref_kind.as_str(),
                current_key.media_ref_id.as_str(),
                next_key.media_ref_kind.as_str(),
                next_key.media_ref_id.as_str(),
                "missed",
            )
        })?
    };
    if updated > 0 {
        let state_guard = state.read().await;
        let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    }
    Ok(())
}

async fn mark_armed_dj_transition_manual_seek_suppressed_if_needed(
    state: &SharedState,
) -> anyhow::Result<()> {
    let pair = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(|conn| active_dj_pair_for_state_and_conn(&state_guard, conn))?
    };
    let (Some(current), Some(next)) = (pair.current.as_ref(), pair.next.as_ref()) else {
        return Ok(());
    };
    let current_key = current.profile_key();
    let next_key = next.profile_key();
    let updated = {
        let state_guard = state.read().await;
        state_guard.db.with_conn(|conn| {
            queries::mark_dj_transition_manual_seek_suppressed_for_pair(
                conn,
                current_key.media_ref_kind.as_str(),
                current_key.media_ref_id.as_str(),
                next_key.media_ref_kind.as_str(),
                next_key.media_ref_id.as_str(),
            )
        })?
    };
    if updated > 0 {
        let state_guard = state.read().await;
        let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    }
    Ok(())
}

async fn handle_runtime_finished_with_retry(
    state: SharedState,
    finished_track_id: i64,
    generation: u64,
) -> anyhow::Result<()> {
    for attempt in 0..=PLAYBACK_FINISH_DB_LOCK_RETRY_LIMIT {
        match handle_runtime_finished(state.clone(), finished_track_id, generation).await {
            Ok(()) => return Ok(()),
            Err(error)
                if sqlite_database_locked(&error)
                    && attempt < PLAYBACK_FINISH_DB_LOCK_RETRY_LIMIT =>
            {
                let next_attempt = attempt + 1;
                warn!(
                    finished_track_id,
                    generation, next_attempt, "Playback advance hit a locked database; retrying"
                );
                tokio::time::sleep(Duration::from_secs(
                    PLAYBACK_FINISH_DB_LOCK_RETRY_DELAY_SECS,
                ))
                .await;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

async fn handle_runtime_track_error(
    state: SharedState,
    failed_track_id: i64,
    generation: u64,
    message: &str,
) -> anyhow::Result<()> {
    {
        let mut state_guard = state.write().await;
        if current_playback_generation(&state_guard) != generation {
            return Ok(());
        }
        state_guard
            .audio_active
            .store(false, std::sync::atomic::Ordering::Relaxed);
        if let Some(info) = state_guard.playback_runtime_info.as_mut() {
            info.last_error = Some(message.to_string());
            if info.active_track_id == Some(failed_track_id) {
                info.active_track_id = None;
            }
        }
    }

    tracing::warn!(
        target: "noor.playback.advance",
        event = "runtime_track_error",
        failed_track_id,
        generation,
        error = %message,
        "runtime track error; advancing queue"
    );
    report_playback_failure(&state, message);
    handle_runtime_finished_with_retry(state, failed_track_id, generation).await
}

async fn handle_prepared_runtime_track_error(state: &SharedState, track_id: i64, message: &str) {
    tracing::warn!(
        target: "noor.playback.advance",
        event = "prepared_track_error",
        track_id,
        error = %message,
        "prepared track failed; keeping current playback"
    );
    {
        let mut state_guard = state.write().await;
        if let Some(info) = state_guard.playback_runtime_info.as_mut() {
            info.last_error = Some(message.to_string());
        }
        let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    }
    report_playback_failure(state, message);
}

fn sqlite_database_locked(error: &anyhow::Error) -> bool {
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

async fn handle_runtime_error(state: SharedState, message: &str) {
    {
        let mut state_guard = state.write().await;
        if let Some(info) = state_guard.playback_runtime_info.as_mut() {
            info.last_error = Some(message.to_string());
            info.active_track_id = None;
        }
        state_guard.external_playback_track = None;
        state_guard.ephemeral_tidal_track = None;
        state_guard.prepared_ephemeral_tidal_next = None;
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

/// Broadcast a "we skipped this track" notice so the UI can toast which track
/// dropped out and why, instead of freezing on a silent dead row.
async fn emit_track_skipped(state: &SharedState, track_id: i64, title: &str, reason: &str) {
    let state_guard = state.read().await;
    let _ = state_guard.event_tx.send(AppEvent::TrackSkipped {
        track_id,
        title: title.to_string(),
        reason: reason.to_string(),
    });
}

/// TIDAL returned `4005 / asset-not-ready` for a track that used to play, which
/// usually means the catalog id rotated. In the background, search TIDAL for
/// the same artist + title, verify a candidate actually streams, and swap the
/// row's `tidal_id` in place so the track heals for next time. Best-effort:
/// any failure leaves the row untouched (it's already been skipped this time).
fn spawn_tidal_id_reresolve(state: &SharedState, track_id: i64) {
    let state = state.clone();
    tokio::spawn(async move {
        match reresolve_tidal_id(&state, track_id).await {
            Ok(Some(new_id)) => tracing::info!(
                target: "noor.playback.reresolve",
                track_id,
                new_tidal_id = new_id,
                "healed track with a fresh TIDAL id"
            ),
            Ok(None) => tracing::debug!(
                target: "noor.playback.reresolve",
                track_id,
                "no fresh TIDAL id found to heal track"
            ),
            Err(error) => tracing::debug!(
                target: "noor.playback.reresolve",
                track_id,
                error = %error,
                "TIDAL id re-resolve failed"
            ),
        }
    });
}

/// Returns `Some(new_tidal_id)` when the row was healed with a fresh, verified
/// id; `None` when no better id was found. Errors only on infrastructure
/// failures (DB, no TIDAL session), never on "couldn't find a match".
async fn reresolve_tidal_id(state: &SharedState, track_id: i64) -> anyhow::Result<Option<i64>> {
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
        match s.tidal_tokens.clone().or(persisted) {
            Some(tokens) => (tokens, s.tidal_http_client.clone()),
            None => return Ok(None),
        }
    };

    let client = TidalClient::with_http(
        http.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
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

    let new_id = candidate.id;
    let updated = db.with_conn(move |conn| {
        // Guard the UNIQUE(tidal_id): if another row already owns this id, leave
        // both alone rather than erroring.
        let clash: bool = conn
            .query_row(
                "SELECT 1 FROM tracks WHERE tidal_id = ?1 AND id != ?2",
                rusqlite::params![new_id, track_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if clash {
            return Ok(0usize);
        }
        Ok(conn.execute(
            "UPDATE tracks SET tidal_id = ?1, updated_at = datetime('now') WHERE id = ?2",
            rusqlite::params![new_id, track_id],
        )?)
    })?;

    Ok((updated > 0).then_some(new_id))
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
/// helper (or `effective_crossfade_ms` to apply the exclusive-mode policy).
/// Passing a hardcoded 0 outside that policy disables the per-engine fade-out
/// ramp and prevents `CrossfadeStart` from firing, which breaks crossfade
/// transitions.
/// Returns `configured` unless exclusive mode is on, in which case it returns 0.
/// Used by callsites that already have a snapshot's `crossfade_ms` and want the
/// same exclusive-mode override that `current_crossfade_ms` applies, without
/// re-querying `playback_state`.
async fn effective_crossfade_ms(state: &SharedState, configured: i32) -> i32 {
    let guard = state.read().await;
    let (exclusive, dj_engine_enabled) = guard
        .db
        .with_conn(|conn| {
            let settings = crate::db::audio_settings::load(conn)?;
            let dj_enabled = queries::is_dj_engine_enabled(conn)?;
            Ok((settings.exclusive_mode, dj_enabled))
        })
        .unwrap_or((false, false));
    effective_crossfade_for_exclusive(exclusive, dj_engine_enabled, configured)
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

    // Bit-perfect exclusive playback must not rewrite samples. DJ mode opts
    // into processing while keeping exclusive device ownership.
    let (exclusive, dj_engine_enabled) = guard
        .db
        .with_conn(|conn| {
            let settings = crate::db::audio_settings::load(conn)?;
            let dj_enabled = queries::is_dj_engine_enabled(conn)?;
            Ok((settings.exclusive_mode, dj_enabled))
        })
        .unwrap_or((false, false));
    effective_crossfade_for_exclusive(exclusive, dj_engine_enabled, configured)
}

fn effective_crossfade_for_exclusive(
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

fn should_skip_prebuffer_for_sample_rate_follow_format_change(
    exclusive_mode: bool,
    sample_rate_follow: bool,
    current_rate: u32,
    next_rate: Option<i32>,
    current_bit_depth: Option<i32>,
    next_bit_depth: Option<i32>,
) -> bool {
    if !sample_rate_follow {
        return false;
    }
    let rate_changes =
        next_rate.is_some_and(|next_rate| next_rate > 0 && next_rate as u32 != current_rate);
    let bit_depth_changes = matches!(
        (current_bit_depth, next_bit_depth),
        (Some(current), Some(next)) if current > 0 && next > 0 && current != next
    );
    rate_changes || (exclusive_mode && bit_depth_changes)
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

struct RuntimeOutputSettings {
    device: playback_runtime::OutputDeviceSelection,
    exclusive_mode: bool,
    sample_rate_follow: bool,
    exclusive_release_grace_secs: u32,
    exclusive_latency_mode: crate::db::audio_settings::ExclusiveLatencyMode,
}

fn runtime_output_settings_from_audio_settings(
    settings: &crate::db::audio_settings::AudioSettings,
) -> RuntimeOutputSettings {
    RuntimeOutputSettings {
        device: playback_runtime::OutputDeviceSelection::from_pref(
            settings.output_device.as_deref(),
        ),
        exclusive_mode: settings.exclusive_mode,
        sample_rate_follow: settings.sample_rate_follow,
        exclusive_release_grace_secs: settings.exclusive_release_grace_secs,
        exclusive_latency_mode: settings.exclusive_latency_mode.clone(),
    }
}

async fn apply_persisted_runtime_output_settings(
    state: &SharedState,
    handle: &playback_runtime::PlaybackRuntimeHandle,
) {
    let settings = {
        let guard = state.read().await;
        guard
            .db
            .with_conn(|conn| crate::db::audio_settings::load(conn).map_err(Into::into))
            .ok()
    };

    let Some(settings) = settings else {
        return;
    };
    let output = runtime_output_settings_from_audio_settings(&settings);
    if let Err(e) = handle.device_swap(
        output.device,
        output.exclusive_mode,
        output.sample_rate_follow,
        None,
        output.exclusive_release_grace_secs,
        output.exclusive_latency_mode,
    ) {
        warn!("Failed to apply persisted audio settings to playback runtime: {e}");
    }
}

// ----- Audio output settings ------------------------------------------------
//
// `GET /api/audio/devices`: enumerate cpal output devices
// `GET /api/audio/settings`: current persisted AudioSettings
// `PUT /api/audio/settings`: persist and live-swap when output settings change
// `POST /api/audio/exclusive/retry`: force a fresh DeviceSwap to retry exclusive grab

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

    if let Some(runtime) = guard.playback_runtime.as_ref() {
        let output = runtime_output_settings_from_audio_settings(&settings);
        if let Err(e) = runtime.handle.device_swap(
            output.device,
            output.exclusive_mode,
            output.sample_rate_follow,
            None,
            output.exclusive_release_grace_secs,
            output.exclusive_latency_mode,
        ) {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "message": e.to_string() })),
            ));
        }
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

fn should_retry_exclusive_release(is_playing: bool, exclusive_mode: bool) -> bool {
    is_playing && exclusive_mode
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
            || old.exclusive_release_grace_secs != saved.exclusive_release_grace_secs
            || old.exclusive_latency_mode != saved.exclusive_latency_mode;

        if needs_swap && let Some(runtime) = guard.playback_runtime.as_ref() {
            let output = runtime_output_settings_from_audio_settings(&saved);
            if let Err(e) = runtime.handle.device_swap(
                output.device,
                output.exclusive_mode,
                output.sample_rate_follow,
                None,
                output.exclusive_release_grace_secs,
                output.exclusive_latency_mode,
            ) {
                warn!("Audio settings update: live device_swap failed: {e}");
            }
        }

        (old, saved)
    };

    // Quality changed: re-issue the current track at the new quality so the
    // user immediately hears (and sees) the new tier. The track restarts from
    // 0; preserving position would require partial-stream offset support that
    // TIDAL's playbackinfo API doesn't expose.
    if old.quality != new.quality
        && let Err(e) = reissue_current_track_at_new_quality(&state).await
    {
        warn!("Audio settings update: re-issue at new quality failed: {e}");
    }

    // Clear live state here so the UI cannot keep showing Excl if the runtime
    // release event races or never arrives.
    if old.exclusive_mode && !new.exclusive_mode {
        let mut guard = state.write().await;
        let released_device = guard.playback_runtime_info.as_mut().map(|info| {
            info.exclusive_engaged = false;
            info.exclusive_transport_format = None;
            info.device_name.clone()
        });
        if let Some(device) = released_device {
            let _ = guard
                .event_tx
                .send(AppEvent::AudioExclusiveReleased { device });
        }
    }

    Ok(Json(new))
}

/// Re-resolve the currently-playing track at the user's current quality and
/// switch the runtime to it. Called after `put_audio_settings` when the user
/// flips the quality dropdown. Without this, quality changes don't take effect
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
    let generation = bump_playback_generation(state).await;
    let job =
        player::build_playback_preparation(&track, Some(&stream_info), crossfade_ms, user_quality)
            .with_generation(generation);

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

        let mut now_playing: Option<crate::services::scrobbling::ScrobblePayload> = None;
        if snapshot.state.is_playing {
            if let Some(track) = snapshot.state.current_track.as_ref() {
                let source = state
                    .db
                    .with_conn(|conn| Ok(player::lookup_current_listen_source(conn)))
                    .unwrap_or(crate::db::models::ListenSource::Unknown);
                let prior = state.live_listen_session.as_ref();
                let dj_transition_event_id = state
                    .db
                    .with_conn(|conn| {
                        player::latest_open_dj_transition_event_for_pair(
                            conn,
                            prior.map(|session| session.last_track_id),
                            track.id,
                        )
                    })
                    .unwrap_or(None);
                state.active_listen_session = Some(
                    player::ActiveListenSession::start(track.id, now, source, prior)
                        .with_dj_transition_event_id(dj_transition_event_id),
                );
                now_playing = Some(crate::services::scrobbling::ScrobblePayload {
                    track_id: Some(track.id),
                    artist: track.artist_name.clone().unwrap_or_default(),
                    title: track.title.clone(),
                    album: track.album_title.clone(),
                    duration_ms: track.duration_ms,
                    listened_ms: None,
                    started_at_unix: Some(now.timestamp()),
                });
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

    if let Some(payload) = scrobble_completed_payload {
        crate::services::scrobbling::enqueue_completed(state.clone(), payload).await;
    }
    if let Some(payload) = now_playing_payload {
        crate::services::scrobbling::enqueue_now_playing(state.clone(), payload).await;
    }
}

pub(crate) struct FlushOutcome {
    flushed_track_id: Option<i64>,
    scrobble_completed: Option<crate::services::scrobbling::ScrobblePayload>,
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
    let dj_transition_event_id = session.dj_transition_event_id;
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
        player::record_dj_transition_listen_outcome(
            conn,
            dj_transition_event_id,
            listened_ms,
            completed,
        )?;
        let payload = crate::services::scrobbling::ScrobblePayload {
            track_id: Some(track_id),
            artist: track.artist_name.clone().unwrap_or_default(),
            title: track.title.clone(),
            album: track.album_title.clone(),
            duration_ms: track.duration_ms,
            listened_ms: Some(listened_ms),
            started_at_unix: Some(started_at_unix),
        };
        Ok((completed, payload))
    });
    // On DB error: restore the session so the next flush attempt retries
    // (helpful for shutdown_handler and clear_tidal_session, which don't
    // immediately start a replacement session). Track-transition flushes
    // are still racy. The next sync_session_after_snapshot will overwrite
    // active_listen_session with a new track's session, but those weren't
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
            let token_blob = tidal_auth::encode_persisted_tidal_tokens(&s.master_key, tokens)?;
            let token_expiry = (chrono::Utc::now()
                + chrono::Duration::seconds(tokens.expires_in.max(0)))
            .to_rfc3339();
            conn.execute(
                "INSERT INTO service_auth (service, access_token_enc, user_id, token_expiry, connected_at)
                 VALUES ('tidal', ?1, ?2, ?3, datetime('now'))
                 ON CONFLICT(service) DO UPDATE SET access_token_enc=excluded.access_token_enc,
                 user_id=excluded.user_id, token_expiry=excluded.token_expiry, connected_at=excluded.connected_at",
                rusqlite::params![token_blob, tokens.user_id, token_expiry],
            )?;
            Ok(())
        })?;
    }

    let mut s = state.write().await;
    s.tidal_tokens = Some(tokens.clone());
    Ok(())
}

/// Upsert a TIDAL track (and its artist) and return the local `tracks.id`.
/// The id is looked up here anyway to attach source genres, so callers that
/// need it should use the return value rather than issuing a second SELECT.
pub(super) fn insert_tidal_track(
    conn: &rusqlite::Connection,
    track: &crate::services::tidal::client::TidalTrack,
    is_favorite: bool,
    favorite_created: Option<&str>,
) -> anyhow::Result<Option<i64>> {
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
        // Every caller is a genuine TIDAL library sync (favorite tracks,
        // favorited-album tracks, playlist tracks), so is_library=1. The
        // ON CONFLICT MAX self-heals: a row first seen as a transient import
        // (is_library=0) is promoted to library when a real sync touches it,
        // and is never demoted. See MIGRATION_052.
        "INSERT INTO tracks (tidal_id, title, artist_id, album_id, disc_number, track_number, duration_ms, isrc, best_quality, best_source, fidelity_score, is_favorite, source, date_added, is_library)
         VALUES (?1, ?2, (SELECT id FROM artists WHERE tidal_id=?3), (SELECT id FROM albums WHERE tidal_id=?4), ?5, ?6, ?7, ?8, ?9, 'tidal', ?10, ?11, 'tidal', COALESCE(?12, datetime('now')), 1)
         ON CONFLICT(tidal_id) DO UPDATE SET
            title=excluded.title, best_quality=excluded.best_quality,
            fidelity_score=MAX(tracks.fidelity_score, excluded.fidelity_score),
            is_favorite=MAX(tracks.is_favorite, excluded.is_favorite),
            is_library=MAX(tracks.is_library, excluded.is_library),
            date_added=CASE
                WHEN ?11 = 1 AND ?12 IS NOT NULL THEN excluded.date_added
                ELSE tracks.date_added
            END",
        rusqlite::params![
            track.id, track.title, track.artist.id, album_tidal_id,
            track.volume_number.unwrap_or(1), track.track_number,
            track.duration * 1000, track.isrc,
            quality, fidelity, is_favorite as i32, favorite_created,
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

    Ok(local_track_id)
}

pub(super) fn apply_tidal_favorite_flags(
    conn: &rusqlite::Connection,
    table: &str,
    favorite_ids: &HashSet<i64>,
    prev_count: i64,
) -> anyhow::Result<()> {
    // Refuse to wipe favorites if this run somehow returned zero items but the
    // previous run had a real population, almost always a transient TIDAL API
    // hiccup, not a legitimate "user unfavorited everything".
    if favorite_ids.is_empty() && prev_count > 0 {
        anyhow::bail!(
            "Refusing to clear is_favorite on '{}': sync returned 0 favorites but previous run had {}",
            table,
            prev_count
        );
    }

    // Scope the reset to TIDAL-sourced rows so manually-imported albums/tracks
    // (e.g. from `import_tidal_album`) keep whatever favorite state they had:
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
mod tests;
