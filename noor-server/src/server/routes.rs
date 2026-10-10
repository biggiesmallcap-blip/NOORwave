use super::transport::command as transport_command;
use super::transport::events::{
    describe_tidal_playback_error, handle_near_end,
    mark_armed_dj_transition_manual_seek_suppressed_if_needed, report_playback_failure,
    switch_runtime_to_snapshot_current,
};
use super::transport::generation::{
    bump as bump_playback_generation, current as current_playback_generation,
};
use super::transport::listen::{
    flush_active_listen_session_locked, record_transition_if_changed,
    resume_session_after_snapshot, sync_session_after_snapshot,
};
use super::transport::pending::{
    resolve_or_skip_pending_current, resolve_pending_row, spawn_pending_queue_resolver,
};
use super::transport::runtime::{RuntimeUnavailable, current as current_playback_runtime};
use super::transport::settings::{
    reissue_current_track_at_new_quality, runtime_output_settings_from_audio_settings,
};
use super::transport::snapshot::build_live_playback_snapshot;
use super::transport::snapshot::{
    current_live_position_ms, current_playback_track_id, overlay_snapshot_with_external_track,
    overlay_snapshot_with_external_track_and_position, recently_cleared,
};
use super::transport::start::{Dispatch, StartError};
use super::transport::stream::{
    TidalPlaybackError, resolve_tidal_playback_stream, runtime_stream_resolver,
};
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
use axum::{
    Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Json, Response},
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
mod artwork_cache_routes;
mod audio_analysis_routes;
pub(crate) mod catalog_routes;
mod catalogue_routes;
mod chart_routes;
mod discovery_routes;
mod discovery_space_routes;
pub(crate) mod dj_routes;
mod download_routes;
mod duplicates_routes;
mod enrichment_routes;
mod genre_routes;
pub(crate) mod home_routes;
pub(crate) mod home_suggestions;
mod library_batch_routes;
pub(crate) mod maintenance_routes;
mod playlist_routes;
mod search_routes;
mod setup_discovery_routes;
mod sportify_routes;
mod tidal_content_routes;
mod tidal_home_routes;
mod tidal_sync_routes;
mod video_discovery_routes;
mod video_station_routes;
pub use discovery_routes::{TrainingSpawn, spawn_discovery_training};
pub use tidal_sync_routes::trigger_auto_sync;

type TidalPlaylistTracksCache = Arc<Mutex<HashMap<String, (Instant, Vec<TidalTrack>)>>>;
type DropPreviewArmKey = (usize, i64, i64, u64, u64);

const TIDAL_PLAYLIST_TRACKS_CACHE_TTL: Duration = Duration::from_secs(60 * 60);
const DJ_LOOKAHEAD_DEADLINE_SAMPLES: u64 = 48_000 * 30;
const DROP_PREVIEW_DURATION_MS: u32 = 16_000;
const DROP_PREVIEW_ARM_RETRY_SECS: u64 = 60 * 60;
pub(crate) const PLAYBACK_FINISH_DB_LOCK_RETRY_LIMIT: usize = 60;
pub(crate) const PLAYBACK_FINISH_DB_LOCK_RETRY_DELAY_SECS: u64 = 2;
pub(crate) const PLAYBACK_ADVANCE_PENDING_SKIP_LIMIT: usize = 8;
pub(crate) const PLAYBACK_PENDING_BUSY_RETRY_LIMIT: usize = 5;
pub(crate) const PLAYBACK_PENDING_BUSY_RETRY_DELAY_MS: u64 = 200;
const TIDAL_SEARCH_UPSTREAM_TIMEOUT_SECS: u64 = 8;

static DROP_PREVIEW_ARM_ATTEMPTS: OnceLock<Mutex<HashMap<DropPreviewArmKey, Instant>>> =
    OnceLock::new();
// One preparation task per active pair, including when a seek emits Started.
static DJ_PAIR_PREPARATION_TASKS: OnceLock<Mutex<HashSet<(usize, i64, i64, u64, u64)>>> =
    OnceLock::new();

struct DjPairPreparationGuard((usize, i64, i64, u64, u64));
impl Drop for DjPairPreparationGuard {
    fn drop(&mut self) {
        if let Some(tasks) = DJ_PAIR_PREPARATION_TASKS.get()
            && let Ok(mut tasks) = tasks.lock()
        {
            tasks.remove(&self.0);
        }
    }
}

pub(crate) async fn queue_missing_dj_profiles_after_pair_change(
    state: SharedState,
    context: &'static str,
) {
    if let Err(status) = dj_routes::queue_missing_dj_profiles_for_current_pair(state).await {
        warn!(
            ?status,
            context, "DJ profile queueing failed after pair change"
        );
    }
}

pub(crate) fn active_dj_lookahead_start_for_state(
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
                DJ_LOOKAHEAD_DEADLINE_SAMPLES,
            ))
        })
        .ok()
        .flatten()
}

pub(crate) async fn start_dj_lookahead_and_queue_profiles_after_pair_change(
    state: SharedState,
    handle: playback_runtime::PlaybackRuntimeHandle,
    context: &'static str,
) {
    let (lookahead, playback_generation) = {
        let state_guard = state.read().await;
        (
            active_dj_lookahead_start_for_state(&state_guard),
            current_playback_generation(&state_guard),
        )
    };
    if let Some(lookahead) = lookahead {
        let _ = lookahead.dispatch(&handle);
        spawn_dj_pair_preparation(
            state.clone(),
            handle.clone(),
            lookahead.clone(),
            playback_generation,
        );
        spawn_drop_preview_scheduler(state.clone(), handle, lookahead, playback_generation);
    }
    queue_missing_dj_profiles_after_pair_change(state, context).await;
}

