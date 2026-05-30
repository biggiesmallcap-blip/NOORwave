use crate::db::queries;
use crate::metadata::discogs::DiscogsClient;
use crate::metadata::lastfm::LastFmClient;
use crate::playback::{automix, pending, player, queue, runtime as playback_runtime};
use crate::services::discovery::{
    DiscoveryCandidateSeed, DiscoveryProvider, TidalDiscoveryProvider,
};
use crate::services::discovery_blend as blend;
use crate::services::discovery_space as ds;
use crate::services::learning as discovery_learning;
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
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

mod analytics_routes;
mod audio_analysis_routes;
mod chart_routes;
mod discovery_routes;
mod dj_routes;
mod duplicates_routes;
mod enrichment_routes;
mod genre_routes;
mod library_batch_routes;
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

#[derive(Debug, Deserialize)]
struct AddTracksToPlaylistRequest {
    track_ids: Vec<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateSmartPlaylistRequest {
    name: String,
    description: Option<String>,
    /// The root `RuleClause` as a raw JSON value - validated by deserializing into
    /// `SmartPlaylistDefinition` before writing to DB.
    rules: Value,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSmartPlaylistRequest {
    name: String,
    description: Option<String>,
    rules: Value,
}

#[derive(Debug, Deserialize)]
pub struct PreviewSmartPlaylistRequest {
    /// Optional name/description carried over from the editor draft - the
    /// preview doesn't persist anything so these are decorative.
    name: Option<String>,
    description: Option<String>,
    rules: Value,
}

#[derive(Debug, Deserialize)]
pub struct ArtistSearchParams {
    q: Option<String>,
    limit: Option<i64>,
}

pub fn api_routes(state: SharedState) -> Router {
    Router::new()
        // Library endpoints
        .route("/api/tracks", get(get_tracks))
        .route("/api/tracks/count", get(get_track_count))
        .route("/api/albums", get(get_albums))
        .route("/api/albums/{id}/tracks", get(get_album_tracks))
        .route(
            "/api/albums/{id}/spotify-stats",
            get(get_album_spotify_stats),
        )
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
        .route("/api/playlists", get(get_playlists))
        .route(
            "/api/playlists/{id}/tracks",
            get(get_playlist_tracks).post(add_tracks_to_playlist_route),
        )
        .route(
            "/api/playlists/{id}/favorite",
            patch(toggle_playlist_favorite_route),
        )
        .route(
            "/api/playlists/{id}/cover-sample",
            get(get_playlist_cover_sample),
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
        .route("/api/smart/playlists/preview", post(preview_smart_playlist))
        .route("/api/artists/search", get(search_artists_route))
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
        .route("/api/discovery/space", post(get_discovery_space))
        .route(
            "/api/discovery/blend/space",
            post(get_discovery_blend_space),
        )
        .route(
            "/api/discovery/blend/add",
            post(add_discovery_blend_to_queue),
        )
        .route("/api/discovery/blend/play", post(play_discovery_blend))
        .route(
            "/api/discovery/blend/radio",
            post(make_discovery_blend_radio),
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
        .route("/api/radio/song", post(radio_song))
        .route("/api/radio/album", post(radio_album))
        .route("/api/radio/artist", post(radio_artist))
        .route("/api/radio/start", post(radio_start))
        .route("/api/discovery/space/meta", get(get_discovery_space_meta))
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
        .route("/api/tidal/playlists/search", get(tidal_playlist_search))
        .route(
            "/api/tidal/playlists/{uuid}/tracks",
            get(tidal_playlist_tracks),
        )
        .route("/api/tidal/play", post(play_tidal_ephemeral))
        .route("/api/tidal/artists/{tidal_id}", get(tidal_artist_profile))
        .route("/api/tidal/logout", post(tidal_logout))
        // Spotify
        .route(
            "/api/spotify/config",
            post(enrichment_routes::spotify_save_config),
        )
        .route(
            "/api/spotify/config",
            axum::routing::delete(enrichment_routes::spotify_clear_config),
        )
        .route(
            "/api/spotify/status",
            get(enrichment_routes::spotify_status),
        )
        .route(
            "/api/library/enrich/spotify",
            post(enrichment_routes::start_spotify_enrichment),
        )
        .route(
            "/api/library/enrich/spotify/status",
            get(enrichment_routes::get_spotify_enrichment_status),
        )
        .route(
            "/api/library/enrich/spotify/reset",
            post(enrichment_routes::reset_spotify_enrichment),
        )
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
        .route("/api/home/releases", get(get_home_releases))
        .route("/api/home/picks", get(get_home_picks))
        .route("/api/home/recommendations", get(get_home_recommendations))
        .route("/api/home/articles", get(get_home_articles))
        .route("/api/home/news", get(get_home_news))
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
        // charts / moods / genres / new-releases (single segment) and
        // mood/{id} / genre/{id} (two segments). Universal across the
        // /v1/pages/* response shape.
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

    // No TIDAL id -> can't enrich; return library tracks alone.
    let Some(tidal_album_id) = album_tidal_id else {
        return Ok(Json(json!({
            "tracks": tracks,
            "tidal_tracks": [],
            "album_tidal_id": null,
        })));
    };

    // TIDAL session needed for the catalog fetch - best-effort only.
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

    // Pre-fix legacy rows that landed with NULL track_number — `TidalTrack`
    // shipped without #[serde(rename = "trackNumber")] for a long time, so
    // every TIDAL-imported track row stored NULL and the album sort fell
    // back to alphabetical. Backfill from the live TIDAL payload on first
    // view post-fix.
    let needs_backfill = tracks
        .iter()
        .any(|t| t.track_number.is_none() && t.tidal_id.is_some());

    let tidal_tracks_payload: Vec<Value> = match client.get_album_tracks(tidal_album_id).await {
        Ok(resp) => {
            if needs_backfill {
                let backfill_pairs: Vec<(i64, i32, i32)> = resp
                    .items
                    .iter()
                    .filter_map(|t| Some((t.id, t.track_number?, t.volume_number.unwrap_or(1))))
                    .collect();
                if !backfill_pairs.is_empty() {
                    let s = state.read().await;
                    let _ = s.db.with_conn(|conn| {
                        let tx = conn.unchecked_transaction()?;
                        let mut count = 0i64;
                        {
                            let mut stmt = tx.prepare(
                                "UPDATE tracks
                                 SET track_number = COALESCE(track_number, ?2),
                                     disc_number  = COALESCE(disc_number, ?3)
                                 WHERE tidal_id = ?1
                                   AND (track_number IS NULL OR disc_number IS NULL)",
                            )?;
                            for (tid, tn, dn) in &backfill_pairs {
                                count += stmt.execute(rusqlite::params![tid, tn, dn])? as i64;
                            }
                        }
                        tx.commit()?;
                        if count > 0 {
                            tracing::info!(
                                target: "noor.album",
                                event = "tracknumber_backfill",
                                album_id = id,
                                updated = count
                            );
                        }
                        Ok::<_, anyhow::Error>(())
                    });
                }
            }

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

    // Reload tracks so the response reflects backfilled track/disc numbers
    // (the original `get_album_tracks` query ordered by COALESCE(..., 999999)
    // and may have produced alphabetical fallback order before the UPDATE).
    let tracks = if needs_backfill {
        let s = state.read().await;
        s.db.with_conn(|conn| queries::get_album_tracks(conn, id))
            .unwrap_or(tracks)
    } else {
        tracks
    };

    Ok(Json(json!({
        "tracks": tracks,
        "tidal_tracks": tidal_tracks_payload,
        "album_tidal_id": tidal_album_id,
    })))
}

async fn get_album_spotify_stats(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> Json<Value> {
    #[cfg(feature = "spotify-public")]
    {
        let (db, client, tracks) = {
            let s = state.read().await;
            let tracks =
                s.db.with_conn(|conn| queries::get_album_tracks(conn, id))
                    .unwrap_or_default();
            (s.db.clone(), s.spotify_public.clone(), tracks)
        };

        let seeds: Vec<crate::services::spotify_public::TrackSeed> = tracks
            .iter()
            .filter_map(|t| {
                let isrc = t.isrc.as_deref()?.trim();
                if isrc.is_empty() {
                    return None;
                }
                Some(crate::services::spotify_public::TrackSeed {
                    isrc: isrc.to_string(),
                    title: t.title.clone(),
                    artist_name: t.artist_name.clone().unwrap_or_default(),
                })
            })
            .collect();

        let stats =
            crate::services::spotify_public::fetch_album_playcounts(&client, &db, &seeds).await;

        let payload: Vec<Value> = stats
            .into_iter()
            .filter_map(|s| {
                s.playcount.map(|pc| {
                    json!({
                        "isrc": s.isrc,
                        "title": s.title,
                        "playcount": pc,
                    })
                })
            })
            .collect();

        Json(json!({
            "monthly_listeners": null,
            "tracks": payload,
        }))
    }
    #[cfg(not(feature = "spotify-public"))]
    {
        let _ = (state, id);
        Json(json!({
            "monthly_listeners": null,
            "tracks": [],
        }))
    }
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
    // Top tracks raised from 10 -> 50 so the merged Top Tracks list on the
    // artist page surfaces a meaningful catalog even when the user has zero
    // library matches; 50 is TIDAL's per-page max.
    let top_fut = client.get_artist_top_tracks(tidal_artist_id, 50, 0);
    let videos_fut = client.get_artist_videos(tidal_artist_id, 50, 0);
    let similar_fut = client.get_artist_similar(tidal_artist_id, 20, 0);
    let bio_fut = client.get_artist_bio(tidal_artist_id);
    // Profile fetch in the same parallel batch - gives us the artist's
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
    // tracks, then finally fall back to an album cover - same trick the
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
    // for artist pictures - many artist images are stored at 320 max.
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
    tracing::debug!(
        "TIDAL artist {} picture resolution: direct={}, top_track={}, album_cover={}, resolved={}",
        tidal_artist_id,
        direct_some,
        top_track_some,
        album_cover_some,
        picture_url.is_some()
    );

    // TIDAL can return the same release under multiple filters (e.g. an album
    // re-issue tagged both ALBUMS and COMPILATIONS). Dedupe by tidal_id while
    // preserving the order of first appearance. Each entry remembers the
    // filter it came from so the frontend can bucket it correctly — TIDAL's
    // per-album `release_type` body field is unreliable and was the original
    // reason Singles / Compilations sections were silently empty.
    let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut all_albums: Vec<(crate::services::tidal::client::TidalAlbum, &'static str)> =
        Vec::new();
    for (r, filter) in [
        (albums_res, "ALBUMS"),
        (eps_res, "EPSANDSINGLES"),
        (comps_res, "COMPILATIONS"),
        (live_res, "LIVE"),
    ] {
        let Ok(items) = r else { continue };
        for item in items {
            if seen.insert(item.id) {
                all_albums.push((item, filter));
            }
        }
    }

    let tidal_album_ids: Vec<i64> = all_albums.iter().map(|(a, _)| a.id).collect();
    let known_map = {
        let s = state.read().await;
        s.db.with_conn(|conn| queries::get_known_album_tidal_ids(conn, &tidal_album_ids))
            .unwrap_or_default()
    };

    let albums_payload: Vec<Value> = all_albums
        .into_iter()
        .map(|(a, source_filter)| {
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
                "source_filter": source_filter,
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
    // for albums above - lets the frontend route /artists/[local_id] when
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
    #[cfg(feature = "spotify-public")]
    {
        let (db, client, tidal_id, artist_name, seeds) = {
            let s = state.read().await;
            let tidal_id =
                s.db.with_conn(|conn| queries::get_artist_tidal_id(conn, id))
                    .unwrap_or(None);
            let artist_tracks =
                s.db.with_conn(|conn| queries::get_artist_tracks(conn, id))
                    .unwrap_or_default();

            let mut sorted = artist_tracks
                .into_iter()
                .filter(|t| t.isrc.as_deref().is_some_and(|s| !s.trim().is_empty()))
                .collect::<Vec<_>>();
            sorted.sort_by(|a, b| b.play_count.cmp(&a.play_count));
            sorted.truncate(10);

            let artist_name = sorted
                .first()
                .and_then(|t| t.artist_name.clone())
                .unwrap_or_default();

            let seeds: Vec<crate::services::spotify_public::TrackSeed> = sorted
                .into_iter()
                .map(|t| crate::services::spotify_public::TrackSeed {
                    isrc: t.isrc.unwrap_or_default(),
                    title: t.title,
                    artist_name: t.artist_name.unwrap_or_default(),
                })
                .collect();

            (
                s.db.clone(),
                s.spotify_public.clone(),
                tidal_id,
                artist_name,
                seeds,
            )
        };

        // Local-only artist (no Tidal id): no key for the map table, so just
        // serve any cached track playcounts via the album helper, no artist
        // resolution attempted, no negative-cache row written.
        let Some(tidal_id) = tidal_id else {
            let stats =
                crate::services::spotify_public::fetch_album_playcounts(&client, &db, &seeds).await;
            let tracks: Vec<Value> = stats
                .into_iter()
                .filter_map(|s| {
                    s.playcount.map(|pc| {
                        json!({
                            "isrc": s.isrc,
                            "title": s.title,
                            "playcount": pc,
                        })
                    })
                })
                .collect();
            return Json(json!({
                "monthly_listeners": null,
                "followers": null,
                "world_rank": null,
                "top_cities": [],
                "tracks": tracks,
            }));
        };

        let tidal_id_str = tidal_id.to_string();
        let result = crate::services::spotify_public::fetch_artist_stats(
            &client,
            &db,
            &tidal_id_str,
            &artist_name,
            &seeds,
        )
        .await;

        let tracks: Vec<Value> = result
            .tracks
            .into_iter()
            .filter_map(|t| {
                t.playcount.map(|pc| {
                    json!({
                        "isrc": t.isrc,
                        "title": t.title,
                        "playcount": pc,
                    })
                })
            })
            .collect();

        Json(json!({
            "monthly_listeners": result.monthly_listeners,
            "followers": result.followers,
            "world_rank": result.world_rank,
            "top_cities": result.top_cities,
            "tracks": tracks,
        }))
    }
    #[cfg(not(feature = "spotify-public"))]
    {
        let _ = (state, id);
        Json(json!({
            "monthly_listeners": null,
            "followers": null,
            "world_rank": null,
            "top_cities": [],
            "tracks": [],
        }))
    }
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

    let items = result.items;
    let tidal_ids: Vec<i64> = items.iter().map(|t| t.id).collect();
    let library_states = {
        let s = state.read().await;
        s.db.with_conn(|conn| queries::get_tidal_track_library_states(conn, &tidal_ids))
            .unwrap_or_default()
    };
    let tracks: Vec<Value> = items
        .into_iter()
        .map(|t| {
            let library_state = library_states.get(&t.id).copied();
            tidal_track_playable_json(t, library_state, 160)
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
        .map(|t| {
            json!({
                "tidal_id": t.tidal_id,
                "local_id": t.local_id,
                "artist_id": t.artist_id,
                "album_id": t.album_id,
            })
        })
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
        "artist_id": imported.artist_id,
        "album_id": imported.album_id,
    })))
}

async fn get_playlists(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
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

/// Up to four distinct album-artwork URLs for the cover mosaic on /playlists.
/// Regular playlists return without scanning every track. Smart playlists
/// still need to evaluate their rules, but the response payload is four
/// short strings instead of the entire track list.
async fn get_playlist_cover_sample(
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
async fn preview_smart_playlist(
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
async fn search_artists_route(
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
    // gates on the same idle check as the auto path — clicking the button does
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
/// Powers the Settings "Build radio similarity index" panel — the frontend
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

/// Stable synthetic id for an external (Last.fm) candidate that has no resolved
/// Tidal id. Negative i64 keyed off `artist|title` so multiple unresolved hits
/// don't all collapse onto the same `track-0` node on the canvas. Hash collisions
/// are negligible at the ~60-candidate scale of a single radio request.
///
/// TODO(option 2): Replace this with real Tidal-search resolution in `radio.rs`
/// before the candidate leaves the orchestrator - that would also let
/// `DiscoverSidePanel.resolveExternalPlayable` go away. Needs an artist+title ->
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
struct DiscoveryBlendRequest {
    seeds: Vec<blend::BlendSeedInput>,
    limit: Option<i64>,
}

fn blend_seed_error_response(error: blend::BlendSeedError) -> (StatusCode, Json<Value>) {
    let message = match error {
        blend::BlendSeedError::Empty => "at least one blend seed is required",
        blend::BlendSeedError::TooMany => "blend supports up to four seeds",
        blend::BlendSeedError::InvalidIdentity => "blend seed is missing a valid identity",
        blend::BlendSeedError::Duplicate => "duplicate blend seeds are not allowed",
    };
    (StatusCode::BAD_REQUEST, Json(json!({ "error": message })))
}

fn parse_discovery_reason_tags(reason_json: Option<&str>, fallback: &str) -> Vec<String> {
    let raw_tags = reason_json
        .and_then(|raw| serde_json::from_str::<Vec<Value>>(raw).ok())
        .unwrap_or_default()
        .iter()
        .filter_map(|value| {
            value
                .get("key")
                .and_then(|key| key.as_str())
                .or_else(|| value.get("label").and_then(|label| label.as_str()))
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    let mut reason_tags = ds::normalize_reason_tags(&raw_tags);
    if reason_tags.is_empty() {
        reason_tags.push(ds::normalize_reason(fallback).to_string());
    }
    reason_tags
}

fn blend_anchor(index: usize, count: usize) -> (f64, f64) {
    match count {
        0 | 1 => (0.0, 0.0),
        2 => {
            if index == 0 {
                (-260.0, 0.0)
            } else {
                (260.0, 0.0)
            }
        }
        _ => {
            let angle = (index as f64 / count as f64) * std::f64::consts::PI * 2.0
                - std::f64::consts::FRAC_PI_2;
            (angle.cos() * 280.0, angle.sin() * 220.0)
        }
    }
}

fn layout_blend_candidate(
    candidate: &mut blend::ScoredBlendCandidate,
    seed_count: usize,
    index: usize,
) {
    if candidate.role == blend::CandidateRole::Seed {
        return;
    }
    if seed_count == 2 {
        let left = candidate
            .per_seed_scores
            .first()
            .map(|score| score.score)
            .unwrap_or(0.0);
        let right = candidate
            .per_seed_scores
            .get(1)
            .map(|score| score.score)
            .unwrap_or(0.0);
        let total = (left + right).max(0.001);
        let t = right / total;
        candidate.x = -260.0 + t * 520.0;
        candidate.y = ((index as f64 * 37.0).sin() * 90.0)
            + (1.0 - candidate.blend_score.clamp(0.0, 1.0)) * 120.0;
        return;
    }

    let mut x = 0.0;
    let mut y = 0.0;
    let mut total = 0.0;
    for (seed_index, seed_score) in candidate.per_seed_scores.iter().enumerate() {
        let (anchor_x, anchor_y) = blend_anchor(seed_index, seed_count);
        let weight = seed_score.score.max(0.0);
        x += anchor_x * weight;
        y += anchor_y * weight;
        total += weight;
    }
    if total > 0.0 {
        candidate.x = x / total + (index as f64 * 19.0).sin() * 45.0;
        candidate.y = y / total + (index as f64 * 23.0).cos() * 45.0;
    } else {
        let angle = (index as f64 / (seed_count.max(1) as f64)) * std::f64::consts::PI * 2.0;
        candidate.x = angle.cos() * 180.0;
        candidate.y = angle.sin() * 140.0;
    }
}

fn resolve_external_blend_seed_anchor(
    conn: &rusqlite::Connection,
    seed: &blend::BlendSeed,
    model_id: Option<i64>,
) -> rusqlite::Result<Option<i64>> {
    if let Some(tidal_id) = seed.tidal_id.filter(|id| *id > 0) {
        if let Some(track_id) = conn
            .query_row(
                "SELECT id FROM tracks WHERE tidal_id = ?1 LIMIT 1",
                params![tidal_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        {
            return Ok(Some(track_id));
        }

        return conn
            .query_row(
                "SELECT COALESCE(c.resolved_track_id, n.library_track_id)
                 FROM external_track_candidates c
                 LEFT JOIN external_track_candidate_neighbors n
                   ON n.candidate_id = c.id
                  AND (?2 IS NULL OR n.model_id = ?2)
                 WHERE c.tidal_id = ?1
                   AND COALESCE(c.resolved_track_id, n.library_track_id) IS NOT NULL
                 ORDER BY
                   CASE WHEN c.resolved_track_id IS NOT NULL THEN 0 ELSE 1 END,
                   n.score DESC,
                   n.rank ASC
                 LIMIT 1",
                params![tidal_id, model_id],
                |row| row.get::<_, i64>(0),
            )
            .optional();
    }

    let artist = seed.artist.as_deref().unwrap_or("").trim();
    let title = seed.title.as_deref().unwrap_or("").trim();
    if artist.is_empty() || title.is_empty() {
        return Ok(None);
    }

    conn.query_row(
        "SELECT COALESCE(c.resolved_track_id, n.library_track_id)
         FROM external_track_candidates c
         LEFT JOIN external_track_candidate_neighbors n
           ON n.candidate_id = c.id
          AND (?3 IS NULL OR n.model_id = ?3)
         WHERE lower(trim(c.title)) = lower(trim(?1))
           AND lower(trim(c.artist_name)) = lower(trim(?2))
           AND COALESCE(c.resolved_track_id, n.library_track_id) IS NOT NULL
         ORDER BY
           CASE WHEN c.resolved_track_id IS NOT NULL THEN 0 ELSE 1 END,
           n.score DESC,
           n.rank ASC
         LIMIT 1",
        params![title, artist, model_id],
        |row| row.get::<_, i64>(0),
    )
    .optional()
}

fn resolve_blend_seed_anchors(
    conn: &rusqlite::Connection,
    seeds: &mut [blend::BlendSeed],
    model_id: Option<i64>,
) -> rusqlite::Result<()> {
    for seed in seeds {
        if seed.kind == blend::BlendSeedKind::Library {
            seed.anchor_track_id = seed.track_id;
        } else if seed.anchor_track_id.is_none() {
            seed.anchor_track_id = resolve_external_blend_seed_anchor(conn, seed, model_id)?;
        }
    }
    Ok(())
}

fn seed_node_from_track(
    seed: &blend::BlendSeed,
    index: usize,
    count: usize,
    row: (
        i64,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
    ),
) -> blend::ScoredBlendCandidate {
    let (x, y) = blend_anchor(index, count);
    blend::ScoredBlendCandidate {
        identity: seed.identity.clone(),
        title: row.1,
        artist_name: row.2.unwrap_or_default(),
        album_title: row.3,
        artwork_url: row.4,
        duration_ms: row.5,
        track_id: Some(row.0),
        tidal_id: seed.tidal_id,
        is_in_library: true,
        source: "library".to_string(),
        reason_tags: vec!["seed".to_string()],
        role: blend::CandidateRole::Seed,
        playability: blend::Playability::Playable,
        per_seed_scores: vec![blend::SeedScore {
            seed_identity: seed.identity.clone(),
            seed_track_id: seed.anchor_track_id.or(seed.track_id),
            score: 1.0,
        }],
        covered_seed_count: 1,
        weighted_seed_proximity: 1.0,
        coverage_bonus: 0.0,
        external_bonus: 0.0,
        library_penalty: 0.0,
        confidence_bonus: 0.0,
        blend_score: 1.0,
        x,
        y,
    }
}

fn seed_node_from_external_seed(
    seed: &blend::BlendSeed,
    index: usize,
    count: usize,
) -> blend::ScoredBlendCandidate {
    let (x, y) = blend_anchor(index, count);
    blend::ScoredBlendCandidate {
        identity: seed.identity.clone(),
        title: seed
            .title
            .clone()
            .unwrap_or_else(|| "TIDAL seed".to_string()),
        artist_name: seed.artist.clone().unwrap_or_default(),
        album_title: None,
        artwork_url: None,
        duration_ms: None,
        track_id: None,
        tidal_id: seed.tidal_id,
        is_in_library: false,
        source: "external".to_string(),
        reason_tags: vec!["seed".to_string()],
        role: blend::CandidateRole::Seed,
        playability: if seed.tidal_id.is_some() {
            blend::Playability::Resolvable
        } else {
            blend::Playability::Pending
        },
        per_seed_scores: vec![blend::SeedScore {
            seed_identity: seed.identity.clone(),
            seed_track_id: seed.anchor_track_id.or(seed.track_id),
            score: 1.0,
        }],
        covered_seed_count: 1,
        weighted_seed_proximity: 1.0,
        coverage_bonus: 0.0,
        external_bonus: 0.0,
        library_penalty: 0.0,
        confidence_bonus: 0.0,
        blend_score: 1.0,
        x,
        y,
    }
}

fn build_discovery_blend_space(
    conn: &rusqlite::Connection,
    seeds: &mut [blend::BlendSeed],
    limit: i64,
    library_cap_ratio: f64,
) -> anyhow::Result<(Vec<blend::ScoredBlendCandidate>, Value)> {
    let model = queries::get_selected_discovery_embedding_model(conn)?;
    resolve_blend_seed_anchors(conn, seeds, model.as_ref().map(|model| model.id))?;
    let seed_count = seeds.len();
    let library_seed_ids = seeds
        .iter()
        .filter_map(|seed| seed.anchor_track_id.or(seed.track_id))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let mut seed_nodes = Vec::new();
    for (index, seed) in seeds.iter().enumerate() {
        if seed.kind == blend::BlendSeedKind::Library && seed.track_id.is_some() {
            let track_id = seed.track_id.unwrap_or_default();
            let row = conn
                .query_row(
                    "SELECT t.id, t.title, ar.name, al.title, al.artwork_url, t.duration_ms
                     FROM tracks t
                     LEFT JOIN artists ar ON t.artist_id = ar.id
                     LEFT JOIN albums al ON t.album_id = al.id
                     WHERE t.id = ?1",
                    params![track_id],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, Option<i64>>(5)?,
                        ))
                    },
                )
                .optional()?;
            if let Some(row) = row {
                seed_nodes.push(seed_node_from_track(seed, index, seed_count, row));
            }
        } else {
            seed_nodes.push(seed_node_from_external_seed(seed, index, seed_count));
        }
    }

    let mut candidate_inputs: HashMap<String, blend::BlendCandidateInput> = HashMap::new();
    if let Some(model) = model {
        let per_seed_limit = limit.max(1).min(200);
        let seed_id_set = library_seed_ids.iter().copied().collect::<HashSet<_>>();
        let seed_identity_set = seeds
            .iter()
            .map(|seed| seed.identity.clone())
            .collect::<HashSet<_>>();
        let library_neighbors = queries::get_track_neighbors_for_seeds(
            conn,
            model.id,
            &library_seed_ids,
            per_seed_limit,
        )?;
        for (seed_id, rows) in library_neighbors {
            for row in rows {
                if seed_id_set.contains(&row.track_id) {
                    continue;
                }
                let identity = format!("library:{}", row.track_id);
                if seed_identity_set.contains(&identity) {
                    continue;
                }
                let reason_tags =
                    parse_discovery_reason_tags(row.reason_json.as_deref(), "library_match");
                let entry = candidate_inputs.entry(identity.clone()).or_insert_with(|| {
                    blend::BlendCandidateInput {
                        identity: identity.clone(),
                        title: row.title.clone(),
                        artist_name: row.artist_name.clone().unwrap_or_default(),
                        album_title: row.album_title.clone(),
                        artwork_url: row.artwork_url.clone(),
                        duration_ms: row.duration_ms,
                        track_id: Some(row.track_id),
                        tidal_id: None,
                        is_in_library: true,
                        confidence: row.confidence,
                        source: "library".to_string(),
                        reason_tags: reason_tags.clone(),
                        per_seed_scores: Vec::new(),
                    }
                });
                entry.per_seed_scores.push((seed_id, row.score));
            }
        }

        for seed_id in &library_seed_ids {
            let rows = queries::get_external_candidate_neighbors(
                conn,
                model.id,
                *seed_id,
                per_seed_limit,
                false,
            )?;
            for row in rows {
                let identity = row
                    .tidal_id
                    .filter(|id| *id > 0)
                    .map(|id| format!("tidal:{id}"))
                    .unwrap_or_else(|| blend::pending_seed_identity(&row.artist_name, &row.title));
                if seed_identity_set.contains(&identity) {
                    continue;
                }
                let reason_tags =
                    parse_discovery_reason_tags(row.reason_json.as_deref(), "external_match");
                let entry = candidate_inputs.entry(identity.clone()).or_insert_with(|| {
                    blend::BlendCandidateInput {
                        identity: identity.clone(),
                        title: row.title.clone(),
                        artist_name: row.artist_name.clone(),
                        album_title: None,
                        artwork_url: None,
                        duration_ms: row.duration_ms,
                        track_id: None,
                        tidal_id: row.tidal_id,
                        is_in_library: false,
                        confidence: 0.7,
                        source: "external".to_string(),
                        reason_tags: reason_tags.clone(),
                        per_seed_scores: Vec::new(),
                    }
                });
                entry.per_seed_scores.push((*seed_id, row.score));
            }
        }
    }

    let mut candidates = candidate_inputs
        .values()
        .map(|candidate| blend::score_blend_candidate(candidate, seeds))
        .collect::<Vec<_>>();
    let resolvable_external_count = candidates
        .iter()
        .filter(|candidate| {
            candidate.role == blend::CandidateRole::ExternalCandidate
                && candidate.playability == blend::Playability::Resolvable
        })
        .count();
    blend::apply_library_guide_cap(
        &mut candidates,
        library_cap_ratio,
        resolvable_external_count < 3,
    );
    for (index, candidate) in candidates.iter_mut().enumerate() {
        layout_blend_candidate(candidate, seed_count, index);
    }
    candidates.truncate(limit as usize);

    let playable_external_count = candidates
        .iter()
        .filter(|candidate| {
            candidate.role == blend::CandidateRole::ExternalCandidate
                && matches!(
                    candidate.playability,
                    blend::Playability::Playable | blend::Playability::Resolvable
                )
        })
        .count();
    let pending_external_count = candidates
        .iter()
        .filter(|candidate| {
            candidate.role == blend::CandidateRole::ExternalCandidate
                && candidate.playability == blend::Playability::Pending
        })
        .count();
    let library_guide_count = candidates
        .iter()
        .filter(|candidate| candidate.role == blend::CandidateRole::LibraryGuide)
        .count();
    let coverage_ratio = if candidates.is_empty() || seed_count == 0 {
        0.0
    } else {
        candidates
            .iter()
            .map(|candidate| candidate.covered_seed_count as f64 / seed_count as f64)
            .sum::<f64>()
            / candidates.len() as f64
    };
    let health = json!({
        "playable_external_count": playable_external_count,
        "pending_external_count": pending_external_count,
        "library_guide_count": library_guide_count,
        "coverage_ratio": coverage_ratio,
    });

    seed_nodes.append(&mut candidates);
    Ok((seed_nodes, health))
}

fn blend_node_json(candidate: &blend::ScoredBlendCandidate) -> Value {
    let track_id = candidate.track_id.unwrap_or_else(|| {
        candidate.tidal_id.unwrap_or_else(|| {
            synthetic_external_track_id(&candidate.artist_name, &candidate.title)
        })
    });
    let score = candidate.blend_score.clamp(0.0, 1.0);
    json!({
        "id": format!("track-{track_id}"),
        "track_id": track_id,
        "title": candidate.title,
        "artist_name": candidate.artist_name,
        "album_title": candidate.album_title,
        "artwork_url": candidate.artwork_url,
        "duration_ms": candidate.duration_ms,
        "similarity_score": score,
        "score": score,
        "raw_score": candidate.weighted_seed_proximity,
        "source": candidate.source,
        "is_in_library": candidate.is_in_library,
        "role": candidate.role,
        "playability": candidate.playability,
        "reason_tags": candidate.reason_tags,
        "primary_reason": candidate.reason_tags.first().cloned().unwrap_or_else(|| "blend".to_string()),
        "per_seed_scores": candidate.per_seed_scores,
        "coverage_bonus": candidate.coverage_bonus,
        "external_bonus": candidate.external_bonus,
        "library_penalty": candidate.library_penalty,
        "final_blend_score": candidate.blend_score,
        "is_seed": candidate.role == blend::CandidateRole::Seed,
        "x": candidate.x,
        "y": candidate.y,
        "vx": 0.0,
        "vy": 0.0,
        "radius": if candidate.role == blend::CandidateRole::Seed { 20.0 } else { 8.0 + score * 16.0 },
        "opacity": 0.0,
        "layout": {
            "x": candidate.x,
            "y": candidate.y,
            "radius_hint": if candidate.role == blend::CandidateRole::Seed { 20.0 } else { 8.0 + score * 16.0 },
            "distance_from_seed": (1.0 - score).clamp(0.0, 1.0),
        },
    })
}

fn blend_edge_json(
    seed: &blend::ScoredBlendCandidate,
    candidate: &blend::ScoredBlendCandidate,
) -> Vec<Value> {
    let from_track_id = seed.track_id.unwrap_or_else(|| {
        seed.tidal_id
            .unwrap_or_else(|| synthetic_external_track_id(&seed.artist_name, &seed.title))
    });
    let to_track_id = candidate.track_id.unwrap_or_else(|| {
        candidate.tidal_id.unwrap_or_else(|| {
            synthetic_external_track_id(&candidate.artist_name, &candidate.title)
        })
    });
    candidate
        .per_seed_scores
        .iter()
        .filter(|score| score.seed_identity == seed.identity && score.score > 0.0)
        .map(|score| {
            json!({
                "id": format!("blend-{from_track_id}-{to_track_id}"),
                "from_id": from_track_id,
                "to_id": to_track_id,
                "from_track_id": from_track_id,
                "to_track_id": to_track_id,
                "type": "blend",
                "reason": "blend",
                "primary_reason": "blend",
                "reason_tags": ["blend"],
                "weight": score.score,
                "confidence": 0.7,
                "source": "blend",
                "support_count": null,
                "behavioral_score": 0.0,
                "audio_score": score.score,
                "metadata_score": 0.0,
            })
        })
        .collect()
}

fn blend_candidates_to_radio(
    candidates: &[blend::ScoredBlendCandidate],
) -> Vec<crate::services::radio::RadioCandidate> {
    candidates
        .iter()
        .filter(|candidate| {
            candidate.role != blend::CandidateRole::Seed
                && matches!(
                    candidate.playability,
                    blend::Playability::Playable | blend::Playability::Resolvable
                )
        })
        .map(|candidate| crate::services::radio::RadioCandidate {
            track_id: candidate
                .track_id
                .or(candidate.tidal_id)
                .unwrap_or_default(),
            tidal_track_id: candidate.tidal_id,
            title: candidate.title.clone(),
            artist_name: candidate.artist_name.clone(),
            album_title: candidate.album_title.clone(),
            artwork_url: candidate.artwork_url.clone(),
            duration_ms: candidate.duration_ms,
            isrc: None,
            is_in_library: candidate.is_in_library,
            source: if candidate.is_in_library {
                crate::services::radio::RadioSource::Library
            } else {
                crate::services::radio::RadioSource::Engine
            },
            reason: "Blend discovery".to_string(),
            similarity_score: candidate.blend_score.clamp(0.0, 1.0),
            confidence: Some(candidate.blend_score.clamp(0.0, 1.0)),
            candidate_in_degree_percentile: None,
            support_count: Some(candidate.covered_seed_count as i64),
            primary_reason: Some(
                candidate
                    .reason_tags
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "blend".to_string()),
            ),
        })
        .collect()
}

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

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": e.to_string() })),
    )
}

async fn get_discovery_blend_space(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryBlendRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut seeds =
        blend::validate_and_normalize_seeds(&payload.seeds).map_err(blend_seed_error_response)?;
    let limit = payload.limit.unwrap_or(60).max(1).min(200);
    let db = {
        let state_guard = state.read().await;
        state_guard.db.clone()
    };
    let (candidates, health) = db
        .with_conn(|conn| build_discovery_blend_space(conn, &mut seeds, limit, 0.25))
        .map_err(internal)?;
    let seed_nodes = candidates
        .iter()
        .filter(|candidate| candidate.role == blend::CandidateRole::Seed)
        .collect::<Vec<_>>();
    let tracks = candidates.iter().map(blend_node_json).collect::<Vec<_>>();
    let mut edges = Vec::new();
    for seed in seed_nodes {
        for candidate in candidates
            .iter()
            .filter(|candidate| candidate.role != blend::CandidateRole::Seed)
        {
            edges.extend(blend_edge_json(seed, candidate));
        }
    }

    Ok(Json(json!({
        "tracks": tracks,
        "edges": edges,
        "artists": [],
        "blend_seeds": seeds,
        "health": health,
        "diagnostics": {
            "node_count": tracks.len(),
            "edge_count": edges.len(),
            "source_counts": {},
            "reason_counts": {},
        },
        "generated_at": chrono::Utc::now().to_rfc3339(),
    })))
}

async fn add_discovery_blend_to_queue(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryBlendRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut seeds =
        blend::validate_and_normalize_seeds(&payload.seeds).map_err(blend_seed_error_response)?;
    let limit = payload.limit.unwrap_or(60).max(1).min(200);
    let db = {
        let state_guard = state.read().await;
        state_guard.db.clone()
    };
    let (candidates, health) = db
        .with_conn(|conn| build_discovery_blend_space(conn, &mut seeds, limit, 0.15))
        .map_err(internal)?;
    let tracks = blend_candidates_to_radio(&candidates);
    if tracks.is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "blend has no playable discoveries" })),
        ));
    }
    let build = db
        .with_conn(move |conn| {
            Ok(crate::server::radio_pipeline::append_radio_queue_from_candidates(conn, tracks)?)
        })
        .map_err(internal)?;
    let pending_item_ids = build.pending_item_ids;
    let pending_count = pending_item_ids.len();
    spawn_pending_resolvers_for_queue_items(&state, &db, pending_item_ids, "blend_add").await;
    {
        let state_guard = state.read().await;
        let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
    }
    Ok(Json(json!({
        "first_playable": match build.first_item {
            Some((queue_item_id, Some(track_id))) => json!({
                "type": "library",
                "queue_item_id": queue_item_id,
                "track_id": track_id,
            }),
            Some((queue_item_id, None)) => json!({
                "type": "pending",
                "queue_item_id": queue_item_id,
                "track_id": null,
            }),
            None => json!(null),
        },
        "pending_count": pending_count,
        "health": health,
    })))
}

async fn play_discovery_blend(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryBlendRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    play_discovery_blend_inner(state, payload, "blend_play").await
}

async fn make_discovery_blend_radio(
    State(state): State<SharedState>,
    Json(mut payload): Json<DiscoveryBlendRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    payload.limit = Some(payload.limit.unwrap_or(200).max(120).min(200));
    play_discovery_blend_inner(state, payload, "blend_radio").await
}

async fn play_discovery_blend_inner(
    state: SharedState,
    payload: DiscoveryBlendRequest,
    context: &'static str,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut seeds =
        blend::validate_and_normalize_seeds(&payload.seeds).map_err(blend_seed_error_response)?;
    let limit = payload.limit.unwrap_or(60).max(1).min(200);
    let db = {
        let state_guard = state.read().await;
        state_guard.db.clone()
    };
    let (candidates, health) = db
        .with_conn(|conn| build_discovery_blend_space(conn, &mut seeds, limit, 0.15))
        .map_err(internal)?;
    let tracks = blend_candidates_to_radio(&candidates);
    if tracks.is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "blend has no playable discoveries" })),
        ));
    }
    let (first_playable, pending_count) =
        build_radio_queue_and_spawn_resolvers(&state, &db, None, tracks, context).await?;
    let snapshot = start_first_radio_queue_item(&state).await?;
    Ok(Json(json!({
        "first_playable": first_playable,
        "pending_count": pending_count,
        "health": health,
        "state": snapshot.state,
        "queue": snapshot.queue,
    })))
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

    // -- 1. Decide track set based on inputs ----------------------------------
    //
    //   prompt set   -> rank_candidates (text/genre/affinity scoring)
    //   seed_id set  -> radio_from_neighbors (embedding graph)
    //   neither      -> most-played fallback

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

    // -- 1b. Prepend the seed track itself when in seed mode (so canvas has center) --
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

    // -- 2. Fill remainder from most-played library tracks --------------------
    // Only fill when browsing without a seed. In seed mode the radio candidates
    // ARE the map - padding with unrelated most-played tracks creates a cloud of
    // disconnected blue dots with no edges and falsely-cold-start labels.
    if seed_id > 0 && prompt.is_empty() && (space_tracks.len() as i64) < limit {
        let remaining = limit - space_tracks.len() as i64;
        let external_rows = state_guard
            .db
            .with_conn(|conn| {
                let Some(model) = queries::get_selected_discovery_embedding_model(conn)? else {
                    return Ok(Vec::new());
                };
                queries::get_external_candidate_neighbors(conn, model.id, seed_id, remaining, true)
            })
            .unwrap_or_default();
        let mut present_ids = space_tracks
            .iter()
            .map(|track| track.track_id)
            .collect::<HashSet<_>>();
        for row in external_rows {
            let Some(tidal_id) = row.tidal_id.filter(|id| *id > 0) else {
                continue;
            };
            if !present_ids.insert(tidal_id) {
                continue;
            }
            let raw_tags = row
                .reason_json
                .as_deref()
                .and_then(|raw| serde_json::from_str::<Vec<Value>>(raw).ok())
                .unwrap_or_default()
                .iter()
                .filter_map(|value| {
                    value
                        .get("key")
                        .and_then(|key| key.as_str())
                        .or_else(|| value.get("label").and_then(|label| label.as_str()))
                        .map(str::to_string)
                })
                .collect::<Vec<_>>();
            let mut reason_tags = ds::normalize_reason_tags(&raw_tags);
            if reason_tags.is_empty() {
                reason_tags.push(ds::normalize_reason("external_match").to_string());
            }
            let primary_reason = reason_tags
                .first()
                .cloned()
                .unwrap_or_else(|| ds::normalize_reason("external_match").to_string());

            space_tracks.push(SpaceTrack {
                track_id: tidal_id,
                title: row.title,
                artist_name: row.artist_name,
                album_title: None,
                artwork_url: None,
                duration_ms: row.duration_ms,
                similarity_score: row.score.clamp(0.0, 1.0),
                source: "external".to_string(),
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
                is_in_library: false,
                radio_source: Some("engine".to_string()),
                radio_reason: Some("external_match".to_string()),
                confidence: 0.7,
                support_count: 1,
                primary_reason,
                reason_tags,
                genres: vec![],
                in_degree_pctile: 0.5,
            });
        }
    }

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

    // -- 3. Fetch DSP features for all collected track IDs --------------------
    if !space_tracks.is_empty() {
        let ids_csv: String = space_tracks
            .iter()
            .filter(|t| t.is_in_library)
            .map(|t| t.track_id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        if ids_csv.is_empty() {
            // No library tracks present (pure external response) - nothing to enrich.
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

    // -- 3b. Aggregate skip-rate + completion-avg from listen_history ---------
    if !space_tracks.is_empty() {
        let ids_csv: String = space_tracks
            .iter()
            .filter(|t| t.is_in_library)
            .map(|t| t.track_id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        if ids_csv.is_empty() {
            // No library tracks present (pure external response) - nothing to enrich.
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
                    // Preserve Option semantics - None means "no listen data" (distinct from 0.0).
                    t.skip_rate = *skip;
                    t.completion_avg = *comp;
                }
            }
        }
    }

    // -- 3c. Backfill last_played_at + play_count from tracks table -----------
    if !space_tracks.is_empty() {
        let ids_csv: String = space_tracks
            .iter()
            .filter(|t| t.is_in_library)
            .map(|t| t.track_id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        if ids_csv.is_empty() {
            // No library tracks present (pure external response) - nothing to enrich.
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

    // -- 3d. Top-genre with source + confidence (highest confidence per track) -
    if !space_tracks.is_empty() {
        let ids_csv: String = space_tracks
            .iter()
            .filter(|t| t.is_in_library)
            .map(|t| t.track_id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        if ids_csv.is_empty() {
            // No library tracks present (pure external response) - nothing to enrich.
        } else {
            // genre_map: track_id -> (top_name, top_source, top_conf, all_names)
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

    // -- 3e. Cohort assignment per track (90-day window) ----------------------
    if !space_tracks.is_empty() {
        let track_ids: Vec<i64> = space_tracks
            .iter()
            .filter(|t| t.is_in_library)
            .map(|t| t.track_id)
            .collect();

        if track_ids.is_empty() {
            // No library tracks - skip cohort assignment.
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

    // -- 4. Build typed edges (v1.5) ------------------------------------------
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

    // Library<->library edges come from `track_neighbors`. We always run this
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
    // a seed->external edge per external track. This runs alongside the library
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

    // -- 5. Score normalization (per source group) -----------------------------
    let score_candidates: Vec<ds::ScoreCandidate> = space_tracks
        .iter()
        .map(|t| ds::ScoreCandidate {
            track_id: t.track_id,
            raw_score: t.similarity_score,
            source: ds::normalize_source(&t.source).to_string(),
        })
        .collect();
    let norm_scores = ds::normalize_scores_by_source(&score_candidates);

    // -- 6. Within-set in-degree stats ----------------------------------------
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

    // -- 7. Graph pruning ------------------------------------------------------
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

    // -- 8. Serialize nodes with v1.5 fields ----------------------------------
    let total = space_tracks.len().max(1);
    let track_nodes: Vec<Value> = space_tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let norm_score = norm_scores
                .get(&t.track_id)
                .copied()
                .unwrap_or_else(|| t.similarity_score.clamp(0.0, 1.0));
            // Library tracks are only truly cold-start if confidence is very low -
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

    // -- 9. Serialize edges with v1.5 fields ----------------------------------
    let edge_nodes: Vec<Value> = typed_edges
        .iter()
        .filter(|e| {
            surviving_ids.contains(&e.from_track_id) && surviving_ids.contains(&e.to_track_id)
        })
        .map(|e| {
            let edge_id = format!("{}-{}-{}", e.from_track_id, e.to_track_id, e.primary_reason);
            json!({
                // -- Existing fields --
                "from_id": e.from_track_id,
                "to_id": e.to_track_id,
                "type": &e.primary_reason,
                "weight": e.weight,
                "reason_tags": &e.reason_tags,
                "behavioral_score": e.behavioral_score,
                "audio_score": e.audio_score,
                "metadata_score": e.metadata_score,
                // -- v1.5 fields --
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

    // -- 10. Diagnostics -------------------------------------------------------
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

    // -- 11. Background seed-neighbor refresh (DiscoverSpace only) ------------
    // Fire-and-forget: computes embedding similarity for this seed, writes to
    // track_neighbors, then sends DiscoverySpaceRefreshed so the map auto-reloads.
    // `refreshed_seeds` is a TTL'd map keyed by (seed_id -> model_id, instant) so
    // entries expire and re-training invalidates them automatically.
    if seed_id > 0 && prompt.is_empty() {
        let guard = state.read().await;
        // Best-effort: read current model_id outside the spawned task so we can
        // skip the spawn entirely when this seed is fresh under the same model.
        let active_model_id: Option<i64> = guard
            .db
            .with_conn(|conn| {
                Ok(crate::db::queries::get_selected_discovery_embedding_model(conn)?.map(|m| m.id))
            })
            .unwrap_or(None);
        let already_fresh = match active_model_id {
            Some(mid) => crate::services::neighbor_refresh::is_seed_fresh(
                &guard.refreshed_seeds,
                seed_id,
                mid,
            ),
            None => true, // no model -> nothing to do anyway
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

    spawn_pending_resolvers_for_queue_items(state, db, pending_item_ids, context).await;

    {
        let s = state.read().await;
        let _ = s.event_tx.send(AppEvent::QueueUpdated);
    }

    Ok((first_playable, pending_count))
}

async fn spawn_pending_resolvers_for_queue_items(
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
                // later sync that upserts the real album by tidal_id — the
                // result is two album rows for the same release and the
                // discovery-injected track hanging off the orphan stub.
                //
                // The fixed shape: look up by `tidal_id`; if missing, insert
                // with the tidal_id set so the next sync finds it. When the
                // candidate has no album tidal id (unknown source), leave
                // `album_id` NULL — the artwork falls back to track.artwork_url
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
                                    // inserted the same album — fetch its id instead
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
                        "INSERT INTO tracks (tidal_id, title, artist_id, album_id, duration_ms, best_quality, best_source, fidelity_score, is_favorite, source)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'tidal', ?7, 0, 'tidal')
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
        state_guard.pending_tidal_mix_queue.lock().unwrap().clear();
    }
}

pub(crate) fn active_ephemeral_tidal_mix_dj_pair(
    state: &crate::AppState,
) -> Option<crate::playback::dj_lookahead::DjLookaheadPair> {
    let current = state.ephemeral_tidal_track.as_ref()?;
    let pending = state
        .pending_tidal_mix_queue
        .lock()
        .unwrap()
        .iter()
        .cloned()
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
    let pending = state.pending_tidal_mix_queue.lock().unwrap();
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
    if let Some(pair) = active_ephemeral_tidal_mix_dj_pair(state)
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
    // now that has no DB record.
    let has_ephemeral_tidal_track = state_guard.ephemeral_tidal_track.is_some();
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
    let db = state_guard.db.clone();
    let pending = state_guard
        .pending_tidal_mix_queue
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    drop(state_guard);
    if !pending.is_empty() {
        if has_ephemeral_tidal_track {
            snapshot.queue.clear();
        }
        let tidal_ids: Vec<i64> = pending.iter().map(|p| p.tidal_track_id).collect();
        let library_states = db
            .with_conn(|conn| queries::get_tidal_track_library_states(conn, &tidal_ids))
            .unwrap_or_default();
        let start_position = snapshot
            .queue
            .iter()
            .map(|q| q.position)
            .max()
            .unwrap_or(-1)
            + 1;
        for (offset, p) in pending.into_iter().enumerate() {
            let library_state = library_states.get(&p.tidal_track_id).copied();
            let track = crate::db::models::Track {
                id: library_state
                    .map(|state| state.local_id)
                    .unwrap_or(-p.tidal_track_id),
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
                is_favorite: library_state
                    .map(|state| state.is_favorite)
                    .unwrap_or(false),
                play_count: 0,
                last_played_at: None,
                date_added: None,
                source: "tidal_ephemeral".to_string(),
                artwork_url: p.artwork_url,
            };
            // Queue ids stay negative for in-memory mix rows. Track ids are
            // local ids when the TIDAL id is already in the library.
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
    // include one. Independent of promotion success — the artist row now
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
) -> Option<crate::db::models::Track> {
    let db = {
        let s = state.read().await;
        s.db.clone()
    };

    let (queue_item_id, pending_artist, pending_title, tidal_id_hint) = db
        .with_conn(|conn| pending::current_pending(conn))
        .ok()
        .flatten()?;

    // Claim ownership; bail if another resolver already claimed this row.
    let claimed = db
        .with_conn(|conn| pending::try_claim(conn, queue_item_id))
        .unwrap_or(false);
    if !claimed {
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

    let artist_tidal_id = metadata.artist_tidal_id;
    let imported = crate::services::tidal::import::import_track_from_metadata(&db, metadata).await;

    let (local_id, artist_local_id) = match imported {
        Ok(imp) => (imp.local_id, imp.artist_id),
        Err(_) => {
            release_lock(&db, queue_item_id);
            return None;
        }
    };

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

fn automix_discover_new_fallback_seed(
    snapshot: &crate::playback::player::PlaybackSnapshot,
) -> Option<crate::db::models::Track> {
    if !snapshot.state.automix_discover_new {
        return None;
    }
    let current_pos = snapshot
        .state
        .current_queue_item_id
        .and_then(|qid| {
            snapshot
                .queue
                .iter()
                .find(|item| item.id == qid)
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

    // The pending TIDAL mix queue lives in-memory outside the DB queue, so
    // `apply_shuffle` above never sees it. Reorder it here so flipping shuffle
    // on during a TIDAL mix actually changes what plays next. Pending entries
    // carry no genre/artist_id metadata, so genre/weighted modes degrade to a
    // plain Fisher-Yates - same shape as `true` shuffle.
    if let Some(debug) = update.debug.as_ref() {
        let mut q = state_guard.pending_tidal_mix_queue.lock().unwrap();
        if q.len() > 1 {
            use rand::seq::SliceRandom;
            let mut rng = crate::playback::shuffle::seeded_rng(
                debug.seed,
                mode.as_str(),
                "pending_tidal_mix",
            );
            q.make_contiguous().shuffle(&mut rng);
        }
    }

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
    refresh_dj_after_queue_change(state, "queue_append_many").await;

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
    let response = {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(|conn| {
                queue::remove_queue_item(conn, payload.queue_item_id)?;
                let queue = queue::load_queue(conn)?;
                let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
                Ok(Json(json!({ "queue": queue })))
            })
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    refresh_dj_after_queue_change(state, "remove_queue_track").await;
    Ok(response)
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
                let queue = queue::load_queue(conn)?;
                let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
                Ok(Json(json!({ "queue": queue })))
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
            match (current_track_id, current_queue_item_id) {
                (Some(track_id), _) => {
                    // Library track playing - preserve by track_id.
                    conn.execute("DELETE FROM queue WHERE track_id != ?1", params![track_id])?;
                }
                (None, Some(qid)) => {
                    // Pending row playing - preserve by queue item id.
                    conn.execute("DELETE FROM queue WHERE id != ?1", params![qid])?;
                }
                (None, None) => {
                    queue::clear_queue(conn)?;
                }
            }
                state_guard.pending_tidal_mix_queue.lock().unwrap().clear();
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

fn pending_tidal_playlist_source(
    pending: crate::PendingEphemeralTidalTrack,
) -> PlaylistFromQueueSource {
    PlaylistFromQueueSource::Tidal(tidal_import::ImportTrackMetadata {
        tidal_id: pending.tidal_track_id,
        title: non_empty_or_default(Some(pending.title), "Unknown title"),
        artist_name: non_empty_or_default(pending.artist_name, "Unknown artist"),
        album_title: optional_non_empty(pending.album_title),
        album_artwork_url: pending.artwork_url,
        duration_ms: pending.duration_ms,
        ..Default::default()
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
    let (db, current_source, pending_tidal_sources) = {
        let state = state.read().await;
        let current = state
            .ephemeral_tidal_track
            .as_ref()
            .or(state.external_playback_track.as_ref())
            .and_then(|track| visible_track_playlist_source(track, include_tidal_only));
        let pending = if include_tidal_only {
            state
                .pending_tidal_mix_queue
                .lock()
                .unwrap()
                .iter()
                .cloned()
                .filter(|pending| pending.tidal_track_id > 0)
                .map(pending_tidal_playlist_source)
                .collect()
        } else {
            Vec::new()
        };
        (state.db.clone(), current, pending)
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
    sources.extend(pending_tidal_sources);

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

    if let Some(tokens) = tokens {
        if !tokens.is_pkce() {
            tidal_auth::warn_if_fallback_client_credentials();
        }
        Json(tidal_status_payload(
            Some(&tokens),
            tidal_auth::tidal_pkce_client_credential_source(),
            tidal_auth::tidal_client_credential_source(),
        ))
    } else {
        Json(tidal_status_payload(
            None,
            tidal_auth::tidal_pkce_client_credential_source(),
            tidal_auth::tidal_client_credential_source(),
        ))
    }
}

fn tidal_status_payload(
    tokens: Option<&tidal_auth::TidalTokens>,
    pkce_source: tidal_auth::TidalCredentialSource,
    legacy_source: tidal_auth::TidalCredentialSource,
) -> Value {
    let Some(tokens) = tokens else {
        return json!({ "connected": false });
    };
    let auth_flow = tokens.auth_flow.as_deref().unwrap_or("legacy");
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

    // Cache check - best-effort. A read failure must NOT block the upstream call.
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
        // Best-effort cache write - log and continue on failure.
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

    let mut artists: Vec<TidalSearchArtistResp> = results
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

    // TIDAL's catalog-search response often omits artist `picture`, so most
    // search-result artists land here with `artwork_url = None` and render as
    // initials in the UI. The /artists/{id} endpoint carries the canonical
    // picture; fan out a parallel fetch for any artist still missing artwork
    // and backfill. Tight cap (12) to bound the fan-out -- artists past that
    // tend to be long-tail rows the user is unlikely to scroll to anyway.
    const ARTIST_PHOTO_BACKFILL_CAP: usize = 12;
    let backfill_targets: Vec<(usize, i64)> = artists
        .iter()
        .enumerate()
        .filter(|(_, a)| a.artwork_url.is_none())
        .take(ARTIST_PHOTO_BACKFILL_CAP)
        .map(|(i, a)| (i, a.tidal_id))
        .collect();
    if !backfill_targets.is_empty() {
        let backfill_client = client.clone();
        let fetches = backfill_targets.iter().map(|(idx, tidal_id)| {
            let c = backfill_client.clone();
            let id = *tidal_id;
            let i = *idx;
            async move {
                let url = c
                    .get_artist(id)
                    .await
                    .ok()
                    .and_then(|a| TidalClient::get_artwork_url(&a.picture, 640));
                (i, url)
            }
        });
        let results: Vec<(usize, Option<String>)> = futures::future::join_all(fetches).await;
        for (i, url) in results {
            if let Some(u) = url {
                if let Some(slot) = artists.get_mut(i) {
                    slot.artwork_url = Some(u);
                }
            }
        }
    }

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
    let cache_key = tidal_playlist_tracks_cache_key(&tokens.country_code, &uuid, limit, offset);
    let client = TidalClient::with_http(
        tidal_http_client.clone(),
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let tracks = match get_cached_tidal_playlist_tracks(&playlist_tracks_cache, &cache_key) {
        Some(cached) => cached,
        None => {
            let resp = match client.get_playlist_tracks(&uuid, limit, offset).await {
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
                        .get_playlist_tracks(&uuid, limit, offset)
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
}

async fn play_tidal_ephemeral(
    State(state): State<SharedState>,
    Json(body): Json<PlayTidalRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    // If the picked track is sitting in the pending mix queue, jump to it.
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
    let shuffle_debug = shuffle_tidal_mix_tracks(&mut tracks, body.shuffle_mode.as_deref());

    let mut iter = tracks
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
    let first_tidal_id = first.tidal_track_id;
    let rest: Vec<crate::PendingEphemeralTidalTrack> = iter.collect();

    // Replace any existing pending queue with this mix's continuation.
    {
        let s = state.read().await;
        let mut q = s.pending_tidal_mix_queue.lock().unwrap();
        q.clear();
        q.extend(rest);
    }

    start_ephemeral_tidal_playback(&state, first).await?;
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

    // Backstop for callers that don't ship artwork (Spotify-resolved playlist
    // tracks below the fold never trigger the lazy IntersectionObserver, so
    // they arrive with `artwork_url: null`). Look up the TIDAL track once and
    // reuse its album cover at the standard 640x640 size.
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
    // route: top-tracks/albums already loaded successfully above.
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

pub(super) async fn recover_tidal_session(
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
        let pending_tracks = state_guard
            .pending_tidal_mix_queue
            .lock()
            .unwrap()
            .iter()
            .cloned()
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
        let pending_front = state_guard
            .pending_tidal_mix_queue
            .lock()
            .unwrap()
            .front()
            .map(|track| track.tidal_track_id);
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
        let pending_front = state_guard
            .pending_tidal_mix_queue
            .lock()
            .unwrap()
            .front()
            .map(|track| track.tidal_track_id);
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
        let mut pending = state_guard.pending_tidal_mix_queue.lock().unwrap();
        if pending.front().map(|track| track.tidal_track_id) != Some(prepared.tidal_track_id) {
            return Ok(true);
        }
        let _ = pending.pop_front();
        drop(pending);

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
    {
        let mut state_guard = state.write().await;
        if let Some(info) = state_guard.playback_runtime_info.as_mut() {
            info.active_track_id = snapshot.state.current_track.as_ref().map(|track| track.id);
        }
    }

    if let Some(track) = snapshot.state.current_track.as_ref() {
        let user_quality = current_user_audio_quality(state).await;
        let runtime_handle = ensure_playback_runtime_for_track(state, track)
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
                effective_crossfade_ms(state, snapshot.state.crossfade_ms).await,
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
            let stream_info = resolve_tidal_playback_stream(state, track, &stream_request)
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
                effective_crossfade_ms(state, snapshot.state.crossfade_ms).await,
                user_quality,
            )
            .with_generation(generation);
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
                        "TIDAL 429 advancing mix to '{}': backing off 3s and retrying once",
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
                            "Failed to advance to '{}' ({status}, fail {consecutive_failures}/{MAX_CONSEC_FAILURES}): {}. Skipping",
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
        let persisted_snapshot = {
            let state_guard = state.read().await;
            let cleared = recently_cleared(&state_guard);
            state_guard
                .db
                .with_conn(|conn| player::next_track(conn, cleared))?
        };
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
            conn.execute(
                "INSERT INTO service_auth (service, access_token_enc, user_id, connected_at)
                 VALUES ('tidal', ?1, ?2, datetime('now'))
                 ON CONFLICT(service) DO UPDATE SET access_token_enc=excluded.access_token_enc,
                 user_id=excluded.user_id, connected_at=excluded.connected_at",
                rusqlite::params![token_blob, tokens.user_id],
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
        "INSERT INTO tracks (tidal_id, title, artist_id, album_id, disc_number, track_number, duration_ms, isrc, best_quality, best_source, fidelity_score, is_favorite, source, date_added)
         VALUES (?1, ?2, (SELECT id FROM artists WHERE tidal_id=?3), (SELECT id FROM albums WHERE tidal_id=?4), ?5, ?6, ?7, ?8, ?9, 'tidal', ?10, ?11, 'tidal', COALESCE(?12, datetime('now')))
         ON CONFLICT(tidal_id) DO UPDATE SET
            title=excluded.title, best_quality=excluded.best_quality,
            fidelity_score=MAX(tracks.fidelity_score, excluded.fidelity_score),
            is_favorite=MAX(tracks.is_favorite, excluded.is_favorite),
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

// --- TIDAL: Your Mixes -------------------------------------------------------

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

async fn get_home_recommendations(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let lastfm = load_or_fetch_recommendation_shelf(state.clone(), "lastfm").await;
    let listenbrainz = load_or_fetch_recommendation_shelf(state.clone(), "listenbrainz").await;
    Ok(Json(json!({
        "shelves": [
            recommendation_shelf_json("lastfm", "Last.fm profile picks", lastfm),
            recommendation_shelf_json("listenbrainz", "ListenBrainz recommends", listenbrainz),
        ]
    })))
}

fn recommendation_shelf_json(
    provider: &str,
    title: &str,
    result: anyhow::Result<Vec<Value>>,
) -> Value {
    match result {
        Ok(items) => json!({
            "provider": provider,
            "title": title,
            "status": if items.is_empty() { "empty" } else { "ok" },
            "items": items,
        }),
        Err(error) => json!({
            "provider": provider,
            "title": title,
            "status": "error",
            "message": error.to_string(),
            "items": [],
        }),
    }
}

async fn load_or_fetch_recommendation_shelf(
    state: SharedState,
    provider: &str,
) -> anyhow::Result<Vec<Value>> {
    if let Some(cached) = read_recommendation_cache(&state, provider).await {
        return Ok(cached);
    }
    let items = match provider {
        "lastfm" => fetch_lastfm_home_recommendations(&state).await?,
        "listenbrainz" => fetch_listenbrainz_home_recommendations(&state).await?,
        _ => Vec::new(),
    };
    write_recommendation_cache(&state, provider, &items).await;
    Ok(items)
}

async fn read_recommendation_cache(state: &SharedState, provider: &str) -> Option<Vec<Value>> {
    let now = unix_now_secs();
    let s = state.read().await;
    s.db.with_conn(|conn| {
        conn.query_row(
            "SELECT payload_json FROM provider_recommendation_cache
                  WHERE provider = ?1 AND cache_key = 'home' AND expires_at > ?2",
            params![provider, now],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(Into::into)
    })
    .ok()
    .flatten()
    .and_then(|raw| serde_json::from_str(&raw).ok())
}

async fn write_recommendation_cache(state: &SharedState, provider: &str, items: &[Value]) {
    let now = unix_now_secs();
    let expires = now + 60 * 60;
    let Ok(payload) = serde_json::to_string(items) else {
        return;
    };
    let s = state.read().await;
    let _ = s.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO provider_recommendation_cache (provider, cache_key, payload_json, fetched_at, expires_at)
             VALUES (?1, 'home', ?2, ?3, ?4)
             ON CONFLICT(provider, cache_key) DO UPDATE SET
                 payload_json = excluded.payload_json,
                 fetched_at = excluded.fetched_at,
                 expires_at = excluded.expires_at",
            params![provider, payload, now, expires],
        )?;
        Ok::<_, anyhow::Error>(())
    });
}

async fn fetch_lastfm_home_recommendations(state: &SharedState) -> anyhow::Result<Vec<Value>> {
    let (http, db, user) = {
        let s = state.read().await;
        let user = s.db.with_conn(|conn| {
            Ok::<_, anyhow::Error>(
                crate::services::lastfm::auth::load_credentials(conn)?.and_then(|c| c.session_user),
            )
        })?;
        (s.http_client.clone(), s.db.clone(), user)
    };
    let Some(user) = user else {
        return Ok(Vec::new());
    };
    let Some(client) = LastFmClient::load(http, &db) else {
        return Ok(Vec::new());
    };
    let seeds = client.user_profile_seed_tracks(&user, 4).await?;
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for seed in seeds {
        for similar in client
            .track_get_similar(&seed.artist, &seed.title, 8)
            .await
            .unwrap_or_default()
        {
            let key = crate::services::radio::normalize_for_dedup(&similar.artist, &similar.title);
            if key.is_empty() || !seen.insert(key) {
                continue;
            }
            if let Some(item) = resolve_recommendation_item(
                state,
                "lastfm",
                &similar.artist,
                &similar.title,
                None,
                Some(similar.match_score),
                "Profile similar track",
            )
            .await
            {
                out.push(item);
            }
            if out.len() >= 12 {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

async fn fetch_listenbrainz_home_recommendations(
    state: &SharedState,
) -> anyhow::Result<Vec<Value>> {
    let (http, token, user) = {
        let s = state.read().await;
        let (token, user) = s.db.with_conn(|conn| {
            let token = crate::services::listenbrainz::load_token(conn, &s.master_key)?;
            let user =
                crate::services::listenbrainz::load_credentials(conn)?.and_then(|c| c.user_name);
            Ok::<_, anyhow::Error>((token, user))
        })?;
        (s.http_client.clone(), token, user)
    };
    let Some(user) = user else {
        return Ok(Vec::new());
    };
    let recs =
        crate::services::listenbrainz::user_recommendations(&http, &user, token.as_deref()).await?;
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for rec in recs {
        let key = crate::services::radio::normalize_for_dedup(&rec.artist, &rec.title);
        if key.is_empty() || !seen.insert(key) {
            continue;
        }
        if let Some(item) = resolve_recommendation_item(
            state,
            "listenbrainz",
            &rec.artist,
            &rec.title,
            rec.mbid.as_deref(),
            rec.score,
            "Collaborative filtering",
        )
        .await
        {
            out.push(item);
        }
        if out.len() >= 12 {
            break;
        }
    }
    Ok(out)
}

async fn resolve_recommendation_item(
    state: &SharedState,
    provider: &str,
    artist: &str,
    title: &str,
    mbid: Option<&str>,
    score: Option<f64>,
    reason: &str,
) -> Option<Value> {
    let s = state.read().await;
    s.db
        .with_conn(|conn| {
            if let Some(mbid_value) = mbid.filter(|v| !v.trim().is_empty()) {
                let by_mbid = conn
                    .query_row(
                        "SELECT t.id, t.tidal_id, t.title, a.name, al.title, t.artwork_url
                           FROM external_track_candidates c
                           JOIN tracks t
                             ON t.id = c.resolved_track_id
                             OR (c.tidal_id IS NOT NULL AND t.tidal_id = c.tidal_id)
                           LEFT JOIN artists a ON a.id = t.artist_id
                           LEFT JOIN albums al ON al.id = t.album_id
                          WHERE c.mbid = ?1
                          ORDER BY (c.resolved_track_id IS NULL), t.is_favorite DESC, t.play_count DESC
                          LIMIT 1",
                        params![mbid_value],
                        |row| {
                            Ok(json!({
                                "provider": provider,
                                "local_track_id": row.get::<_, i64>(0)?,
                                "tidal_id": row.get::<_, Option<i64>>(1)?,
                                "title": row.get::<_, String>(2)?,
                                "artist_name": row.get::<_, Option<String>>(3)?,
                                "album_title": row.get::<_, Option<String>>(4)?,
                                "artwork_url": row.get::<_, Option<String>>(5)?,
                                "mbid": mbid,
                                "score": score,
                                "reason": reason,
                                "playable": true,
                            }))
                        },
                    )
                    .optional()?;
                if by_mbid.is_some() {
                    return Ok::<_, anyhow::Error>(by_mbid);
                }
            }

            conn.query_row(
                "SELECT t.id, t.tidal_id, t.title, a.name, al.title, t.artwork_url
                   FROM tracks t
                   LEFT JOIN artists a ON a.id = t.artist_id
                   LEFT JOIN albums al ON al.id = t.album_id
                  WHERE LOWER(t.title) = LOWER(?1)
                    AND LOWER(COALESCE(a.name, '')) = LOWER(?2)
                  ORDER BY t.is_favorite DESC, t.play_count DESC
                  LIMIT 1",
                params![title, artist],
                |row| {
                    Ok(json!({
                        "provider": provider,
                        "local_track_id": row.get::<_, i64>(0)?,
                        "tidal_id": row.get::<_, Option<i64>>(1)?,
                        "title": row.get::<_, String>(2)?,
                        "artist_name": row.get::<_, Option<String>>(3)?,
                        "album_title": row.get::<_, Option<String>>(4)?,
                        "artwork_url": row.get::<_, Option<String>>(5)?,
                        "mbid": mbid,
                        "score": score,
                        "reason": reason,
                        "playable": true,
                    }))
                },
            )
            .optional()
            .map_err(Into::into)
        })
        .ok()
        .flatten()
}

fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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

// -- Spotify Config & Enrichment ----------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, schema};
    use axum::{body::Body, http::Request};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_track(id: i64, title: &str) -> crate::db::models::Track {
        crate::db::models::Track {
            id,
            title: title.to_string(),
            artist_id: 1,
            artist_name: Some("Artist".to_string()),
            album_id: None,
            album_title: None,
            disc_number: None,
            track_number: None,
            duration_ms: Some(180_000),
            isrc: None,
            tidal_id: Some(id),
            ytmusic_id: None,
            soundcloud_id: None,
            best_quality: Some("LOSSLESS".to_string()),
            best_source: Some("tidal".to_string()),
            fidelity_score: 0,
            is_favorite: false,
            play_count: 0,
            last_played_at: None,
            date_added: None,
            source: "tidal".to_string(),
            artwork_url: None,
        }
    }

    fn test_queue_item(
        id: i64,
        track: crate::db::models::Track,
        position: i32,
        source: &str,
    ) -> crate::db::models::QueueItem {
        crate::db::models::QueueItem {
            id,
            track,
            position,
            source: source.to_string(),
            reason: None,
            is_pending: source == "automix-new",
        }
    }

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

    #[tokio::test]
    async fn pause_and_resume_playback_invalidate_inflight_generation() {
        let (db, db_path) = fresh_migrated_db();
        let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db)));
        let app = api_routes(state.clone());

        let initial = {
            let guard = state.read().await;
            current_playback_generation(&guard)
        };

        let pause_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/playback/pause")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(pause_response.status(), StatusCode::OK);
        let after_pause = {
            let guard = state.read().await;
            current_playback_generation(&guard)
        };
        assert!(after_pause > initial);

        let resume_response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/playback/resume")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resume_response.status(), StatusCode::OK);
        let after_resume = {
            let guard = state.read().await;
            current_playback_generation(&guard)
        };
        assert!(after_resume > after_pause);

        let _ = std::fs::remove_file(db_path);
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
    fn tidal_status_payload_reports_pkce_source_only_for_pkce_tokens() {
        let tokens = test_tidal_tokens(Some("pkce"));

        let body = tidal_status_payload(
            Some(&tokens),
            tidal_auth::TidalCredentialSource::Env,
            tidal_auth::TidalCredentialSource::Fallback,
        );

        assert_eq!(body["connected"], true);
        assert_eq!(body["auth_flow"], "pkce");
        assert_eq!(body["pkce_client_credential_source"], "env");
        assert!(body.get("legacy_client_credential_source").is_none());
    }

    #[test]
    fn tidal_status_payload_reports_legacy_source_only_for_legacy_tokens() {
        let tokens = test_tidal_tokens(None);

        let body = tidal_status_payload(
            Some(&tokens),
            tidal_auth::TidalCredentialSource::Env,
            tidal_auth::TidalCredentialSource::Fallback,
        );

        assert_eq!(body["connected"], true);
        assert_eq!(body["auth_flow"], "legacy");
        assert_eq!(body["legacy_client_credential_source"], "fallback");
        assert!(body.get("pkce_client_credential_source").is_none());
    }

    #[test]
    fn tidal_status_payload_disconnected_omits_credential_sources() {
        let body = tidal_status_payload(
            None,
            tidal_auth::TidalCredentialSource::Env,
            tidal_auth::TidalCredentialSource::Fallback,
        );

        assert_eq!(body["connected"], false);
        assert!(body.get("auth_flow").is_none());
        assert!(body.get("pkce_client_credential_source").is_none());
        assert!(body.get("legacy_client_credential_source").is_none());
    }

    #[test]
    fn tidal_playlist_tracks_cache_returns_fresh_entries_and_expires_stale_entries() {
        let cache = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let key = tidal_playlist_tracks_cache_key("AU", "playlist-uuid", 100, 0);
        let tracks: Vec<TidalTrack> = Vec::new();

        put_cached_tidal_playlist_tracks(&cache, key.clone(), tracks.clone());

        assert!(get_cached_tidal_playlist_tracks(&cache, &key).is_some());

        {
            let mut guard = cache.lock().unwrap();
            guard.insert(
                key.clone(),
                (
                    Instant::now() - TIDAL_PLAYLIST_TRACKS_CACHE_TTL - Duration::from_secs(1),
                    tracks,
                ),
            );
        }

        assert!(get_cached_tidal_playlist_tracks(&cache, &key).is_none());
        assert!(!cache.lock().unwrap().contains_key(&key));
    }

    #[test]
    fn ephemeral_stream_request_uses_user_audio_quality() {
        let request = build_ephemeral_tidal_stream_request(
            123,
            Some(crate::db::audio_settings::AudioQuality::HiResLossless),
        );

        assert_eq!(request.track_id, 123);
        assert_eq!(request.audio_quality, "HI_RES_LOSSLESS");
        assert_eq!(request.playback_mode, "STREAM");
        assert_eq!(request.asset_presentation, "FULL");
    }

    #[test]
    fn ephemeral_stream_request_defaults_to_lossless_without_user_quality() {
        let request = build_ephemeral_tidal_stream_request(123, None);

        assert_eq!(request.audio_quality, tidal_stream::DEFAULT_AUDIO_QUALITY);
    }

    #[test]
    fn requested_tidal_quality_prefers_user_setting_over_payload_quality() {
        let quality = requested_tidal_quality(
            Some(crate::db::audio_settings::AudioQuality::Lossless),
            Some("HI_RES_LOSSLESS"),
        );

        assert_eq!(quality, "LOSSLESS");
    }

    #[test]
    fn requested_tidal_quality_uses_payload_quality_without_user_setting() {
        let quality = requested_tidal_quality(None, Some("HI_RES_LOSSLESS"));

        assert_eq!(quality, "HI_RES_LOSSLESS");
    }

    #[test]
    fn ephemeral_synthetic_track_keeps_resolved_stream_quality() {
        let track = crate::PendingEphemeralTidalTrack {
            tidal_track_id: 456,
            title: "Resolved Track".to_string(),
            artist_name: Some("Artist".to_string()),
            album_title: Some("Album".to_string()),
            artwork_url: None,
            duration_ms: Some(180_000),
        };
        let stream = tidal_stream::StreamInfo {
            url: "https://cdn.example.test/audio.flac".to_string(),
            segment_urls: vec![],
            segment_offsets_ms: vec![],
            track_id: 456,
            audio_quality: "HI_RES_LOSSLESS".to_string(),
            codec: "audio/flac".to_string(),
            sample_rate: Some(96_000),
            bit_depth: Some(24),
        };

        let synthetic = build_ephemeral_synthetic_track(&track, &stream, None);

        assert_eq!(synthetic.id, -456);
        assert_eq!(synthetic.tidal_id, Some(456));
        assert_eq!(synthetic.best_quality.as_deref(), Some("HI_RES_LOSSLESS"));
        assert_eq!(synthetic.source, "tidal_ephemeral");
    }

    fn test_tidal_tokens(auth_flow: Option<&str>) -> tidal_auth::TidalTokens {
        tidal_auth::TidalTokens {
            access_token: "access-secret".to_string(),
            refresh_token: "refresh-secret".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 86_400,
            user_id: "u-1".to_string(),
            country_code: "AU".to_string(),
            auth_flow: auth_flow.map(str::to_string),
        }
    }

    fn test_tidal_track(id: i64, title: &str) -> crate::services::tidal::client::TidalTrack {
        crate::services::tidal::client::TidalTrack {
            id,
            title: title.to_string(),
            duration: 180,
            track_number: Some(1),
            volume_number: Some(1),
            isrc: None,
            artist: crate::services::tidal::client::TidalArtist {
                id: 10,
                name: "Artist".to_string(),
                picture: None,
                extra: HashMap::new(),
            },
            artists: None,
            album: None,
            audio_quality: Some("LOSSLESS".to_string()),
            stream_ready: Some(true),
            extra: HashMap::new(),
        }
    }

    #[test]
    fn insert_tidal_track_uses_favorite_created_as_date_added() {
        let (db, db_path) = fresh_migrated_db();
        db.with_conn(|conn| {
            let track = test_tidal_track(2001, "Newest favorite");

            insert_tidal_track(conn, &track, true, Some("2026-05-01T12:34:56.000Z"))?;

            let date_added: String = conn.query_row(
                "SELECT date_added FROM tracks WHERE tidal_id = 2001",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(date_added, "2026-05-01T12:34:56.000Z");
            Ok(())
        })
        .expect("inserted favorite track");
        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn runtime_output_settings_preserve_persisted_exclusive_preferences() {
        let mut settings = crate::db::audio_settings::AudioSettings::default();
        settings.output_device = Some("Zen DAC V2".to_string());
        settings.exclusive_mode = true;
        settings.sample_rate_follow = true;
        settings.exclusive_release_grace_secs = 12;
        settings.exclusive_latency_mode =
            crate::db::audio_settings::ExclusiveLatencyMode::LowLatency;

        let output = runtime_output_settings_from_audio_settings(&settings);

        match output.device {
            playback_runtime::OutputDeviceSelection::Named(name) => {
                assert_eq!(name, "Zen DAC V2");
            }
            playback_runtime::OutputDeviceSelection::Default => {
                panic!("expected named output device")
            }
        }
        assert!(output.exclusive_mode);
        assert!(output.sample_rate_follow);
        assert_eq!(output.exclusive_release_grace_secs, 12);
        assert_eq!(
            output.exclusive_latency_mode,
            crate::db::audio_settings::ExclusiveLatencyMode::LowLatency
        );
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

        let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
            db.clone(),
        ))));

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
    /// initializers - when `crate::AppState` gains a field, add it here once.
    fn fresh_test_state(db: Database) -> crate::AppState {
        let (event_tx, _) = tokio::sync::broadcast::channel(16);
        #[cfg(feature = "spotify-public")]
        let spotify_public = Arc::new(
            crate::services::spotify_public::SpotifyPublicClient::new(db.clone())
                .expect("SpotifyPublicClient::new must succeed in tests"),
        );
        crate::AppState {
            db,
            event_tx,
            http_client: reqwest::Client::new(),
            tidal_http_client: reqwest::Client::new(),
            tidal_tokens: None,
            tidal_mixes_cache: Arc::new(std::sync::Mutex::new(None)),
            tidal_radio_stations_cache: Arc::new(std::sync::Mutex::new(None)),
            tidal_moods_cache: Arc::new(std::sync::Mutex::new(None)),
            tidal_page_modules_cache: Arc::new(std::sync::Mutex::new(HashMap::new())),
            tidal_playlist_tracks_cache: Arc::new(std::sync::Mutex::new(HashMap::new())),
            spotify_tokens: None,
            playback_runtime: None,
            playback_runtime_info: None,
            playback_generation: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            current_stream_display: None,
            pending_stream_display: None,
            next_prebuffer_inflight: None,
            last_drop_preview: None,
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
            dj_analysis_tx: None,
            dj_profile_rebuild_inflight: Arc::new(std::sync::Mutex::new(HashMap::new())),
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
            radio_similarity_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            refreshed_seeds: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            embedding_cache: Arc::new(std::sync::Mutex::new(None)),
            master_key: crate::services::crypto::MasterKey::load_or_generate(
                &std::env::temp_dir().join(format!("noor-test-key-{}", uuid::Uuid::new_v4())),
            )
            .expect("test master key"),
            pending_tidal_mix_queue: Arc::new(std::sync::Mutex::new(
                std::collections::VecDeque::new(),
            )),
            prepared_ephemeral_tidal_next: None,
            lastfm_api_secret: None,
            server_token: String::new(),
            audio_active: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            user_cleared_at: Arc::new(std::sync::atomic::AtomicI64::new(0)),
            #[cfg(feature = "spotify-public")]
            spotify_public,
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

    fn app_for_db(db: Database) -> Router {
        api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(db))))
    }

    fn seed_dj_queue_pair(db: &Database) -> (i64, i64) {
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (7001, 'DJ Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id)
                 VALUES
                    (7001, 'Outgoing', 7001, 180000, 77001),
                    (7002, 'Incoming', 7001, 180000, 77002)",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (7001, 0, 'test')",
                [],
            )?;
            let current_queue_item_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (7002, 1, 'test')",
                [],
            )?;
            let next_queue_item_id = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = 7001, current_queue_item_id = ?1, is_playing = 1
                 WHERE id = 1",
                params![current_queue_item_id],
            )?;
            Ok((current_queue_item_id, next_queue_item_id))
        })
        .expect("seed dj queue pair")
    }

    async fn json_request(
        app: Router,
        method: &str,
        uri: &str,
        body: &str,
    ) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response")
    }

    async fn response_json(response: axum::response::Response) -> Value {
        serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body bytes"),
        )
        .expect("json body")
    }

    #[tokio::test]
    async fn dj_enabled_defaults_false() {
        let (db, db_path) = fresh_migrated_db();
        let response = json_request(app_for_db(db), "GET", "/api/dj/enabled", "").await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["enabled"], false);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn dj_enabled_can_be_toggled() {
        let (db, db_path) = fresh_migrated_db();
        let app = app_for_db(db);
        let response =
            json_request(app.clone(), "PUT", "/api/dj/enabled", r#"{"enabled":true}"#).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["enabled"], true);

        let response = json_request(app, "PUT", "/api/dj/enabled", r#"{"enabled":false}"#).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["enabled"], false);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn chart_snapshots_return_latest_ranked_entries_without_zero_tidal_id() {
        let (db, db_path) = fresh_migrated_db();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO chart_snapshots
                    (id, source_key, region, period, chart_date, fetched_at, status)
                 VALUES
                    (1, 'spotify_daily', 'AU', 'daily', '2026-05-27', 100, 'ok'),
                    (2, 'spotify_daily', 'AU', 'daily', '2026-05-28', 200, 'ok')",
                [],
            )?;
            conn.execute(
                "INSERT INTO chart_entries
                    (id, snapshot_id, rank, rank_delta, artist, title, entity_type, streams)
                 VALUES
                    (10, 1, 1, 0, 'Old Artist', 'Old Track', 'track', 10),
                    (20, 2, 2, -1, 'Second Artist', 'Second Track', 'track', 200),
                    (21, 2, 1, 1, 'Top Artist', 'Top Track', 'track', 300)",
                [],
            )?;
            conn.execute(
                "INSERT INTO chart_entry_resolutions
                    (entry_id, status, tidal_id)
                 VALUES
                    (20, 'unresolved', NULL),
                    (21, 'tidal', 4242)",
                [],
            )?;
            Ok(())
        })
        .expect("seed chart snapshots");

        let response = json_request(
            app_for_db(db),
            "GET",
            "/api/charts/snapshots?source=spotify_daily&period=daily&region=AU&limit=2",
            "",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;

        assert_eq!(body["snapshot"]["chart_date"], "2026-05-28");
        assert_eq!(body["entries"].as_array().unwrap().len(), 2);
        assert_eq!(body["entries"][0]["rank"], 1);
        assert_eq!(body["entries"][0]["title"], "Top Track");
        assert_eq!(body["entries"][0]["resolution_status"], "tidal");
        assert_eq!(body["entries"][0]["tidal_id"], 4242);
        assert_eq!(body["entries"][1]["rank"], 2);
        assert_eq!(body["entries"][1]["title"], "Second Track");
        assert_eq!(body["entries"][1]["resolution_status"], "unresolved");
        assert!(body["entries"][1]["tidal_id"].is_null());

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn chart_matrix_returns_provider_cells_and_explicit_missing_cells() {
        let (db, db_path) = fresh_migrated_db();
        db.with_conn(|conn| {
            queries::upsert_chart_snapshot(
                conn,
                &queries::ChartSnapshotSeed {
                    source_key: "spotify_daily",
                    region: "global",
                    period: "daily",
                    chart_date: "2026-05-27",
                    fetched_at: 100,
                    etag: None,
                    content_hash: None,
                    status: "ok",
                },
                &[queries::ChartEntrySeed {
                    streams: Some(10),
                    ..queries::ChartEntrySeed::track(1, "Old Artist", "Old Song")
                }],
            )?;
            queries::upsert_chart_snapshot(
                conn,
                &queries::ChartSnapshotSeed {
                    source_key: "spotify_daily",
                    region: "global",
                    period: "daily",
                    chart_date: "2026-05-28",
                    fetched_at: 200,
                    etag: None,
                    content_hash: None,
                    status: "ok",
                },
                &[
                    queries::ChartEntrySeed {
                        streams: Some(500),
                        tidal_id: Some(5001),
                        resolution_status: Some("tidal"),
                        ..queries::ChartEntrySeed::track(1, "Top Artist", "Top Song")
                    },
                    queries::ChartEntrySeed {
                        streams: Some(400),
                        ..queries::ChartEntrySeed::track(2, "Ignored Artist", "Ignored Song")
                    },
                ],
            )?;
            queries::upsert_chart_snapshot(
                conn,
                &queries::ChartSnapshotSeed {
                    source_key: "spotify_daily",
                    region: "global",
                    period: "daily",
                    chart_date: "2026-05-28",
                    fetched_at: 205,
                    etag: None,
                    content_hash: Some("same-day-refresh"),
                    status: "ok",
                },
                &[queries::ChartEntrySeed {
                    streams: Some(550),
                    tidal_id: Some(5001),
                    resolution_status: Some("tidal"),
                    ..queries::ChartEntrySeed::track(1, "Top Artist", "Top Song")
                }],
            )?;
            queries::upsert_chart_snapshot(
                conn,
                &queries::ChartSnapshotSeed {
                    source_key: "youtube_daily",
                    region: "global",
                    period: "daily",
                    chart_date: "2026-05-28",
                    fetched_at: 210,
                    etag: None,
                    content_hash: None,
                    status: "ok",
                },
                &[queries::ChartEntrySeed {
                    entity_type: "video",
                    views: Some(900),
                    resolution_status: Some("not_playable"),
                    ..queries::ChartEntrySeed::track(1, "Video Artist", "Top Video")
                }],
            )?;
            queries::upsert_chart_snapshot(
                conn,
                &queries::ChartSnapshotSeed {
                    source_key: "shazam_daily",
                    region: "AU",
                    period: "daily",
                    chart_date: "2026-05-28",
                    fetched_at: 220,
                    etag: None,
                    content_hash: None,
                    status: "ok",
                },
                &[queries::ChartEntrySeed::track(1, "AU Artist", "AU Shazam")],
            )?;
            Ok(())
        })
        .expect("seed chart matrix");

        let response = json_request(app_for_db(db), "GET", "/api/charts/matrix", "").await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;

        let providers = body["providers"].as_array().expect("providers");
        assert_eq!(providers.len(), 6);
        assert_eq!(providers[0]["source_key"], "itunes_daily");
        assert_eq!(providers[1]["source_key"], "spotify_daily");
        assert_eq!(providers[2]["source_key"], "apple_music_daily");
        assert_eq!(providers[3]["source_key"], "youtube_daily");
        assert_eq!(providers[4]["source_key"], "shazam_daily");
        assert_eq!(providers[5]["source_key"], "deezer_daily");

        let rows = body["rows"].as_array().expect("rows");
        let global = rows
            .iter()
            .find(|row| row["region"] == "global")
            .expect("global row");
        assert_eq!(global["cells"]["spotify_daily"]["title"], "Top Song");
        assert_eq!(global["cells"]["spotify_daily"]["tidal_id"], 5001);
        assert_eq!(global["cells"]["spotify_daily"]["streams"], 550);
        assert_eq!(global["cells"]["spotify_daily"]["chart_date"], "2026-05-28");
        assert_eq!(global["cells"]["youtube_daily"]["title"], "Top Video");
        assert!(global["cells"]["itunes_daily"].is_null());
        assert!(global["cells"]["deezer_daily"].is_null());

        let au = rows
            .iter()
            .find(|row| row["region"] == "AU")
            .expect("AU row");
        assert_eq!(au["cells"]["shazam_daily"]["title"], "AU Shazam");
        assert!(au["cells"]["shazam_daily"]["tidal_id"].is_null());

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn spotify_daily_import_endpoint_persists_snapshot_for_matrix() {
        let (db, db_path) = fresh_migrated_db();
        let csv = r#"rank,uri,artist_names,track_name,peak_rank,previous_rank,days_on_chart,streams
1,spotify:track:imported,"Import Artist","Import Track",1,2,3,12345
"#;

        let response = json_request(
            app_for_db(db.clone()),
            "POST",
            "/api/charts/spotify/daily/import?region=AU&date=2026-05-28",
            csv,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["source"], "spotify_daily");
        assert_eq!(body["region"], "AU");
        assert_eq!(body["chart_date"], "2026-05-28");

        let response = json_request(app_for_db(db), "GET", "/api/charts/matrix", "").await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let au = body["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .find(|row| row["region"] == "AU")
            .expect("AU row");
        assert_eq!(au["cells"]["spotify_daily"]["title"], "Import Track");
        assert_eq!(au["cells"]["spotify_daily"]["streams"], 12345);

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn dj_enabled_true_starts_current_pair_lookahead() {
        let (db, db_path) = fresh_migrated_db();
        let (_current, next) = seed_dj_queue_pair(&db);
        let app = app_for_db(db);
        let response =
            json_request(app.clone(), "PUT", "/api/dj/enabled", r#"{"enabled":true}"#).await;
        assert_eq!(response.status(), StatusCode::OK);

        let response = json_request(app, "GET", "/api/dj/status", "").await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["enabled"], true);
        assert_eq!(body["next"]["media_ref_kind"], "tidal_track");
        assert_eq!(body["next"]["media_ref_id"], "77002");
        assert!(next > 0);
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn persisted_dj_enabled_builds_current_pair_lookahead_without_toggle() {
        let (db, db_path) = fresh_migrated_db();
        seed_dj_queue_pair(&db);
        db.with_conn(|conn| queries::set_dj_engine_enabled(conn, true))
            .expect("enable dj");
        let state = fresh_test_state(db);

        let start = active_dj_lookahead_start_for_state(&state).expect("lookahead start");

        assert_eq!(
            start.current,
            Some(crate::playback::dj_lookahead::DjMediaRef::TidalTrack {
                tidal_id: 77001,
                track_id: Some(7001),
            })
        );
        assert_eq!(
            start.next,
            Some(crate::playback::dj_lookahead::DjMediaRef::TidalTrack {
                tidal_id: 77002,
                track_id: Some(7002),
            })
        );
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn dj_lookahead_pairs_ephemeral_current_with_persisted_queue() {
        let (db, db_path) = fresh_migrated_db();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (7101, 'Queue Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id)
                 VALUES (7102, 'Queued Next', 7101, 180000, 88002)",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (7102, 0, 'user_queue')",
                [],
            )?;
            queries::set_dj_engine_enabled(conn, true)?;
            Ok(())
        })
        .expect("seed queue");
        let mut state = fresh_test_state(db);
        state.ephemeral_tidal_track = Some(crate::db::models::Track {
            id: -88001,
            title: "Direct Current".to_string(),
            artist_id: 0,
            artist_name: Some("Direct Artist".to_string()),
            album_id: None,
            album_title: None,
            disc_number: None,
            track_number: None,
            duration_ms: Some(180_000),
            isrc: None,
            tidal_id: Some(88001),
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
            artwork_url: None,
        });

        let start = active_dj_lookahead_start_for_state(&state).expect("lookahead start");

        assert_eq!(
            start.current,
            Some(crate::playback::dj_lookahead::DjMediaRef::TidalTrack {
                tidal_id: 88001,
                track_id: None,
            })
        );
        assert_eq!(
            start.next,
            Some(crate::playback::dj_lookahead::DjMediaRef::TidalTrack {
                tidal_id: 88002,
                track_id: Some(7102),
            })
        );
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn dj_enabled_false_cancels_lookahead_and_discards_program() {
        let (db, db_path) = fresh_migrated_db();
        seed_dj_queue_pair(&db);
        let app = app_for_db(db);
        let response =
            json_request(app.clone(), "PUT", "/api/dj/enabled", r#"{"enabled":true}"#).await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = json_request(
            app.clone(),
            "PUT",
            "/api/dj/enabled",
            r#"{"enabled":false}"#,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = json_request(app, "GET", "/api/dj/status", "").await;
        let body = response_json(response).await;
        assert_eq!(body["enabled"], false);
        assert_eq!(body["fallback_reason"], "disabled");
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn dj_profile_rebuild_does_not_claim_accepted_without_job() {
        let (db, db_path) = fresh_migrated_db();
        seed_dj_queue_pair(&db);
        let app = app_for_db(db);
        let response =
            json_request(app.clone(), "PUT", "/api/dj/enabled", r#"{"enabled":true}"#).await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = json_request(
            app,
            "POST",
            "/api/dj/profile-rebuild",
            r#"{"media_ref_kind":"tidal_track","media_ref_id":"77001"}"#,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["accepted"], false);
        assert_eq!(body["status"], "source_unavailable");
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn dj_profile_rebuild_does_not_mutate_armed_transition() {
        let (db, db_path) = fresh_migrated_db();
        seed_dj_queue_pair(&db);
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO dj_transition_events (
                    from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                    template, program_json, planner_version, outcome
                 ) VALUES ('tidal_track', '77001', 'tidal_track', '77002', 'SafeCrossfade', '{}', 'v1', 'armed')",
                [],
            )?;
            Ok(())
        })
        .expect("seed transition");
        let app = app_for_db(db.clone());
        let response = json_request(
            app,
            "POST",
            "/api/dj/profile-rebuild",
            r#"{"media_ref_kind":"tidal_track","media_ref_id":"77001"}"#,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let outcome: String = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT outcome FROM dj_transition_events LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(Into::into)
            })
            .expect("outcome");
        assert_eq!(outcome, "armed");
        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn dj_profile_returns_404_for_missing_profile() {
        let (db, db_path) = fresh_migrated_db();
        let response = json_request(app_for_db(db), "GET", "/api/dj/profile/999", "").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let _ = std::fs::remove_file(db_path);
    }

    fn seed_spotify_stats_track(
        db: &Database,
        album_id: Option<i64>,
        title: &str,
        isrc: &str,
        spotify_track_id: &str,
        playcount: i64,
    ) {
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (42, 'Stats Artist')",
                [],
            )?;
            if let Some(album_id) = album_id {
                conn.execute(
                    "INSERT INTO albums (id, title, artist_id, source)
                     VALUES (?1, 'Stats Album', 42, 'tidal')",
                    rusqlite::params![album_id],
                )?;
            }
            conn.execute(
                "INSERT INTO tracks (
                    id, title, artist_id, album_id, duration_ms, isrc, source, fidelity_score
                 ) VALUES (77, ?1, 42, ?2, 180000, ?3, 'tidal_stream', 0)",
                rusqlite::params![title, album_id, isrc],
            )?;

            let track = crate::services::sportify::models::SportifyTrack {
                id: Some(spotify_track_id.to_string()),
                playcount: Some(playcount),
                external_ids: Some(crate::services::sportify::models::SportifyExternalIds {
                    isrc: Some(isrc.to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            crate::services::sportify::stats::write_track_playcount(conn, &track);
            Ok::<_, anyhow::Error>(())
        })
        .expect("seed spotify stats track");
    }

    #[test]
    fn background_resolution_batches_are_bounded_by_concurrency() {
        let inputs: Vec<(String, crate::services::sportify::models::SportifyTrack)> = (0..13)
            .map(|i| {
                (
                    format!("sp-{i}"),
                    crate::services::sportify::models::SportifyTrack {
                        id: Some(format!("sp-{i}")),
                        name: Some(format!("Track {i}")),
                        artist: Some("Artist".to_string()),
                        ..Default::default()
                    },
                )
            })
            .collect();

        let batches = background_resolution_batches(&inputs, 6);

        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].len(), 6);
        assert_eq!(batches[1].len(), 6);
        assert_eq!(batches[2].len(), 1);
    }

    #[tokio::test]
    async fn album_spotify_stats_returns_cached_playcounts() {
        let (db, db_path) = fresh_migrated_db();
        seed_spotify_stats_track(
            &db,
            Some(9),
            "Album Stats Track",
            "ISRCALBUMSTATS",
            "sp-album-stats",
            1_234,
        );
        let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
            db.clone(),
        ))));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/albums/9/spotify-stats")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .expect("body bytes"),
        )
        .expect("json body");
        let tracks = body["tracks"].as_array().expect("tracks array");
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0]["isrc"], "ISRCALBUMSTATS");
        assert_eq!(tracks[0]["title"], "Album Stats Track");
        assert_eq!(tracks[0]["playcount"], 1_234);
        assert!(body["monthly_listeners"].is_null());

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn artist_spotify_stats_returns_cached_playcounts() {
        let (db, db_path) = fresh_migrated_db();
        seed_spotify_stats_track(
            &db,
            None,
            "Artist Stats Track",
            "ISRCARTISTSTATS",
            "sp-artist-stats",
            5_678,
        );
        let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
            db.clone(),
        ))));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/artists/42/spotify-stats")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .expect("body bytes"),
        )
        .expect("json body");
        let tracks = body["tracks"].as_array().expect("tracks array");
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0]["isrc"], "ISRCARTISTSTATS");
        assert_eq!(tracks[0]["title"], "Artist Stats Track");
        assert_eq!(tracks[0]["playcount"], 5_678);
        assert!(body["monthly_listeners"].is_null());

        let _ = std::fs::remove_file(db_path);
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
    async fn playback_shuffle_returns_debug_and_persists_seed() {
        let (db, db_path) = fresh_migrated_db();
        seed_basic_tracks(&db);
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (2, 1, 'user')",
                [],
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
                    .uri("/api/playback/shuffle")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"mode":"true"}"#))
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
        let seed = body["shuffle_debug"]["seed"]
            .as_i64()
            .expect("positive seed");
        assert!(seed > 0);
        assert_eq!(body["shuffle_debug"]["mode"], "true");
        assert_eq!(body["shuffle_debug"]["scope"], "playback_state");

        let stored_seed: Option<i64> = db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT shuffle_seed FROM playback_state WHERE id = 1",
                    [],
                    |row| row.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(stored_seed, Some(seed));

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn replace_playback_queue_accepts_one_shot_shuffle_mode() {
        let (db, db_path) = fresh_migrated_db();
        seed_basic_tracks(&db);

        let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
            db.clone(),
        ))));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/playback/queue")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"track_ids":[1,2],"shuffle_mode":"true"}"#))
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
        assert_eq!(body["queue"].as_array().expect("queue").len(), 2);
        assert_eq!(body["shuffle_debug"]["mode"], "true");
        assert_eq!(body["shuffle_debug"]["scope"], "queue_replace");

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn tidal_play_mix_shuffle_returns_debug_and_preserves_tracks() {
        let mut tracks = vec![
            PlayTidalRequest {
                tidal_track_id: 101,
                title: "One".to_string(),
                artist_name: Some("Artist A".to_string()),
                album_title: None,
                artwork_url: None,
                duration_ms: Some(180_000),
            },
            PlayTidalRequest {
                tidal_track_id: 102,
                title: "Two".to_string(),
                artist_name: Some("Artist B".to_string()),
                album_title: None,
                artwork_url: None,
                duration_ms: Some(181_000),
            },
            PlayTidalRequest {
                tidal_track_id: 103,
                title: "Three".to_string(),
                artist_name: Some("Artist C".to_string()),
                album_title: None,
                artwork_url: None,
                duration_ms: Some(182_000),
            },
        ];

        let debug = shuffle_tidal_mix_tracks(&mut tracks, Some("true")).expect("shuffle debug");
        let mut ids: Vec<i64> = tracks.iter().map(|track| track.tidal_track_id).collect();
        ids.sort_unstable();

        assert_eq!(ids, vec![101, 102, 103]);
        assert_eq!(debug.mode, "true");
        assert!(debug.seed > 0);
        assert_eq!(debug.scope, "tidal_mix");
        assert_eq!(debug.locked_count, 0);
        assert_eq!(debug.candidate_count, 3);
    }

    #[tokio::test]
    async fn genre_snapshot_route_returns_galaxy_payload() {
        let app = build_test_app().await;

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/genres/snapshot?days=30")
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

        assert!(body["genres"].is_array());
        assert!(body["heat"].is_array());
        assert!(body["cohorts"].is_array());
        assert!(body["evolution"].is_array());
        assert!(body["metrics"].is_array());
        assert_eq!(body["filter"], "confidence_0_50");
    }

    #[tokio::test]
    async fn discovery_space_includes_resolved_sidecar_external_neighbors() {
        let (db, db_path) = fresh_migrated_db();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (1, 'Seed Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id)
                 VALUES (1, 'Seed Track', 1, 200000, 1001)",
                [],
            )?;
            let model = queries::create_embedding_model(
                conn,
                "discovery-fusion-v2:space-external",
                "discovery-fusion-v2",
                2,
                "ready",
                None,
            )?;
            queries::activate_embedding_model(conn, model.id)?;
            let unresolved = queries::upsert_external_track_candidate(
                conn,
                &queries::ExternalTrackCandidateUpsert {
                    tidal_id: None,
                    mbid: None,
                    dedupe_key: "unresolved-space".to_string(),
                    title: "Unresolved External".to_string(),
                    artist_name: "Outside".to_string(),
                    genre_tags_json: None,
                    duration_ms: Some(180_000),
                    expires_at: "2099-01-01 00:00:00".to_string(),
                },
            )?;
            let resolved = queries::upsert_external_track_candidate(
                conn,
                &queries::ExternalTrackCandidateUpsert {
                    tidal_id: Some(990_001),
                    mbid: None,
                    dedupe_key: "tidal:990001".to_string(),
                    title: "Resolved External".to_string(),
                    artist_name: "Outside".to_string(),
                    genre_tags_json: None,
                    duration_ms: Some(181_000),
                    expires_at: "2099-01-01 00:00:00".to_string(),
                },
            )?;
            queries::replace_external_candidate_neighbors(
                conn,
                model.id,
                1,
                &[
                    queries::ExternalCandidateNeighborWriteRow {
                        candidate_id: unresolved.id,
                        rank: 1,
                        score: 0.99,
                        audio_score: 0.99,
                        metadata_score: 0.0,
                        reason_json: Some(r#"[{"key":"external_audio_proxy"}]"#.to_string()),
                    },
                    queries::ExternalCandidateNeighborWriteRow {
                        candidate_id: resolved.id,
                        rank: 2,
                        score: 0.91,
                        audio_score: 0.91,
                        metadata_score: 0.0,
                        reason_json: Some(r#"[{"key":"external_audio_proxy"}]"#.to_string()),
                    },
                ],
            )?;
            Ok(())
        })
        .expect("seed discovery space");

        let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
            db.clone(),
        ))));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/discovery/space")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"seed_track_id":1,"mode":"radio","limit":20}"#,
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
        let tracks = body["tracks"].as_array().expect("tracks array");
        let external = tracks
            .iter()
            .find(|track| track["track_id"] == 990_001)
            .expect("resolved external sidecar node");
        assert_eq!(external["source"], "external");
        assert_eq!(external["is_in_library"], false);
        assert_eq!(external["primary_reason"], "external");
        assert!(
            tracks
                .iter()
                .all(|track| track["title"] != "Unresolved External"),
            "unresolved external candidate must stay hidden"
        );
        let edges = body["edges"].as_array().expect("edges array");
        assert!(
            edges
                .iter()
                .any(|edge| { edge["from_track_id"] == 1 && edge["to_track_id"] == 990_001 })
        );

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn discovery_blend_space_includes_pending_external_nodes_and_health() {
        let (db, db_path) = fresh_migrated_db();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (1, 'Seed Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id)
                 VALUES
                    (1, 'Seed One', 1, 200000, 1001),
                    (2, 'Seed Two', 1, 201000, 1002)",
                [],
            )?;
            let model = queries::create_embedding_model(
                conn,
                "discovery-fusion-v2:blend-space",
                "discovery-fusion-v2",
                2,
                "ready",
                None,
            )?;
            queries::activate_embedding_model(conn, model.id)?;
            let pending = queries::upsert_external_track_candidate(
                conn,
                &queries::ExternalTrackCandidateUpsert {
                    tidal_id: None,
                    mbid: None,
                    dedupe_key: "pending-blend".to_string(),
                    title: "Pending Blend".to_string(),
                    artist_name: "Outside".to_string(),
                    genre_tags_json: None,
                    duration_ms: Some(180_000),
                    expires_at: "2099-01-01 00:00:00".to_string(),
                },
            )?;
            let resolved = queries::upsert_external_track_candidate(
                conn,
                &queries::ExternalTrackCandidateUpsert {
                    tidal_id: Some(990_002),
                    mbid: None,
                    dedupe_key: "tidal:990002".to_string(),
                    title: "Resolved Blend".to_string(),
                    artist_name: "Outside".to_string(),
                    genre_tags_json: None,
                    duration_ms: Some(181_000),
                    expires_at: "2099-01-01 00:00:00".to_string(),
                },
            )?;
            for seed_id in [1, 2] {
                queries::replace_external_candidate_neighbors(
                    conn,
                    model.id,
                    seed_id,
                    &[
                        queries::ExternalCandidateNeighborWriteRow {
                            candidate_id: pending.id,
                            rank: 1,
                            score: 0.94,
                            audio_score: 0.94,
                            metadata_score: 0.0,
                            reason_json: Some(r#"[{"key":"external_audio_proxy"}]"#.to_string()),
                        },
                        queries::ExternalCandidateNeighborWriteRow {
                            candidate_id: resolved.id,
                            rank: 2,
                            score: 0.90,
                            audio_score: 0.90,
                            metadata_score: 0.0,
                            reason_json: Some(r#"[{"key":"external_audio_proxy"}]"#.to_string()),
                        },
                    ],
                )?;
            }
            Ok(())
        })
        .expect("seed blend space");

        let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
            db.clone(),
        ))));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/discovery/blend/space")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"seeds":[{"kind":"library","track_id":1},{"kind":"library","track_id":2}],"limit":20}"#,
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
        let tracks = body["tracks"].as_array().expect("tracks array");
        let pending = tracks
            .iter()
            .find(|track| track["title"] == "Pending Blend")
            .expect("pending external blend node");
        assert_eq!(pending["role"], "external_candidate");
        assert_eq!(pending["playability"], "pending");
        assert_eq!(body["health"]["pending_external_count"], 1);
        assert_eq!(body["health"]["playable_external_count"], 1);
        assert!(body["health"]["coverage_ratio"].as_f64().unwrap() > 0.0);

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn discovery_blend_space_uses_external_seed_anchors() {
        let (db, db_path) = fresh_migrated_db();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (1, 'Guide Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id)
                 VALUES
                    (1, 'Guide One', 1, 200000, 1001),
                    (2, 'Guide Two', 1, 201000, 1002)",
                [],
            )?;
            let model = queries::create_embedding_model(
                conn,
                "discovery-fusion-v2:blend-external-seeds",
                "discovery-fusion-v2",
                2,
                "ready",
                None,
            )?;
            queries::activate_embedding_model(conn, model.id)?;
            let external_seed_one = queries::upsert_external_track_candidate(
                conn,
                &queries::ExternalTrackCandidateUpsert {
                    tidal_id: Some(991_001),
                    mbid: None,
                    dedupe_key: "tidal:991001".to_string(),
                    title: "External Seed One".to_string(),
                    artist_name: "Outside".to_string(),
                    genre_tags_json: None,
                    duration_ms: Some(180_000),
                    expires_at: "2099-01-01 00:00:00".to_string(),
                },
            )?;
            let external_seed_two = queries::upsert_external_track_candidate(
                conn,
                &queries::ExternalTrackCandidateUpsert {
                    tidal_id: Some(991_002),
                    mbid: None,
                    dedupe_key: "tidal:991002".to_string(),
                    title: "External Seed Two".to_string(),
                    artist_name: "Outside".to_string(),
                    genre_tags_json: None,
                    duration_ms: Some(181_000),
                    expires_at: "2099-01-01 00:00:00".to_string(),
                },
            )?;
            let shared_discovery = queries::upsert_external_track_candidate(
                conn,
                &queries::ExternalTrackCandidateUpsert {
                    tidal_id: Some(991_003),
                    mbid: None,
                    dedupe_key: "tidal:991003".to_string(),
                    title: "Shared Discovery".to_string(),
                    artist_name: "Outside".to_string(),
                    genre_tags_json: None,
                    duration_ms: Some(182_000),
                    expires_at: "2099-01-01 00:00:00".to_string(),
                },
            )?;
            queries::replace_external_candidate_neighbors(
                conn,
                model.id,
                1,
                &[
                    queries::ExternalCandidateNeighborWriteRow {
                        candidate_id: external_seed_one.id,
                        rank: 1,
                        score: 0.99,
                        audio_score: 0.99,
                        metadata_score: 0.0,
                        reason_json: Some(r#"[{"key":"external_audio_proxy"}]"#.to_string()),
                    },
                    queries::ExternalCandidateNeighborWriteRow {
                        candidate_id: shared_discovery.id,
                        rank: 2,
                        score: 0.91,
                        audio_score: 0.91,
                        metadata_score: 0.0,
                        reason_json: Some(r#"[{"key":"external_audio_proxy"}]"#.to_string()),
                    },
                ],
            )?;
            queries::replace_external_candidate_neighbors(
                conn,
                model.id,
                2,
                &[
                    queries::ExternalCandidateNeighborWriteRow {
                        candidate_id: external_seed_two.id,
                        rank: 1,
                        score: 0.99,
                        audio_score: 0.99,
                        metadata_score: 0.0,
                        reason_json: Some(r#"[{"key":"external_audio_proxy"}]"#.to_string()),
                    },
                    queries::ExternalCandidateNeighborWriteRow {
                        candidate_id: shared_discovery.id,
                        rank: 2,
                        score: 0.89,
                        audio_score: 0.89,
                        metadata_score: 0.0,
                        reason_json: Some(r#"[{"key":"external_audio_proxy"}]"#.to_string()),
                    },
                ],
            )?;
            Ok(())
        })
        .expect("seed external blend anchors");

        let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
            db.clone(),
        ))));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/discovery/blend/space")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"seeds":[{"kind":"tidal","tidal_id":991001,"title":"External Seed One","artist":"Outside"},{"kind":"tidal","tidal_id":991002,"title":"External Seed Two","artist":"Outside"}],"limit":20}"#,
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
        let tracks = body["tracks"].as_array().expect("tracks array");
        assert!(
            tracks
                .iter()
                .any(|track| track["title"] == "Shared Discovery"),
            "external blend seeds should produce anchored discoveries"
        );
        assert_eq!(body["health"]["playable_external_count"], 1);
        assert!(body["health"]["coverage_ratio"].as_f64().unwrap() > 0.0);

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn discovery_blend_add_appends_discoveries_without_replacing_queue() {
        let (db, db_path) = fresh_migrated_db();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (1, 'Seed Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id)
                 VALUES (1, 'Seed One', 1, 200000, 1001)",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user_queue')",
                [],
            )?;
            let model = queries::create_embedding_model(
                conn,
                "discovery-fusion-v2:blend-add",
                "discovery-fusion-v2",
                2,
                "ready",
                None,
            )?;
            queries::activate_embedding_model(conn, model.id)?;
            let resolved = queries::upsert_external_track_candidate(
                conn,
                &queries::ExternalTrackCandidateUpsert {
                    tidal_id: Some(990_003),
                    mbid: None,
                    dedupe_key: "tidal:990003".to_string(),
                    title: "Resolved Add".to_string(),
                    artist_name: "Outside".to_string(),
                    genre_tags_json: None,
                    duration_ms: Some(181_000),
                    expires_at: "2099-01-01 00:00:00".to_string(),
                },
            )?;
            queries::replace_external_candidate_neighbors(
                conn,
                model.id,
                1,
                &[queries::ExternalCandidateNeighborWriteRow {
                    candidate_id: resolved.id,
                    rank: 1,
                    score: 0.95,
                    audio_score: 0.95,
                    metadata_score: 0.0,
                    reason_json: Some(r#"[{"key":"external_audio_proxy"}]"#.to_string()),
                }],
            )?;
            Ok(())
        })
        .expect("seed blend add");

        let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
            db.clone(),
        ))));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/discovery/blend/add")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"seeds":[{"kind":"library","track_id":1}],"limit":10}"#,
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
        assert_eq!(body["pending_count"], 1);
        db.with_conn(|conn| {
            let rows: Vec<(i32, Option<i64>, Option<i64>)> = conn
                .prepare(
                    "SELECT position, track_id, tidal_id_hint FROM queue ORDER BY position ASC",
                )?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            assert_eq!(rows, vec![(0, Some(1), None), (1, None, Some(990_003))]);
            Ok(())
        })
        .unwrap();

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn tidal_mix_overlay_preserves_pending_deque_order() {
        let (db, db_path) = fresh_migrated_db();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (8101, 'Old Playlist Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, duration_ms)
                 VALUES
                    (8101, 'Old Playlist First', 8101, 200000),
                    (8102, 'Old Playlist Second', 8101, 200000)",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (track_id, position, source)
                 VALUES (8101, 0, 'playlist'), (8102, 1, 'playlist')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db)));
        {
            let mut guard = state.write().await;
            guard.ephemeral_tidal_track = Some(crate::db::models::Track {
                id: -158_296_914,
                title: "The Harder They Come".to_string(),
                artist_id: 0,
                artist_name: Some("Jimmy Cliff".to_string()),
                album_id: None,
                album_title: None,
                disc_number: None,
                track_number: None,
                duration_ms: Some(220_000),
                isrc: None,
                tidal_id: Some(158_296_914),
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
                artwork_url: None,
            });
            let mut pending = guard.pending_tidal_mix_queue.lock().unwrap();
            pending.push_back(crate::PendingEphemeralTidalTrack {
                tidal_track_id: 873_891_22,
                title: "The Big Tree".to_string(),
                artist_name: Some("Stand High Patrol".to_string()),
                album_title: None,
                artwork_url: None,
                duration_ms: Some(180_000),
            });
            pending.push_back(crate::PendingEphemeralTidalTrack {
                tidal_track_id: 341_378_223,
                title: "Positive".to_string(),
                artist_name: Some("Jamback".to_string()),
                album_title: None,
                artwork_url: None,
                duration_ms: Some(170_000),
            });
            pending.push_back(crate::PendingEphemeralTidalTrack {
                tidal_track_id: 172_522_829,
                title: "Moi Aussi Marianne".to_string(),
                artist_name: Some("Yatoba Lia".to_string()),
                album_title: None,
                artwork_url: None,
                duration_ms: Some(210_000),
            });
        }

        let snapshot = {
            let guard = state.read().await;
            guard.db.with_conn(player::load_snapshot).unwrap()
        };
        let snapshot = overlay_snapshot_with_external_track(&state, snapshot).await;

        assert_eq!(
            snapshot
                .state
                .current_track
                .as_ref()
                .map(|track| track.title.as_str()),
            Some("The Harder They Come")
        );
        let titles = snapshot
            .queue
            .iter()
            .map(|item| item.track.title.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            titles,
            vec!["The Big Tree", "Positive", "Moi Aussi Marianne"]
        );
        assert!(
            snapshot.queue.iter().all(|item| item.source == "tidal_mix"),
            "active TIDAL mix overlay must shadow the stale durable playlist queue"
        );

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn clear_queue_clears_pending_tidal_mix_overlay() {
        let (db, db_path) = fresh_migrated_db();
        let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db)));
        {
            let guard = state.read().await;
            guard.pending_tidal_mix_queue.lock().unwrap().push_back(
                crate::PendingEphemeralTidalTrack {
                    tidal_track_id: 987_654,
                    title: "Queued TIDAL Mix Track".to_string(),
                    artist_name: Some("TIDAL Artist".to_string()),
                    album_title: Some("TIDAL Mix".to_string()),
                    artwork_url: None,
                    duration_ms: Some(180_000),
                },
            );
        }
        let app = api_routes(state.clone());

        let resp = app
            .clone()
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

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/playback/queue")
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
        assert!(
            queue.is_empty(),
            "pending TIDAL mix overlay must not reappear after clear"
        );
        assert!(
            state
                .read()
                .await
                .pending_tidal_mix_queue
                .lock()
                .unwrap()
                .is_empty(),
            "pending TIDAL mix deque must be cleared"
        );

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn disabling_exclusive_clears_runtime_engaged_state() {
        let (db, db_path) = fresh_migrated_db();
        db.with_conn(|conn| {
            let mut settings = crate::db::audio_settings::AudioSettings::default();
            settings.exclusive_mode = true;
            crate::db::audio_settings::save(conn, &settings)?;
            Ok(())
        })
        .unwrap();

        let state = Arc::new(tokio::sync::RwLock::new(fresh_test_state(db)));
        {
            let mut guard = state.write().await;
            guard.playback_runtime_info = Some(PlaybackRuntimeInfo {
                device_name: "Test DAC".to_string(),
                sample_rate: 96_000,
                channels: 2,
                active_track_id: Some(1),
                last_error: None,
                exclusive_engaged: true,
                exclusive_transport_format: Some("i24-in-32".to_string()),
            });
        }
        let app = api_routes(state);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/playback/runtime")
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
        assert_eq!(body["runtime"]["exclusive_transport_format"], "i24-in-32");

        let mut next_settings = crate::db::audio_settings::AudioSettings::default();
        next_settings.exclusive_mode = false;
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/audio/settings")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&next_settings).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/playback/runtime")
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

        assert_eq!(body["runtime"]["exclusive_engaged"], false);
        assert_eq!(body["runtime"]["exclusive_transport_format"], Value::Null);

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn retries_exclusive_release_only_while_playing_with_exclusive_enabled() {
        assert!(should_retry_exclusive_release(true, true));
        assert!(!should_retry_exclusive_release(false, true));
        assert!(!should_retry_exclusive_release(true, false));
        assert!(!should_retry_exclusive_release(false, false));
    }

    #[test]
    fn exclusive_crossfade_policy_suppresses_crossfade() {
        assert_eq!(effective_crossfade_for_exclusive(true, false, 1_500), 0);
        assert_eq!(
            effective_crossfade_for_exclusive(false, false, 1_500),
            1_500
        );
        assert_eq!(effective_crossfade_for_exclusive(true, true, 1_500), 1_500);
        assert_eq!(effective_crossfade_for_exclusive(true, true, -10), 0);
    }

    #[test]
    fn next_prebuffer_slot_suppresses_duplicate_pair_only() {
        let key = crate::NextPrebufferKey {
            current_track_id: 1,
            next_track_id: 2,
            generation: 3,
        };
        let replacement = crate::NextPrebufferKey {
            current_track_id: 1,
            next_track_id: 4,
            generation: 3,
        };
        let mut slot = None;

        assert!(claim_next_prebuffer_slot(&mut slot, key));
        assert!(!claim_next_prebuffer_slot(&mut slot, key));
        assert!(claim_next_prebuffer_slot(&mut slot, replacement));
        release_next_prebuffer_slot(&mut slot, key);
        assert_eq!(slot, Some(replacement));
        release_next_prebuffer_slot(&mut slot, replacement);
        assert_eq!(slot, None);
    }

    #[test]
    fn exclusive_sample_rate_follow_skips_prebuffer_on_rate_change() {
        assert!(should_skip_prebuffer_for_sample_rate_follow_format_change(
            true,
            true,
            44_100,
            Some(96_000),
            Some(16),
            Some(24),
        ));
        assert!(!should_skip_prebuffer_for_sample_rate_follow_format_change(
            true,
            true,
            96_000,
            Some(96_000),
            Some(24),
            Some(24),
        ));
        assert!(!should_skip_prebuffer_for_sample_rate_follow_format_change(
            true,
            false,
            44_100,
            Some(96_000),
            Some(16),
            Some(24),
        ));
        assert!(!should_skip_prebuffer_for_sample_rate_follow_format_change(
            true,
            true,
            44_100,
            None,
            Some(16),
            Some(16),
        ));
        assert!(should_skip_prebuffer_for_sample_rate_follow_format_change(
            true,
            true,
            44_100,
            Some(44_100),
            Some(16),
            Some(24),
        ));
    }

    #[test]
    fn shared_sample_rate_follow_skips_prebuffer_on_rate_change() {
        assert!(should_skip_prebuffer_for_sample_rate_follow_format_change(
            false,
            true,
            44_100,
            Some(96_000),
            Some(16),
            Some(24),
        ));
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
    async fn create_playlist_from_queue_imports_pending_tidal_rows_with_hint() {
        let (db, db_path) = fresh_migrated_db();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO queue (
                    track_id, position, source, pending_artist, pending_title, pending_at, tidal_id_hint
                 ) VALUES (NULL, 0, 'user_queue', 'Queued Artist', 'Queued TIDAL', datetime('now'), 777)",
                [],
            )?;
            Ok(())
        })
        .expect("seed pending queue row");

        let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
            db.clone(),
        ))));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/playlists/from-queue")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"Saved pending TIDAL","include_tidal_only":true}"#,
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
        assert_eq!(body["added"], 1);

        let saved: Vec<(i64, String)> = db
            .with_conn(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT t.tidal_id, t.title
                     FROM playlist_tracks pt
                     JOIN playlists p ON p.id = pt.playlist_id
                     JOIN tracks t ON t.id = pt.track_id
                     WHERE p.name = 'Saved pending TIDAL'
                     ORDER BY pt.position ASC",
                )?;
                Ok(stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .unwrap();
        assert_eq!(saved, vec![(777, "Queued TIDAL".to_string())]);

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn create_playlist_from_queue_imports_ephemeral_tidal_overlay() {
        let (db, db_path) = fresh_migrated_db();
        let mut state = fresh_test_state(db.clone());
        let mut current = test_track(-901, "Current TIDAL");
        current.tidal_id = Some(901);
        current.artist_name = Some("Current Artist".to_string());
        current.album_title = Some("Current Album".to_string());
        current.source = "tidal_ephemeral".to_string();
        state.ephemeral_tidal_track = Some(current);
        {
            let mut pending = state.pending_tidal_mix_queue.lock().unwrap();
            pending.push_back(crate::PendingEphemeralTidalTrack {
                tidal_track_id: 902,
                title: "Next TIDAL".to_string(),
                artist_name: Some("Next Artist".to_string()),
                album_title: Some("Next Album".to_string()),
                artwork_url: Some("https://resources.tidal.com/images/cover.jpg".to_string()),
                duration_ms: Some(181_000),
            });
            pending.push_back(crate::PendingEphemeralTidalTrack {
                tidal_track_id: 903,
                title: "Third TIDAL".to_string(),
                artist_name: None,
                album_title: None,
                artwork_url: None,
                duration_ms: None,
            });
        }

        let app = api_routes(Arc::new(tokio::sync::RwLock::new(state)));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/playlists/from-queue")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"Saved ephemeral TIDAL","include_tidal_only":true}"#,
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
        assert_eq!(body["added"], 3);

        let saved: Vec<(i64, String)> = db
            .with_conn(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT t.tidal_id, t.title
                     FROM playlist_tracks pt
                     JOIN playlists p ON p.id = pt.playlist_id
                     JOIN tracks t ON t.id = pt.track_id
                     WHERE p.name = 'Saved ephemeral TIDAL'
                     ORDER BY pt.position ASC",
                )?;
                Ok(stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .unwrap();
        assert_eq!(
            saved,
            vec![
                (901, "Current TIDAL".to_string()),
                (902, "Next TIDAL".to_string()),
                (903, "Third TIDAL".to_string())
            ]
        );

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
    async fn promote_pending_row_emit_marks_external_candidate_resolved() {
        let db_path = std::env::temp_dir().join(format!("noor-test-{}.db", uuid::Uuid::new_v4()));
        let db = Database::open(&db_path).expect("db opened");
        db.run_migrations().expect("migrations");
        db.with_conn(|conn| schema::run_migrations(conn))
            .expect("schema migrations");

        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (1, 'Resolved Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (
                    id, title, artist_id, tidal_id, source, fidelity_score
                 ) VALUES (1, 'Resolved Title', 1, 4242, 'tidal_stream', 0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO external_track_candidates (
                    tidal_id, dedupe_key, title, artist_name, expires_at
                 ) VALUES (4242, 'tidal:4242', 'Resolved Title', 'Resolved Artist', '2026-03-01 00:00:00')",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (
                    track_id, position, source, pending_artist, pending_title, pending_at, tidal_id_hint
                 ) VALUES (NULL, 0, 'automix-new', 'Resolved Artist', 'Resolved Title', datetime('now'), 4242)",
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

        let (event_tx, _rx) = tokio::sync::broadcast::channel(8);
        let promoted = promote_pending_row_emit(&db, &event_tx, queue_item_id, 1, 990);
        assert!(promoted);

        let resolved: Option<i64> = db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT resolved_track_id FROM external_track_candidates WHERE tidal_id = 4242",
                    [],
                    |row| row.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(resolved, Some(1));

        let _ = std::fs::remove_file(db_path);
    }

    #[test]
    fn automix_discover_new_fallback_waits_when_sidecar_new_rows_fill_slots() {
        let current = test_track(1, "Current");
        let mut snapshot = crate::playback::player::PlaybackSnapshot {
            state: crate::db::models::PlaybackState {
                current_track: Some(current.clone()),
                current_queue_item_id: Some(10),
                position_ms: 0,
                is_playing: true,
                volume: 1.0,
                shuffle_mode: "off".to_string(),
                repeat_mode: "off".to_string(),
                automix_enabled: true,
                crossfade_ms: 0,
                automix_discover_new: true,
                automix_use_learning: true,
                automix_allow_external: true,
                buffered_ms: 0,
                buffered_start_ms: 0,
            },
            queue: vec![
                test_queue_item(10, current, 0, "manual"),
                test_queue_item(11, test_track(2, "Sidecar A"), 1, "automix-new"),
                test_queue_item(12, test_track(3, "Sidecar B"), 2, "automix-new"),
            ],
        };

        assert!(automix_discover_new_fallback_seed(&snapshot).is_none());

        snapshot.queue.pop();

        assert!(automix_discover_new_fallback_seed(&snapshot).is_some());
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
    /// ephemeral negative - it usually indicates a serialisation
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

    // Characterization tests for TIDAL-handler failure shapes. These differ on
    // purpose: the album endpoint is "best-effort" (TIDAL is enrichment) while
    // tidal_search treats a disconnected session as a user-visible error.
    // A single shared error helper would erase this distinction - so these tests
    // exist to flag any refactor that does.

    #[tokio::test]
    async fn get_album_tracks_returns_local_tracks_when_tidal_session_absent() {
        let (db, db_path) = fresh_migrated_db();
        // The album MUST have a tidal_id, otherwise the handler returns at
        // routes.rs:977 with album_tidal_id: null - which is a different code
        // path. To exercise the "no TIDAL session" branch (routes.rs:995) we
        // need a TIDAL-mapped album with no session.
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (1, 'Local Artist')",
                [],
            )?;
            conn.execute(
                "INSERT INTO albums (id, tidal_id, title, artist_id, source)
                 VALUES (5, 8888, 'Local Album', 1, 'tidal')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (
                    id, title, artist_id, album_id, duration_ms, source, fidelity_score
                 ) VALUES (10, 'Local Track', 1, 5, 180000, 'tidal_stream', 0)",
                [],
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .expect("seed");

        // fresh_test_state has tidal_tokens: None -> the session is "disconnected".
        let app = api_routes(Arc::new(tokio::sync::RwLock::new(fresh_test_state(
            db.clone(),
        ))));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/albums/5/tracks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // Best-effort path: the album still resolves, local tracks are returned,
        // tidal_tracks is empty, album_tidal_id is preserved. Do NOT change this
        // to 400 / 502 / hide the album_tidal_id in a refactor.
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let tracks = body["tracks"].as_array().expect("tracks array");
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0]["title"], "Local Track");
        let tidal_tracks = body["tidal_tracks"].as_array().expect("tidal_tracks array");
        assert!(
            tidal_tracks.is_empty(),
            "tidal_tracks must be [] when disconnected"
        );
        assert_eq!(
            body["album_tidal_id"], 8888,
            "album_tidal_id must survive even when session is absent"
        );

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn tidal_search_returns_400_when_tidal_session_absent() {
        // Opposite of the album endpoint: tidal_search has no library fallback,
        // so a missing session must surface as a user-visible 400, not silently
        // return an empty result set.
        let app = build_test_app().await;
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/tidal/search?q=foo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let error = body["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("TIDAL not connected"),
            "expected 'TIDAL not connected' error, got: {body}"
        );
    }

    // Note: an integration test for the recover_tidal_session path on a 401 upstream
    // response is deferred - it requires intercepting the reqwest::Client, which
    // requires wiremock or a trait-based http client. Until that infra lands, the
    // refresh-on-auth path in tidal_search / tidal_video_playback / tidal_playlist_*
    // remains uncovered.

    // Library-track filter predicate (favorite_only / liked_only) is already
    // covered by characterization-grade tests in db/queries.rs:
    //   - liked_only_excludes_album_favorited_tracks (queries.rs:7931)
    //   - favorite_only_preserves_legacy_union_behavior (queries.rs:7949)
    //   - liked_only_takes_precedence_over_favorite_only (queries.rs:8036)
    // Do not add duplicates here.

    /// Route-registration smoke test. Probes every `/api/*` route registered in
    /// `api_routes` with a deliberately-wrong HTTP method and asserts the
    /// response is NOT 404.
    ///
    /// Why wrong-method: axum returns 405 METHOD_NOT_ALLOWED when a path is
    /// registered but the method doesn't match, and 404 NOT_FOUND when the path
    /// isn't registered at all. Probing with the wrong method means routing
    /// resolves before any extractor runs - so path params, request bodies, and
    /// auth never enter the picture. A 404 here means the route is genuinely
    /// missing, which is exactly the failure mode a careless handler extraction
    /// introduces: move a handler to a submodule, forget to re-register it.
    ///
    /// This is a structural guard, NOT a behavior contract. It deliberately
    /// does not assert on bodies or success codes. Per-route behavior belongs
    /// in dedicated tests. When you add a route, add it here too.
    #[tokio::test]
    async fn all_api_routes_are_registered() {
        // (real_method, path). Path params use concrete sentinels; the value is
        // irrelevant because the wrong-method probe short-circuits at routing.
        let routes: &[(&str, &str)] = &[
            ("GET", "/api/tracks"),
            ("GET", "/api/tracks/count"),
            ("GET", "/api/albums"),
            ("GET", "/api/albums/1/tracks"),
            ("GET", "/api/albums/1/spotify-stats"),
            ("GET", "/api/artists"),
            ("GET", "/api/artists/1"),
            ("GET", "/api/artists/1/tracks"),
            ("GET", "/api/artists/1/discography"),
            ("GET", "/api/artists/1/spotify-stats"),
            ("GET", "/api/tidal/albums/1/tracks"),
            ("POST", "/api/tidal/albums/1/import"),
            ("POST", "/api/tidal/tracks/import"),
            ("GET", "/api/genres"),
            ("GET", "/api/genres/snapshot"),
            ("GET", "/api/genres/heat"),
            ("GET", "/api/genres/co-occurrence"),
            ("GET", "/api/genres/cohorts"),
            ("GET", "/api/genres/evolution"),
            ("GET", "/api/genres/audio-metrics"),
            ("GET", "/api/genres/1/tracks"),
            ("GET", "/api/playlists"),
            ("GET", "/api/playlists/1/tracks"),
            ("PATCH", "/api/playlists/1/favorite"),
            ("GET", "/api/playlists/1/cover-sample"),
            ("POST", "/api/smart/playlists"),
            ("PUT", "/api/smart/playlists/1"),
            ("GET", "/api/smart/playlists/1/evaluate"),
            ("POST", "/api/smart/playlists/preview"),
            ("GET", "/api/artists/search"),
            ("GET", "/api/analytics/overview"),
            ("GET", "/api/analytics/dashboard"),
            ("GET", "/api/analytics/signals"),
            ("GET", "/api/analytics/listens/recent"),
            ("POST", "/api/discovery/preview"),
            ("POST", "/api/discovery/new"),
            ("POST", "/api/discovery/save"),
            ("POST", "/api/discovery/play"),
            ("POST", "/api/discovery/connections"),
            ("GET", "/api/discovery/status"),
            ("POST", "/api/discovery/train"),
            ("GET", "/api/discovery/train/status"),
            ("POST", "/api/discovery/train/stop"),
            ("GET", "/api/discovery/train/intensity"),
            ("GET", "/api/discovery/train/engine"),
            ("GET", "/api/discovery/train/safety"),
            ("GET", "/api/discovery/train/safety-profile"),
            ("POST", "/api/discovery/feedback"),
            ("GET", "/api/discovery/presets"),
            ("POST", "/api/discovery/radio"),
            ("POST", "/api/discovery/radio/compute"),
            ("POST", "/api/discovery/space"),
            ("POST", "/api/discovery/blend/space"),
            ("POST", "/api/discovery/blend/add"),
            ("POST", "/api/discovery/blend/play"),
            ("POST", "/api/discovery/blend/radio"),
            ("GET", "/api/resolve/tidal/track"),
            ("POST", "/api/resolve/tidal/bulk"),
            ("GET", "/api/resolve/tidal/status"),
            ("GET", "/api/discovery/sportify/search"),
            ("GET", "/api/discovery/sportify/track/x"),
            ("GET", "/api/discovery/sportify/album/x"),
            ("GET", "/api/discovery/sportify/playlist/x"),
            ("GET", "/api/discovery/sportify/artist/x"),
            ("GET", "/api/discovery/sportify/artist/x/top-tracks"),
            ("GET", "/api/discovery/sportify/artist/x/related"),
            ("GET", "/api/discovery/sportify/album/x/related"),
            ("GET", "/api/discovery/sportify/track/x/related"),
            ("POST", "/api/spotify-playlist/save"),
            ("POST", "/api/radio/song"),
            ("POST", "/api/radio/album"),
            ("POST", "/api/radio/artist"),
            ("POST", "/api/radio/start"),
            ("GET", "/api/discovery/space/meta"),
            ("GET", "/api/discovery/artists"),
            ("POST", "/api/library/batch/add-to-playlist"),
            ("POST", "/api/library/batch/delete"),
            ("POST", "/api/library/batch/set-genre"),
            ("POST", "/api/library/enrich/musicbrainz"),
            ("GET", "/api/library/enrich/musicbrainz/status"),
            ("GET", "/api/library/enrich/musicbrainz/portable"),
            ("POST", "/api/library/enrich/musicbrainz/portable/export"),
            ("POST", "/api/library/enrich/musicbrainz/portable/import"),
            ("POST", "/api/library/tracks/favorite"),
            ("POST", "/api/library/duplicates/scan"),
            ("GET", "/api/library/duplicates"),
            ("POST", "/api/library/duplicates/1/resolve"),
            ("POST", "/api/library/duplicates/1/dismiss"),
            ("GET", "/api/playback/state"),
            ("GET", "/api/playback/runtime"),
            ("POST", "/api/playback/play"),
            ("POST", "/api/playback/pause"),
            ("POST", "/api/playback/resume"),
            ("POST", "/api/playback/previous"),
            ("POST", "/api/playback/next"),
            ("POST", "/api/playback/position"),
            ("POST", "/api/playback/volume"),
            ("POST", "/api/playback/shuffle"),
            ("POST", "/api/playback/repeat"),
            ("POST", "/api/playback/automix"),
            ("GET", "/api/playback/queue"),
            ("POST", "/api/playback/queue/add"),
            ("POST", "/api/playback/queue/remove"),
            ("POST", "/api/playback/queue/move"),
            ("POST", "/api/playback/queue/clear"),
            ("POST", "/api/queue/play_next"),
            ("POST", "/api/queue/play_next_many"),
            ("POST", "/api/queue/append"),
            ("POST", "/api/queue/append_many"),
            ("POST", "/api/playlists/from-queue"),
            ("GET", "/api/audio/devices"),
            ("GET", "/api/audio/settings"),
            ("POST", "/api/audio/exclusive/retry"),
            ("GET", "/api/search"),
            ("POST", "/api/search/audio"),
            ("GET", "/api/search/vibe"),
            ("GET", "/api/search/underrated"),
            ("POST", "/api/tidal/login"),
            ("POST", "/api/tidal/login/complete"),
            ("POST", "/api/tidal/login/poll"),
            ("POST", "/api/tidal/sync"),
            ("POST", "/api/tidal/sync/cancel"),
            ("GET", "/api/tidal/status"),
            ("GET", "/api/tidal/search"),
            ("GET", "/api/tidal/videos/search"),
            ("GET", "/api/tidal/videos/1/playback"),
            ("GET", "/api/tidal/video-mixes/1/items"),
            ("GET", "/api/tidal/playlists/search"),
            ("GET", "/api/tidal/playlists/x/tracks"),
            ("POST", "/api/tidal/play"),
            ("GET", "/api/tidal/artists/1"),
            ("POST", "/api/tidal/logout"),
            ("POST", "/api/spotify/config"),
            ("GET", "/api/spotify/status"),
            ("POST", "/api/library/enrich/spotify"),
            ("GET", "/api/library/enrich/spotify/status"),
            ("POST", "/api/library/enrich/spotify/reset"),
            ("POST", "/api/library/tidal-stream/purge"),
            ("POST", "/api/lastfm/config"),
            ("GET", "/api/lastfm/config"),
            ("DELETE", "/api/lastfm/config"),
            ("GET", "/api/lastfm/status"),
            ("POST", "/api/listenbrainz/config"),
            ("GET", "/api/listenbrainz/config"),
            ("DELETE", "/api/listenbrainz/config"),
            ("GET", "/api/listenbrainz/status"),
            ("POST", "/api/lastfm/auth/start"),
            ("POST", "/api/lastfm/auth/complete"),
            ("POST", "/api/lastfm/auth/disconnect"),
            ("POST", "/api/library/enrich/lastfm"),
            ("POST", "/api/library/enrich/lastfm/stop"),
            ("GET", "/api/library/enrich/lastfm/status"),
            ("POST", "/api/library/enrich/lastfm/reset"),
            ("POST", "/api/scrobbling/backfill"),
            ("POST", "/api/library/analyze/audio-features"),
            ("POST", "/api/library/analyze/stop"),
            ("GET", "/api/library/analyze/status"),
            ("GET", "/api/library/analyze/passive"),
            ("GET", "/api/tracks/1/audio-features"),
            ("POST", "/api/tracks/1/bpm-multiplier"),
            ("GET", "/api/library/audio-features/stats"),
            ("GET", "/api/library/audio-features/quality"),
            ("GET", "/api/library/analytics"),
            ("GET", "/api/library/analyze/reanalyze-stale"),
            ("POST", "/api/library/analyze/reset"),
            ("GET", "/api/sync/info"),
            ("POST", "/api/sync/auto"),
            ("GET", "/api/status"),
            ("GET", "/api/home/releases"),
            ("GET", "/api/home/picks"),
            ("GET", "/api/home/articles"),
            ("GET", "/api/home/news"),
            ("GET", "/api/home/recommendations"),
            ("GET", "/api/tidal/mixes"),
            ("GET", "/api/tidal/mixes/1/tracks"),
            ("POST", "/api/tidal/play-mix"),
            ("GET", "/api/tidal/radio-stations"),
            ("GET", "/api/tidal/home-modules"),
            ("GET", "/api/tidal/discover-modules/1/items"),
            ("GET", "/api/tidal/page/mood/1"),
            ("GET", "/api/tidal/moods"),
            ("GET", "/api/tidal/mood-page/mood_party"),
            ("GET", "/api/charts"),
            ("GET", "/api/charts/snapshots"),
            ("POST", "/api/charts/spotify/daily/import"),
            ("GET", "/api/charts/matrix"),
            ("POST", "/api/charts/matrix/refresh"),
            ("GET", "/api/charts/lastfm/genres"),
            ("GET", "/api/charts/lastfm/countries"),
            ("GET", "/api/server/token"),
            ("POST", "/api/server/token/regenerate"),
            ("GET", "/api/server/info"),
            ("PUT", "/api/server/host_mode"),
        ];

        let app = build_test_app().await;
        let mut missing = Vec::new();
        for (method, path) in routes {
            // Probe with a method the route does NOT use, so routing resolves
            // to 405 (registered) or 404 (not registered) before extractors run.
            let probe = if *method == "GET" { "POST" } else { "GET" };
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(probe)
                        .uri(*path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            if resp.status() == StatusCode::NOT_FOUND {
                missing.push(format!("{method} {path}"));
            }
        }

        assert!(
            missing.is_empty(),
            "routes returned 404 to a wrong-method probe (not registered):\n  {}",
            missing.join("\n  ")
        );
    }
}