fn spawn_dj_pair_preparation(
    state: SharedState,
    handle: playback_runtime::PlaybackRuntimeHandle,
    lookahead: player::DjLookaheadStart,
    playback_generation: u64,
) {
    let (Some(track_id), Some(next_queue_id)) = (
        lookahead.current.as_ref().and_then(|r| r.track_id()),
        lookahead.next_queue_item_id,
    ) else {
        return;
    };
    let key = (
        Arc::as_ptr(&state) as usize,
        track_id,
        next_queue_id,
        lookahead.queue_generation,
        playback_generation,
    );
    if !DJ_PAIR_PREPARATION_TASKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(key)
    {
        return;
    }
    tokio::spawn(async move {
        let _guard = DjPairPreparationGuard(key);
        let mut prepare_attempts = 0_u64;
        let mut next_prepare_attempt = Instant::now();
        loop {
            let same_pair = {
                let state_guard = state.read().await;
                state_guard
                    .playback_runtime
                    .as_ref()
                    .is_some_and(|runtime| runtime.handle.is_same_runtime(&handle))
                    && current_playback_generation(&state_guard) == playback_generation
                    && state_guard
                        .playback_runtime_info
                        .as_ref()
                        .and_then(|info| info.active_track_id)
                        == Some(track_id)
                    && active_dj_lookahead_start_for_state(&state_guard).is_some_and(|pair| {
                        pair.current_queue_item_id == lookahead.current_queue_item_id
                            && pair.next_queue_item_id == lookahead.next_queue_item_id
                            && pair.queue_generation == lookahead.queue_generation
                    })
            };
            if !same_pair {
                break;
            }
            // Use the established peek/prebuffer path from track start. Keep
            // network resolution away from the runtime event listener.
            if prepare_attempts < 3 && Instant::now() >= next_prepare_attempt {
                match handle_near_end(state.clone(), track_id, playback_generation).await {
                    Ok(false) => {} // Already prepared or another preparation owns the slot.
                    result => {
                        prepare_attempts += 1;
                        next_prepare_attempt =
                            Instant::now() + Duration::from_secs(20 * prepare_attempts);
                        if let Err(error) = result {
                            warn!("Early DJ preparation skipped: {error:?}");
                        }
                    }
                }
            }
            if let Err(error) = refresh_prepared_dj_transition(&state, &handle).await {
                warn!("Prepared DJ plan refresh skipped: {error:?}");
            }
            // Profiles may arrive after the pair starts playing. An absent
            // preview plan must not consume its one-preview-per-pair latch.
            if let Err(error) = schedule_drop_preview_for_pair(
                state.clone(),
                handle.clone(),
                lookahead.clone(),
                playback_generation,
            )
            .await
            {
                warn!("Drop preview scheduling skipped: {error:?}");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

async fn refresh_prepared_dj_transition(
    state: &SharedState,
    handle: &playback_runtime::PlaybackRuntimeHandle,
) -> anyhow::Result<()> {
    let (engine, update) = {
        let state_guard = state.read().await;
        let Some(info) = state_guard.playback_runtime_info.as_ref() else {
            return Ok(());
        };
        let pair = state_guard
            .db
            .with_conn(|conn| active_dj_pair_for_state_and_conn(&state_guard, conn))?;
        let engine = crate::playback::dj_engine::DjEngine::new(state_guard.db.clone());
        if !state_guard.db.with_conn(queries::is_dj_engine_enabled)? {
            return Ok(());
        }
        let update = player::plan_prepared_dj_transition_update(
            &engine,
            pair,
            info.sample_rate,
            info.channels,
            handle.get_position_ms(info.sample_rate, info.channels),
        )?;
        (engine, update)
    };
    let Some(update) = update else {
        return Ok(());
    };
    let runtime = handle.clone();
    let transition = update.transition.clone();
    let gapless = update.gapless;
    let accepted = tokio::task::spawn_blocking(move || {
        runtime.update_prepared_transition(transition, gapless)
    })
    .await?;
    if accepted {
        player::persist_prepared_dj_transition_update(&engine, &update)?;
        let _ = state
            .read()
            .await
            .event_tx
            .send(AppEvent::PlaybackStateChanged);
    }
    Ok(())
}

fn spawn_drop_preview_scheduler(
    state: SharedState,
    handle: playback_runtime::PlaybackRuntimeHandle,
    lookahead: player::DjLookaheadStart,
    playback_generation: u64,
) {
    tokio::spawn(async move {
        if let Err(error) =
            schedule_drop_preview_for_pair(state, handle, lookahead, playback_generation).await
        {
            warn!("Drop preview scheduling skipped: {error:?}");
        }
    });
}

fn drop_preview_pair_is_current(
    state: &crate::AppState,
    handle: &playback_runtime::PlaybackRuntimeHandle,
    lookahead: &player::DjLookaheadStart,
    playback_generation: u64,
) -> bool {
    state
        .playback_runtime
        .as_ref()
        .is_some_and(|runtime| runtime.handle.is_same_runtime(handle))
        && current_playback_generation(state) == playback_generation
        && state
            .playback_runtime_info
            .as_ref()
            .and_then(|info| info.active_track_id)
            == lookahead
                .current
                .as_ref()
                .and_then(|media| media.track_id())
        && active_dj_lookahead_start_for_state(state).is_some_and(|pair| {
            pair.current == lookahead.current
                && pair.next == lookahead.next
                && pair.current_queue_item_id == lookahead.current_queue_item_id
                && pair.next_queue_item_id == lookahead.next_queue_item_id
                && pair.queue_generation == lookahead.queue_generation
        })
}

async fn schedule_drop_preview_for_pair(
    state: SharedState,
    handle: playback_runtime::PlaybackRuntimeHandle,
    lookahead: player::DjLookaheadStart,
    playback_generation: u64,
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
    let (next, runtime_info, user_quality) = {
        let state_guard = state.read().await;
        if !state_guard.db.with_conn(queries::is_dj_engine_enabled)? {
            return Ok(());
        }
        if !drop_preview_pair_is_current(&state_guard, &handle, &lookahead, playback_generation) {
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
    if !claim_drop_preview_arm(
        Arc::as_ptr(&state) as usize,
        current_track_id,
        next_track_id,
        lookahead.queue_generation,
        playback_generation,
    ) {
        return Ok(());
    }
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
        .with_generation(playback_generation)
        .with_dj_media_ref(next_ref.clone())
        .with_prepared_transition(player::PreparedTransitionProgram {
            program,
            transition_event_id: None,
            fire_ahead_ms: 0,
            queue_generation: lookahead.queue_generation,
            current_queue_item_id: lookahead.current_queue_item_id,
            next_queue_item_id: lookahead.next_queue_item_id,
            anchor_start_ms: Some(plan.planned_fire_ms),
        });
    job.gapless = crate::playback::gapless::GaplessPlan::disabled();

    {
        let state_guard = state.read().await;
        if !state_guard.db.with_conn(queries::is_dj_engine_enabled)? {
            return Ok(());
        }
        if !drop_preview_pair_is_current(&state_guard, &handle, &lookahead, playback_generation) {
            return Ok(());
        }
        let info = state_guard
            .playback_runtime_info
            .as_ref()
            .context("playback runtime info missing")?;
        if info.sample_rate != sample_rate || info.channels != channels {
            return Ok(());
        }
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
        playback_generation,
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

fn claim_drop_preview_arm(
    state_id: usize,
    current_track_id: i64,
    next_track_id: i64,
    queue_generation: u64,
    playback_generation: u64,
) -> bool {
    let attempts = DROP_PREVIEW_ARM_ATTEMPTS.get_or_init(|| Mutex::new(HashMap::new()));
    let now = Instant::now();
    let mut attempts = attempts.lock().unwrap_or_else(|error| error.into_inner());
    claim_drop_preview_arm_at(
        &mut attempts,
        state_id,
        current_track_id,
        next_track_id,
        queue_generation,
        playback_generation,
        now,
    )
}

fn claim_drop_preview_arm_at(
    attempts: &mut HashMap<DropPreviewArmKey, Instant>,
    state_id: usize,
    current_track_id: i64,
    next_track_id: i64,
    queue_generation: u64,
    playback_generation: u64,
    now: Instant,
) -> bool {
    let retry_after = Duration::from_secs(DROP_PREVIEW_ARM_RETRY_SECS);
    attempts.retain(|_, last_attempt| now.duration_since(*last_attempt) < retry_after);
    let key = (
        state_id,
        current_track_id,
        next_track_id,
        queue_generation,
        playback_generation,
    );
    if let Some(last_attempt) = attempts.get(&key)
        && now.duration_since(*last_attempt) < retry_after
    {
        return false;
    }
    attempts.insert(key, now);
    true
}

pub(crate) fn refresh_dj_after_queue_change(
    state: SharedState,
    context: &'static str,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    // Preparation can remove an unavailable next row and request a new pair.
    // Box this scheduling boundary so that bounded retry has no recursive
    // opaque-future type through the spawned preparation task.
    Box::pin(async move {
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
    })
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
    normalized_genres: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct PlaybackTrackRequest {
    track_id: i64,
}

#[derive(Debug, Deserialize)]
pub struct QueueReplaceRequest {
    /// Ordered library and external rows. Every replacement producer uses this
    /// shape so the queue never changes representation by source.
    items: Vec<MixedQueueItemRequest>,
    #[serde(default)]
    shuffle_mode: Option<String>,
    #[serde(default)]
    start_playback: bool,
}

/// Body of POST /api/playback/queue/play-item - jump to a queue row by id.
#[derive(Debug, Deserialize)]
pub struct PlayQueueItemRequest {
    queue_item_id: i64,
}

#[derive(Debug, Deserialize)]
pub struct MixedQueueItemRequest {
    #[serde(default)]
    track_id: Option<i64>,
    #[serde(default)]
    tidal_id: Option<i64>,
    #[serde(default)]
    artist: Option<String>,
    #[serde(default)]
    title: Option<String>,
    // Display metadata persisted on the pending row so the queue renders
    // artwork/album/duration before the resolver imports a library track.
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
    /// Optional queue provenance, primarily used by radio rows.
    #[serde(default)]
    reason: Option<String>,
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

    let tidal_id = parse_provider_track_id(&payload.provider_track_id)?;
    let candidate = crate::server::radio_pipeline::OrderedQueueCandidate {
        track_id: None,
        tidal_id: Some(tidal_id),
        artist: payload
            .artist_name
            .clone()
            .unwrap_or_else(|| "Unknown Artist".to_string()),
        title: payload.title.clone(),
        album_title: payload.album_title.clone(),
        artwork_url: payload.artwork_url.clone(),
        duration_ms: payload.duration_ms,
        artist_tidal_id: None,
        album_tidal_id: None,
        reason: Some("discovery".to_string()),
    };
    let db = {
        let state_guard = state.read().await;
        state_guard.db.clone()
    };
    let build = db
        .with_conn(|conn| {
            Ok(
                crate::server::radio_pipeline::replace_queue_with_ordered_candidates(
                    conn,
                    &[candidate],
                )?,
            )
        })
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to queue discovery track" })),
            )
        })?;
    spawn_pending_resolvers_for_queue_items(
        &state,
        &db,
        build.pending_item_ids,
        "play_discovery_track",
    )
    .await;
    let snapshot = start_first_radio_queue_item(&state).await?;
    record_transition_if_changed(&state, previous_track_id, &snapshot, "discovery", false).await;

    let state_guard = state.read().await;
    if let Some(track) = snapshot.state.current_track.as_ref() {
        let _ = state_guard
            .event_tx
            .send(AppEvent::TrackChanged { track_id: track.id });
    }
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
    drop(state_guard);
    Ok(Json(json!({
        "state": snapshot.state,
        "queue": snapshot.queue,
        "queue_revision": snapshot.queue_revision
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
    let limit = payload.limit.unwrap_or(20).clamp(1, 50);
    let exclude_ids = payload.exclude_ids.unwrap_or_default();

    let state = state.read().await;

    let seed_track_id: i64 = if let Some(id) = payload.seed_track_id.filter(|&id| id > 0) {
        id
    } else if let Some(tidal_id) = payload.seed_tidal_id {
        state
            .db
            .with_conn(|conn| crate::db::catalogue::track_id(conn, tidal_id))
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
    let mut rng = rand::rng();

    let mut scored: Vec<_> = similar
        .into_iter()
        .map(|track| {
            // Add noise proportional to creativity
            let noise = rng.random_range(0.0..=temperature);
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

    let (tokens, tidal_session) = {
        let persisted = load_persisted_tidal_tokens(&state)
            .await
            .map_err(internal)?;
        let s = state.read().await;
        (s.tidal.tokens().or(persisted), s.tidal.clone())
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

    let tidal_client = TidalClient::for_session(tidal_session.clone(), &tokens.country_code)
        .with_metadata_store(state.read().await.db.clone());
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

    let (sportify_client, cache_cfg, resolve_cfg, db, tokens_in_state, tidal_session) = {
        let s = state.read().await;
        (
            s.sportify_client.clone(),
            s.sportify_cache_config,
            s.sportify_resolve_config,
            s.db.clone(),
            s.tidal.tokens(),
            s.tidal.clone(),
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

    let client = TidalClient::for_session(tidal_session.clone(), &tokens.country_code)
        .with_metadata_store(state.read().await.db.clone());
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

    let (cache_cfg, resolve_cfg, db, tidal_tokens_in_state, tidal_session) = {
        let s = state.read().await;
        (
            s.sportify_cache_config,
            s.sportify_resolve_config,
            s.db.clone(),
            s.tidal.tokens(),
            s.tidal.clone(),
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
    let client = TidalClient::for_session(tidal_session.clone(), &tokens.country_code)
        .with_metadata_store(state.read().await.db.clone());

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

    let (cache_cfg, resolve_cfg, db, tidal_tokens_in_state, session) = {
        let s = state.read().await;
        (
            s.sportify_cache_config,
            s.sportify_resolve_config,
            s.db.clone(),
            s.tidal.tokens(),
            s.tidal.clone(),
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
        let client =
            TidalClient::for_session(session, &tokens.country_code).with_metadata_store(db.clone());
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

    let mut queue = crate::services::radio::orchestrate_song(
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
    add_tidal_mix_fallback(
        &state,
        &db,
        TidalMixSeed::Track(payload.seed_track_id),
        &mut queue.tracks,
        limit,
    )
    .await;

    let (first_playable, pending_count) = build_radio_queue_and_spawn_resolvers(
        &state,
        &db,
        Some(payload.seed_track_id),
        queue.tracks.clone(),
        "radio_song",
    )
    .await?;
    remember_radio_seed(
        &db,
        crate::server::radio_continuation::RadioSeedKind::Track,
        payload.seed_track_id,
        blend,
    );
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

/// Seed for the TIDAL mix fallback, by local id.
#[derive(Clone, Copy)]
pub(crate) enum TidalMixSeed {
    Track(i64),
    Artist(i64),
}

/// A radio with fewer than `TIDAL_MIX_FALLBACK_MIN_PICKS` picks has a seed with
/// too little evidence of its own (never played, no genres or audio analysis,
/// no Last.fm match). Fill it from TIDAL's track or artist mix for the seed.
/// Best effort: no TIDAL session or no mix leaves the radio as it was.
pub(crate) async fn add_tidal_mix_fallback(
    state: &SharedState,
    db: &crate::db::Database,
    seed: TidalMixSeed,
    tracks: &mut Vec<crate::services::radio::RadioCandidate>,
    limit: usize,
) {
    use crate::services::radio::{
        TIDAL_MIX_FALLBACK_MIN_PICKS, tidal_mix_candidates, tidal_mix_id,
    };
    if tracks.len() >= TIDAL_MIX_FALLBACK_MIN_PICKS {
        return;
    }
    let (sql, local_id) = match seed {
        TidalMixSeed::Track(id) => ("SELECT tidal_id FROM tracks WHERE id = ?1", id),
        TidalMixSeed::Artist(id) => ("SELECT tidal_id FROM artists WHERE id = ?1", id),
    };
    let tidal_id = db
        .with_conn(|conn| {
            Ok(conn
                .query_row(sql, [local_id], |row| row.get::<_, Option<i64>>(0))
                .optional()?
                .flatten())
        })
        .ok()
        .flatten()
        .filter(|id| *id > 0);
    let Some(tidal_id) = tidal_id else { return };
    let Some(persisted) = load_persisted_tidal_tokens(state).await.ok().flatten() else {
        return;
    };
    let (tokens, tidal_session) = {
        let s = state.read().await;
        (s.tidal.tokens().unwrap_or(persisted), s.tidal.clone())
    };
    let client = TidalClient::for_session(tidal_session.clone(), &tokens.country_code)
        .with_metadata_store(db.clone());
    let mix_id = match seed {
        TidalMixSeed::Track(_) => client
            .get_track(tidal_id)
            .await
            .ok()
            .and_then(|track| tidal_mix_id(&track.extra, "TRACK_MIX")),
        TidalMixSeed::Artist(_) => client
            .get_artist(tidal_id)
            .await
            .ok()
            .and_then(|artist| tidal_mix_id(&artist.extra, "ARTIST_MIX")),
    };
    let Some(mix_id) = mix_id else { return };
    let mix = match client.get_mix_tracks(&mix_id).await {
        Ok(mix) => mix,
        Err(error) => {
            tracing::warn!(%error, mix_id, "radio: TIDAL mix fallback failed");
            return;
        }
    };
    let seed_tidal_id = matches!(seed, TidalMixSeed::Track(_)).then_some(tidal_id);
    let wanted = limit.saturating_sub(tracks.len());
    let existing = tracks.clone();
    let added = db
        .with_conn(move |conn| {
            Ok(tidal_mix_candidates(
                conn,
                mix,
                seed_tidal_id,
                &existing,
                wanted,
            ))
        })
        .unwrap_or_default();
    tracing::info!(
        before = tracks.len(),
        added = added.len(),
        "radio: seed had too little evidence; filled from TIDAL mix"
    );
    tracks.extend(added);
}

/// Remember which radio built the queue so topping it up continues that radio
/// (server::radio_continuation). Best effort: a failure only means automix
/// carries the queue on instead.
fn remember_radio_seed(
    db: &crate::db::Database,
    kind: crate::server::radio_continuation::RadioSeedKind,
    id: i64,
    blend: crate::services::radio::RadioBlend,
) {
    let seed = crate::server::radio_continuation::RadioSeed { kind, id, blend };
    if let Err(error) = db.with_conn(|conn| {
        Ok(crate::server::radio_continuation::remember_seed(
            conn, seed,
        )?)
    }) {
        tracing::warn!(%error, "radio: could not remember the radio seed");
    }
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
        if let Some(t) = s.tidal.tokens() {
            Some(t)
        } else {
            drop(s);
            load_persisted_tidal_tokens(state).await.ok().flatten()
        }
    };

    if let Some(tokens) = tokens_opt {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(RESOLVER_POOL_SIZE));
        let event_tx = state.read().await.event_tx.clone();
        for item_id in pending_item_ids {
            let sem = semaphore.clone();
            let db_bg = db.clone();
            let tok = tokens.clone();
            let tx = event_tx.clone();
            let state_bg = state.clone();
            tokio::spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                if resolve_pending_row(state_bg.clone(), db_bg, tok, item_id, tx).await {
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
    transport_command::start_queue_from_beginning(state)
        .await
        .map_err(|error| command_error_response(state, error))
}

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

    let mut radio_queue = crate::services::radio::orchestrate_song(
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
    add_tidal_mix_fallback(
        &state,
        &db,
        TidalMixSeed::Track(payload.seed_track_id),
        &mut radio_queue.tracks,
        limit,
    )
    .await;

    // Build queue atomically and collect pending row IDs for background tasks.
    let seed_track_id = payload.seed_track_id;
    let build = db
        .with_conn(move |conn| {
            Ok(
                crate::server::radio_pipeline::build_radio_queue_from_candidates(
                    conn,
                    seed_track_id,
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
    remember_radio_seed(
        &db,
        crate::server::radio_continuation::RadioSeedKind::Track,
        seed_track_id,
        blend,
    );
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
            if let Some(t) = s.tidal.tokens() {
                Some(t)
            } else {
                drop(s);
                load_persisted_tidal_tokens(&state).await.ok().flatten()
            }
        };

        if let Some(tokens) = tokens_opt {
            let semaphore = Arc::new(tokio::sync::Semaphore::new(RESOLVER_POOL_SIZE));
            let event_tx = state.read().await.event_tx.clone();
            for item_id in pending_item_ids {
                let sem = semaphore.clone();
                let db_bg = db.clone();
                let tok = tokens.clone();
                let tx = event_tx.clone();
                let state_bg = state.clone();
                tokio::spawn(async move {
                    let _permit = sem.acquire_owned().await.ok();
                    if resolve_pending_row(state_bg.clone(), db_bg, tok, item_id, tx).await {
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
        "queue": snapshot.queue,
        "queue_revision": snapshot.queue_revision
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
    remember_radio_seed(
        &db,
        crate::server::radio_continuation::RadioSeedKind::Album,
        payload.seed_album_id,
        blend,
    );
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

    let mut queue = crate::services::radio::orchestrate_artist(
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
    add_tidal_mix_fallback(
        &state,
        &db,
        TidalMixSeed::Artist(payload.seed_artist_id),
        &mut queue.tracks,
        limit,
    )
    .await;

    let (first_playable, pending_count) = build_radio_queue_and_spawn_resolvers(
        &state,
        &db,
        None,
        queue.tracks.clone(),
        "radio_artist",
    )
    .await?;
    remember_radio_seed(
        &db,
        crate::server::radio_continuation::RadioSeedKind::Artist,
        payload.seed_artist_id,
        blend,
    );
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
    let limit = query.limit.unwrap_or(50).clamp(1, 200);

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
    let tokens = state_guard.tidal.tokens().ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "status": "not_connected",
                "message": "Connect TIDAL in Settings before searching for new music.",
            })),
        )
    })?;

    Ok(TidalDiscoveryProvider::new(
        state_guard.tidal.clone(),
        tokens.user_id,
        tokens.country_code,
        state_guard.db.clone(),
    ))
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

pub(crate) fn active_dj_pair_for_state_and_conn(
    _state: &crate::AppState,
    conn: &rusqlite::Connection,
) -> anyhow::Result<crate::playback::dj_lookahead::DjLookaheadPair> {
    crate::playback::dj_lookahead::load_dj_lookahead_pair(conn)
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

    // Update local DB immediately - Tidal sync happens in the background.
    // An intentional re-like moves the song to the top once; delivery retries retain that action time.
    {
        let state = state.read().await;
        state
            .db
            .with_conn(|conn| {
                let tx = conn.unchecked_transaction()?;
                crate::db::catalogue_favorites::request(
                    &tx,
                    "track",
                    payload.track_id,
                    payload.favorite,
                )?;
                tx.commit()?;
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

    crate::services::tidal::favorites::run_if_idle(state.clone()).await;

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

    {
        let s = state.read().await;
        s.db.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            crate::db::catalogue_favorites::request(
                &tx,
                "album",
                payload.album_id,
                payload.favorite,
            )?;
            tx.commit()?;
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

    crate::services::tidal::favorites::run_if_idle(state.clone()).await;

    Ok(Json(json!({
        "album_id": payload.album_id,
        "tidal_id": tidal_id,
        "favorite": payload.favorite,
        "updated": state_changed
    })))
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

async fn pause_playback(State(state): State<SharedState>) -> Result<Json<Value>, StatusCode> {
    // Deliberately does NOT bump the playback generation. The generation
    // identifies WHICH playback job is current; a transport toggle does not
    // start one, so bumping here orphaned the engine that is still loaded:
    // it keeps the generation it was created with, and every generation-guarded
    // path then rejected its events for the rest of the track. That silently
    // broke the end-of-track queue advance (the track played to its final
    // sample and froze), prepare-next/gapless, the Started event that sets
    // `audio_active`, and track-error recovery, until the user hit Next.
    //
    // The race this was reaching for -- an in-flight resolve starting audio
    // after the user paused -- is already handled by the play/switch paths
    // through `with_start_paused(!transport_intent_is_playing(..))`, which
    // reads the `is_playing` intent that `player::pause` writes below.
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

    drop(state_guard);
    let live_position_ms = current_live_position_ms(&state).await;
    let snapshot =
        overlay_snapshot_with_external_track_and_position(&state, snapshot, live_position_ms).await;
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
    // No generation bump, for the same reason as `pause_playback`: resuming
    // does not start a new playback job, and bumping here left the engine that
    // is about to keep playing stranded on a stale generation.
    let (runtime_handle, runtime_active_track_id, persisted_track_id) = {
        let state_guard = state.read().await;
        let runtime_handle = state_guard
            .playback_runtime
            .as_ref()
            .map(|runtime| runtime.handle.clone())
            .filter(playback_runtime::PlaybackRuntimeHandle::is_healthy);
        let runtime_active_track_id = state_guard
            .playback_runtime_info
            .as_ref()
            .and_then(|info| info.active_track_id);
        let persisted_track_id = state_guard
            .db
            .with_conn(player::current_track_id)
            .unwrap_or(None);
        (runtime_handle, runtime_active_track_id, persisted_track_id)
    };
    let runtime_needs_rebuild = if runtime_active_track_id != persisted_track_id {
        true
    } else {
        match runtime_handle {
            Some(runtime_handle) => match runtime_handle.resume() {
                Ok(()) => false,
                Err(error) => {
                    tracing::warn!(
                        target: "noor.playback.recovery",
                        event = "resume_dead_runtime",
                        error = %error,
                        "resume found a closed runtime command channel; rebuilding"
                    );
                    true
                }
            },
            None => true,
        }
    };

    let mut snapshot = {
        let state = state.read().await;
        state
            .db
            .with_conn(player::resume)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    if runtime_needs_rebuild {
        let Some(track) = snapshot.state.current_track.clone() else {
            snapshot = {
                let state_guard = state.read().await;
                state_guard
                    .db
                    .with_conn(player::pause)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            };
            let state_guard = state.read().await;
            let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
            let snapshot = overlay_snapshot_with_external_track(&state, snapshot).await;
            return Ok(Json(json!({ "state": snapshot.state })));
        };

        if let Err((status, body)) = ensure_playback_runtime_for_track(&state, &track).await {
            let message = body
                .0
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Playback runtime could not restart.")
                .to_string();
            let state_guard = state.read().await;
            let _ = state_guard.db.with_conn(player::pause);
            let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
            drop(state_guard);
            report_playback_failure(&state, &message);
            return Err(status);
        }
        let generation = {
            let state_guard = state.read().await;
            current_playback_generation(&state_guard)
        };
        if let Err(error) = switch_runtime_to_snapshot_current(&state, &snapshot, generation).await
        {
            let message = format!("Playback runtime could not recover on resume: {error}");
            let state_guard = state.read().await;
            let _ = state_guard.db.with_conn(player::pause);
            let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);
            drop(state_guard);
            report_playback_failure(&state, &message);
            return Err(StatusCode::BAD_GATEWAY);
        }
        tracing::info!(
            target: "noor.playback.recovery",
            event = "runtime_rebuilt_on_resume",
            track_id = track.id,
            generation,
            "restored the current queue row in the playback runtime"
        );
    }

    resume_session_after_snapshot(&state, &snapshot).await;

    let state_guard = state.read().await;
    let _ = state_guard.event_tx.send(AppEvent::PlaybackStateChanged);

    drop(state_guard);
    let live_position_ms = current_live_position_ms(&state).await;
    let snapshot =
        overlay_snapshot_with_external_track_and_position(&state, snapshot, live_position_ms).await;
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

pub(crate) fn import_metadata_from_search_track(
    t: TidalSearchTrack,
) -> tidal_import::ImportTrackMetadata {
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

pub(crate) fn import_metadata_from_tidal_track(t: TidalTrack) -> tidal_import::ImportTrackMetadata {
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
        // Artist photos come in 160/320/480/750; 640 is a cover size.
        artist_picture: TidalClient::get_artwork_url(&t.artist.picture, 750),
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
pub(crate) const TIDAL_RESOLVE_POOL: i32 = 10;

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
pub(crate) fn select_best_tidal_match(
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

    // A fired event remains part of listening history, but an accepted seek
    // means its rendered overlap is no longer the live visual association.
    if matches!(
        outcome,
        playback_runtime::SeekToOutcome::Dispatched
            | playback_runtime::SeekToOutcome::DispatchedCrossfadeSuppressed
    ) && let Some(session) = state.write().await.active_listen_session.as_mut()
    {
        session.transition_visual_valid = false;
    }

    let snapshot = build_live_playback_snapshot(&state)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
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
                let queue_revision = player::queue_revision(conn);
                let _ = state_guard.event_tx.send(AppEvent::QueueUpdated);
                Ok(Json(json!({
                    "queue": queue,
                    "queue_revision": queue_revision
                })))
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
            local_track_id: Some(positive(payload.track_id, "track_id")?),
            ..Default::default()
        }),
        // TIDAL rows keep their display metadata so the pending queue row
        // renders artwork/album/duration immediately, before resolution.
        QueueExternalKind::Tidal => Ok(queue::ExternalTrackInsert {
            artist: non_empty(&payload.artist, "artist")?,
            title: non_empty(&payload.title, "title")?,
            source,
            tidal_id_hint: Some(positive(payload.tidal_id, "tidal_id")?),
            album_title: payload.album_title.as_deref(),
            artwork_url: payload.artwork_url.as_deref(),
            duration_ms: payload.duration_ms.filter(|ms| *ms > 0),
            artist_tidal_id: payload.artist_tidal_id.filter(|id| *id > 0),
            album_tidal_id: payload.album_tidal_id.filter(|id| *id > 0),
            ..Default::default()
        }),
        QueueExternalKind::External => Ok(queue::ExternalTrackInsert {
            artist: non_empty(&payload.artist, "artist")?,
            title: non_empty(&payload.title, "title")?,
            source,
            ..Default::default()
        }),
    }
}

/// Return the active persisted queue position, preferring the queue-item anchor.
fn current_queue_position(conn: &rusqlite::Connection) -> anyhow::Result<Option<i32>> {
    let (current_queue_item_id, current_track_id): (Option<i64>, Option<i64>) = conn.query_row(
        "SELECT current_queue_item_id, current_track_id FROM playback_state WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    if let Some(queue_item_id) = current_queue_item_id
        && queue_item_matches_current_track(conn, queue_item_id, current_track_id)?
        && let Some(position) = queue_item_position(conn, queue_item_id)?
    {
        return Ok(Some(position));
    }

    if let Some(track_id) = current_track_id
        && let Some((queue_item_id, position)) = first_queue_item_for_track(conn, track_id)?
    {
        conn.execute(
            "UPDATE playback_state SET current_queue_item_id = ?1 WHERE id = 1",
            params![queue_item_id],
        )?;
        return Ok(Some(position));
    }

    if current_queue_item_id.is_some() {
        conn.execute(
            "UPDATE playback_state SET current_queue_item_id = NULL WHERE id = 1",
            [],
        )?;
    }

    Ok(None)
}

/// Look up a persisted queue row position by its stable item id.
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
    crate::server::radio_continuation::forget_seed(conn);
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

async fn queue_append(
    State(state): State<SharedState>,
    Json(payload): Json<QueueExternalRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let insert = queue_external_insert(&payload, "user_queue")
        .map_err(|message| (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))))?;

    let (queue, inserted, queue_revision) = {
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
                let queue_revision = player::queue_revision(conn);
                let _ = event_tx.send(AppEvent::QueueUpdated);
                Ok((queue, Some(inserted), queue_revision))
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

    Ok(Json(json!({
        "queue": queue,
        "queue_revision": queue_revision
    })))
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

    let (queue, inserted, queue_revision) = {
        let state_guard = state.read().await;
        state_guard
            .user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let event_tx = state_guard.event_tx.clone();
        state_guard
            .db
            .with_conn(|conn| {
                let inserted = queue::append_external_tracks(conn, &inserts)?;
                let queue = queue::load_queue(conn)?;
                let queue_revision = player::queue_revision(conn);
                let _ = event_tx.send(AppEvent::QueueUpdated);
                Ok((queue, inserted, queue_revision))
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

    Ok(Json(json!({
        "queue": queue,
        "queue_revision": queue_revision
    })))
}

/// Insert after the active persisted queue row. With no active row, append.
fn play_next_after_position(current_pos: Option<i32>) -> Option<i32> {
    current_pos
}

async fn queue_play_next(
    State(state): State<SharedState>,
    Json(payload): Json<QueueExternalRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let insert = queue_external_insert(&payload, "user_play_next")
        .map_err(|message| (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))))?;

    let (queue, inserted, queue_revision) = {
        let state_guard = state.read().await;
        state_guard
            .user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let event_tx = state_guard.event_tx.clone();
        state_guard
            .db
            .with_conn(|conn| {
                let after = play_next_after_position(current_queue_position(conn)?);
                let inserted = match after {
                    Some(after) => queue::insert_external_track_after(conn, &insert, after)?,
                    None => queue::append_external_track(conn, &insert)?,
                };
                let queue = queue::load_queue(conn)?;
                let queue_revision = player::queue_revision(conn);
                let _ = event_tx.send(AppEvent::QueueUpdated);
                Ok((queue, Some(inserted), queue_revision))
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

    Ok(Json(json!({
        "queue": queue,
        "queue_revision": queue_revision
    })))
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

    let (queue, inserted, queue_revision) = {
        let state_guard = state.read().await;
        state_guard
            .user_cleared_at
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let event_tx = state_guard.event_tx.clone();
        state_guard
            .db
            .with_conn(|conn| {
                let after = play_next_after_position(current_queue_position(conn)?);
                let inserted = match after {
                    Some(after) => queue::insert_external_tracks_after(conn, &inserts, after)?,
                    None => queue::append_external_tracks(conn, &inserts)?,
                };
                let queue = queue::load_queue(conn)?;
                let queue_revision = player::queue_revision(conn);
                let _ = event_tx.send(AppEvent::QueueUpdated);
                Ok((queue, inserted, queue_revision))
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

    Ok(Json(json!({
        "queue": queue,
        "queue_revision": queue_revision
    })))
}

async fn replace_playback_queue(
    State(state): State<SharedState>,
    Json(payload): Json<QueueReplaceRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use crate::server::radio_pipeline::OrderedQueueCandidate;

    let candidates: Vec<OrderedQueueCandidate> = payload
        .items
        .iter()
        .filter(|item| {
            item.track_id.is_some_and(|id| id > 0)
                || item.tidal_id.is_some_and(|id| id > 0)
                || (item.artist.as_deref().is_some_and(|s| !s.trim().is_empty())
                    && item.title.as_deref().is_some_and(|s| !s.trim().is_empty()))
        })
        .map(|item| OrderedQueueCandidate {
            track_id: item.track_id.filter(|id| *id > 0),
            tidal_id: item.tidal_id.filter(|id| *id > 0),
            artist: item.artist.clone().unwrap_or_default(),
            title: item.title.clone().unwrap_or_default(),
            album_title: item.album_title.clone(),
            artwork_url: item.artwork_url.clone(),
            duration_ms: item.duration_ms.filter(|ms| *ms > 0),
            artist_tidal_id: item.artist_tidal_id.filter(|id| *id > 0),
            album_tidal_id: item.album_tidal_id.filter(|id| *id > 0),
            reason: item.reason.clone(),
        })
        .collect();
    if candidates.is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "no queueable tracks in request" })),
        ));
    }
    let queued_count = candidates.len();
    let shuffle_mode = payload
        .shuffle_mode
        .as_deref()
        .map(queue::ShuffleMode::parse)
        .unwrap_or(queue::ShuffleMode::Off);

    let db = {
        let state_guard = state.read().await;
        state_guard.db.clone()
    };
    let (build, shuffle_debug) = db
        .with_conn(move |conn| {
            let build = crate::server::radio_pipeline::replace_queue_with_ordered_candidates(
                conn,
                &candidates,
            )?;
            let mut shuffle_debug = None;
            if shuffle_mode != queue::ShuffleMode::Off {
                let seed = crate::playback::shuffle::generate_shuffle_seed();
                let result = crate::playback::queue::apply_shuffle_with_seed(
                    conn,
                    shuffle_mode,
                    None,
                    seed,
                    "replace_playback_queue",
                )?;
                shuffle_debug = result.debug;
            }
            Ok((build, shuffle_debug))
        })
        .map_err(|e| {
            tracing::error!("replace_playback_queue: queue build failed: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to build queue" })),
            )
        })?;

    let pending_count = build.pending_item_ids.len();
    spawn_pending_resolvers_for_queue_items(
        &state,
        &db,
        build.pending_item_ids,
        "replace_playback_queue",
    )
    .await;
    {
        let s = state.read().await;
        let _ = s.event_tx.send(AppEvent::QueueUpdated);
    }

    let snapshot = if payload.start_playback {
        start_first_radio_queue_item(&state).await?
    } else {
        let state_guard = state.read().await;
        state_guard
            .db
            .with_conn(player::load_snapshot)
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "failed to load queue snapshot" })),
                )
            })?
    };
    refresh_dj_after_queue_change(state, "replace_playback_queue").await;
    Ok(Json(json!({
        "queued_count": queued_count,
        "pending_count": pending_count,
        "shuffle_debug": shuffle_debug,
        "state": snapshot.state,
        "queue": snapshot.queue,
        "queue_revision": snapshot.queue_revision,
    })))
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
            "playback_state": snapshot.state,
            "queue_revision": snapshot.queue_revision
        })))
    } else {
        Ok(Json(json!({
            "queue": snapshot.queue,
            "queue_revision": snapshot.queue_revision
        })))
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
                        "playback_state": snapshot.state,
                        "queue_revision": snapshot.queue_revision
                    })),
                    None => Json(json!({
                        "queue": queue,
                        "queue_revision": player::queue_revision(conn)
                    })),
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
                    "queue_revision": snapshot.queue_revision,
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
            if include_tidal_only || track_source.as_str() != "tidal_stream" {
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
    let db = {
        let state = state.read().await;
        state.db.clone()
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

    let response = db
        .with_conn(|conn| {
            let playlist = queries::create_playlist(conn, &name, None)?;
            let added = queries::add_tracks_to_playlist(conn, playlist.id, &track_ids)?;
            let playlist = queries::get_playlist(conn, playlist.id)?
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
        })?;
    playlist_routes::notify_playlists_changed(&state).await;
    Ok(response)
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
        let s = state.read().await;
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
    let tokens = match load_persisted_tidal_tokens(&state).await {
        Ok(tokens) => tokens,
        Err(error) => {
            tracing::warn!(
                "Failed to rehydrate persisted TIDAL tokens during login poll: {}",
                error
            );
            None
        }
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
    let session = state.read().await.tidal.clone();
    session.reload_from_store()
}

/// Get TIDAL backoff gate status.
async fn get_tidal_backoff_status() -> impl axum::response::IntoResponse {
    let state = crate::services::tidal::backoff::global().state();
    axum::Json(state)
}

/// Get TIDAL connection status.
async fn tidal_status(State(state): State<SharedState>) -> Json<Value> {
    let session = state.read().await.tidal.clone();
    let tokens = match session.reload_from_store() {
        Ok(tokens) => tokens,
        Err(error) => {
            tracing::warn!("Failed to rehydrate persisted TIDAL tokens: {}", error);
            None
        }
    };

    if let Some(mut tokens) = tokens {
        if !tokens.is_pkce() {
            tidal_auth::warn_if_fallback_client_credentials();
        }
        let mut expired = session.needs_reconnect()
            || tidal_tokens_locally_expired(&state, &tokens)
                .await
                .unwrap_or_else(|error| {
                    tracing::warn!("Failed to inspect persisted TIDAL token expiry: {error}");
                    false
                });
        if expired && !session.needs_reconnect() && !tokens.refresh_token.trim().is_empty() {
            match session.refresh_stale(&tokens.access_token).await {
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

/// Canonical fetch size for TIDAL search, so callers asking for fewer results
/// share one cache row and one upstream call. Sized to the largest limit the
/// app actually requests (the remote's 12); a bigger request bypasses it and
/// caches on its own size.
const TIDAL_SEARCH_CACHE_BUCKET: i32 = 12;

fn normalize_tidal_search_limit(limit: Option<i32>) -> i32 {
    limit
        .unwrap_or(TIDAL_SEARCH_DEFAULT_LIMIT)
        .clamp(1, TIDAL_SEARCH_MAX_LIMIT)
}

fn tidal_search_flight_key(query: &str, fetch_limit: i32, offset: i32) -> String {
    format!("{}|{}|{}", query.trim().to_lowercase(), fetch_limit, offset)
}

fn tidal_search_flights() -> &'static crate::services::tidal::singleflight::KeyedSingleFlight<String>
{
    static FLIGHTS: OnceLock<crate::services::tidal::singleflight::KeyedSingleFlight<String>> =
        OnceLock::new();
    FLIGHTS.get_or_init(Default::default)
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
    Ok(s.tidal.tokens().or(persisted))
}

/// Every video list the listener opens teaches the discovery crawler.
async fn harvest_seen_videos(
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

async fn tidal_artist_profile(
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

async fn tidal_artist_core(
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

pub(crate) fn should_retry_exclusive_release(is_playing: bool, exclusive_mode: bool) -> bool {
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
    let session = s.tidal.clone();
    drop(s);
    session.logout().await
}

async fn persist_tidal_tokens(
    state: &SharedState,
    tokens: &tidal_auth::TidalTokens,
) -> anyhow::Result<()> {
    let session = state.read().await.tidal.clone();
    session.login(tokens.clone()).await
}

/// Upsert a TIDAL track (and its artist) and return the local `tracks.id`.
/// The id is looked up here anyway to attach source genres, so callers that
/// need it should use the return value rather than issuing a second SELECT.
pub(super) fn insert_tidal_track(
    conn: &rusqlite::Connection,
    track: &crate::services::tidal::client::TidalTrack,
    is_favorite: bool,
    is_library: bool,
    favorite_created: Option<&str>,
) -> anyhow::Result<Option<i64>> {
    let catalogue = crate::db::catalogue::enabled(conn)?;
    if catalogue {
        let known = crate::db::catalogue::track_id(conn, track.id)?;
        let matched = if known.is_some() {
            known
        } else {
            let incoming = crate::library::duplicates::IncomingTrack {
                tidal_id: track.id,
                title: &track.title,
                artist_name: &track.artist.name,
                isrc: track.isrc.as_deref(),
                duration_ms: track.duration * 1000,
                version: track.extra.get("version").and_then(|v| v.as_str()),
                explicit: track.extra.get("explicit").and_then(|v| v.as_bool()),
            };
            let candidates = crate::library::duplicates::fetch_import_candidates(
                conn,
                track.id,
                track.artist.id,
                track.isrc.as_deref(),
                incoming.duration_ms,
            )?;
            match crate::library::duplicates::decide_import(&incoming, &candidates) {
                crate::library::duplicates::ImportDecision::LinkAlias {
                    existing_track_id, ..
                } => Some(existing_track_id),
                _ => None,
            }
        };
        if let Some(id) = matched {
            crate::db::catalogue::record_track(conn, id, track, is_favorite, favorite_created)?;
            crate::db::catalogue::curate(conn, id, is_favorite, is_library, favorite_created)?;
            queries::replace_track_source_genres(
                conn,
                id,
                &infer_tidal_track_genres(track),
                "tidal",
                0.82,
            )?;
            return Ok(Some(id));
        }
    }
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
        // Curated write paths (liked tracks, playlist tracks) pass
        // is_library=1; discovery-enrichment fill from favorited albums
        // passes is_library=0 so it stays out of the Library grid and Genre
        // Galaxy while still feeding radio/similarity. The ON CONFLICT MAX
        // self-heals: a row first seen as background is promoted to library
        // when a curated write touches it, and is never demoted. See
        // MIGRATION_052.
        "INSERT INTO tracks (tidal_id, title, artist_id, album_id, disc_number, track_number, duration_ms, isrc, best_quality, best_source, fidelity_score, is_favorite, source, date_added, is_library)
         VALUES (?1, ?2, (SELECT id FROM artists WHERE tidal_id=?3), (SELECT id FROM albums WHERE tidal_id=?4), ?5, ?6, ?7, ?8, ?9, 'tidal', ?10, ?11, 'tidal', COALESCE(?12, datetime('now')), ?13)
         ON CONFLICT(tidal_id) DO UPDATE SET
            title=excluded.title, best_quality=excluded.best_quality,
            fidelity_score=MAX(tracks.fidelity_score, excluded.fidelity_score),
            is_favorite=MAX(tracks.is_favorite, excluded.is_favorite),
            is_library=MAX(tracks.is_library, excluded.is_library),
            date_added=CASE
                WHEN ?11 = 1 AND ?12 IS NOT NULL AND tracks.is_library=0 AND tracks.is_favorite=0 THEN excluded.date_added
                ELSE tracks.date_added
            END",
        rusqlite::params![
            track.id, track.title, track.artist.id, album_tidal_id,
            track.volume_number.unwrap_or(1), track.track_number,
            track.duration * 1000, track.isrc,
            quality, fidelity, is_favorite as i32, favorite_created,
            is_library as i32,
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
        if catalogue {
            crate::db::catalogue::record_track(
                conn,
                local_track_id,
                track,
                is_favorite,
                favorite_created,
            )?;
            crate::db::catalogue::curate(
                conn,
                local_track_id,
                is_favorite,
                is_library,
                favorite_created,
            )?;
        }
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

#[cfg(test)]
pub(super) fn apply_tidal_favorite_flags(
    conn: &rusqlite::Connection,
    table: &str,
    favorite_ids: &HashSet<i64>,
    prev_count: i64,
) -> anyhow::Result<()> {
    apply_tidal_favorite_flags_at(
        conn,
        table,
        favorite_ids,
        prev_count,
        &crate::db::catalogue_favorites::now(),
    )
}

pub(super) fn apply_tidal_favorite_flags_at(
    conn: &rusqlite::Connection,
    table: &str,
    favorite_ids: &HashSet<i64>,
    prev_count: i64,
    snapshot_started: &str,
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

    if crate::db::catalogue::enabled(conn)? {
        let tx = conn.unchecked_transaction()?;
        crate::db::catalogue::reconcile_favorites_at(
            &tx,
            favorite_ids,
            table == "albums",
            snapshot_started,
        )?;
        tx.commit()?;
        return Ok(());
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
pub(super) mod tests;

#[cfg(test)]
mod drop_preview_tests;
