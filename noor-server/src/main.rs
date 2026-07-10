mod db;
mod genre;
mod library;
mod metadata;
mod paths;
mod playback;
mod server;
mod services;
mod smart;
mod tags;

use anyhow::Result;
use rusqlite::OptionalExtension;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64};
use tokio::sync::{RwLock, broadcast};
use tracing::info;
#[cfg(not(feature = "spotify-public"))]
use tracing::warn;

#[derive(Clone)]
pub struct PlaybackRuntimeState {
    pub access_token: String,
    pub handle: playback::runtime::PlaybackRuntimeHandle,
}

#[derive(Debug, Clone)]
pub struct PlaybackRuntimeInfo {
    pub device_name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub active_track_id: Option<i64>,
    pub last_error: Option<String>,
    pub exclusive_engaged: bool,
    pub exclusive_transport_format: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StreamDisplayInfo {
    pub audio_quality: String,
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i32>,
}

/// One slot in the pending ephemeral TIDAL queue (e.g. rest of a TIDAL mix
/// after the first track started). Just metadata — stream URL is resolved
/// lazily when this slot is promoted to the active track, since TIDAL stream
/// URLs expire after ~30 min and a long mix could outlive an upfront resolve.
#[derive(Debug, Clone)]
pub struct PendingEphemeralTidalTrack {
    pub tidal_track_id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
    // TIDAL identity carried through so the synthetic now-playing track and the
    // Up Next rows can link to the artist/album pages.
    pub artist_tidal_id: Option<i64>,
    pub album_tidal_id: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct PreparedEphemeralTidalNext {
    pub tidal_track_id: i64,
    pub synthetic_track: db::models::Track,
    pub stream_display: StreamDisplayInfo,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NextPrebufferKey {
    pub current_track_id: i64,
    pub next_track_id: i64,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DropPreviewRuntimeState {
    pub track_id: i64,
    pub generation: u64,
    pub actual_fire_ms: i64,
}

/// Shared application state accessible by all modules
pub struct AppState {
    pub db: db::Database,
    pub event_tx: broadcast::Sender<AppEvent>,
    pub http_client: reqwest::Client,
    /// Long-lived TIDAL-tuned reqwest client, built once at boot.
    /// Per-request `TidalClient` instances reuse this via `with_http` to skip
    /// per-call TLS pool setup. Token + country_code are stitched in per-call.
    pub tidal_http_client: reqwest::Client,
    pub tidal_tokens: Option<services::tidal::auth::TidalTokens>,
    /// 6h TTL cache for the home Your Mixes shelf. TIDAL builds these on a
    /// daily cadence, so re-fetching on every Home remount was wasted work
    /// (and a visible skeleton flash). Cleared on app restart.
    pub tidal_mixes_cache:
        Arc<std::sync::Mutex<Option<(std::time::Instant, Vec<services::tidal::client::TidalMix>)>>>,
    /// 6h TTL cache for the home Personal Radio shelf. Same cadence as mixes.
    pub tidal_radio_stations_cache:
        Arc<std::sync::Mutex<Option<(std::time::Instant, Vec<services::tidal::client::TidalMix>)>>>,
    /// 2h TTL cache for /api/home/picks. The genre-variety query uses
    /// `ORDER BY RANDOM()` (a full table scan), so recomputing it on every home
    /// remount was wasted work. Cleared on app restart; staleness bounded by the TTL.
    pub home_picks_cache: Arc<std::sync::Mutex<Option<(std::time::Instant, serde_json::Value)>>>,
    /// 6h TTL cache for the TIDAL moods landing categories. The handler
    /// hydrates category thumbnails from multiple upstream pages, so keep the
    /// computed list in memory instead of repeating that fan-out per request.
    pub tidal_moods_cache: Arc<
        std::sync::Mutex<
            Option<(
                std::time::Instant,
                std::time::Duration,
                Vec<serde_json::Value>,
            )>,
        >,
    >,
    /// 6h TTL cache for parsed TIDAL pages such as mood drill-down pages.
    pub tidal_page_modules_cache: Arc<
        std::sync::Mutex<
            std::collections::HashMap<
                String,
                (
                    std::time::Instant,
                    Vec<services::tidal::client::TidalHomeModule>,
                ),
            >,
        >,
    >,
    /// 1h TTL cache for external TIDAL playlist track pages. Library state is
    /// enriched after reading from this cache so favorite badges stay fresh.
    pub tidal_playlist_tracks_cache: Arc<
        std::sync::Mutex<
            std::collections::HashMap<
                String,
                (std::time::Instant, Vec<services::tidal::client::TidalTrack>),
            >,
        >,
    >,
    /// 6h TTL cache for Last.fm track.getSimilar radio seeds. The radio
    /// endpoint can replay the same seed several times during exploration, so
    /// keep similar rows per process instead of re-hitting Last.fm every call.
    pub lastfm_similar_cache: services::radio::LastFmSimilarCache,
    pub playback_runtime: Option<PlaybackRuntimeState>,
    pub playback_runtime_info: Option<PlaybackRuntimeInfo>,
    pub playback_generation: Arc<AtomicU64>,
    pub current_stream_display: Option<StreamDisplayInfo>,
    pub pending_stream_display: Option<StreamDisplayInfo>,
    pub next_prebuffer_inflight: Option<NextPrebufferKey>,
    pub last_drop_preview: Option<DropPreviewRuntimeState>,
    pub active_listen_session: Option<playback::player::ActiveListenSession>,
    pub live_listen_session: Option<playback::player::LiveListenSession>,
    pub external_playback_track: Option<db::models::Track>,
    pub ephemeral_tidal_track: Option<db::models::Track>,
    /// What actually played, in order, for previous-track navigation. The
    /// queue cannot serve as history (shuffle/automix/mix rows); see
    /// `playback::history`. In-memory only: resets on restart.
    pub play_history: playback::history::PlayHistory,
    /// Cancellation flag for in-flight TIDAL device code login polling.
    pub tidal_login_cancel: Arc<AtomicBool>,
    /// Reentrancy guard — true while a TIDAL library sync is running. Manual
    /// click and the boot-time auto-sync both observe this to avoid racing on
    /// the same `albums`/`tracks`/`playlists` rows and producing inconsistent
    /// `is_favorite` state.
    pub tidal_sync_running: Arc<AtomicBool>,
    /// Cancellation flag for the in-flight TIDAL sync. Checked between pages
    /// in `do_tidal_sync`; reset to `false` at sync start.
    pub tidal_sync_cancel: Arc<AtomicBool>,
    /// RSS feed aggregator for music news and articles
    pub rss_aggregator: Arc<services::rss_feeds::FeedAggregator>,
    // Audio analysis
    pub analysis_tx:
        Option<tokio::sync::mpsc::UnboundedSender<services::audio_analysis::AnalysisJob>>,
    pub dj_analysis_tx: Option<
        tokio::sync::mpsc::UnboundedSender<services::audio_analysis::dj_profile::DjAnalysisJob>,
    >,
    pub dj_profile_rebuild_inflight:
        Arc<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>>,
    pub audio_analysis_cancel: Arc<AtomicBool>,
    pub audio_analysis_running: Arc<AtomicBool>,
    /// MusicBrainz enrichment running flag — gates auto-enrich + manual-trigger
    /// handler against double-runs.
    pub musicbrainz_enrich_running: Arc<AtomicBool>,
    /// TIDAL metadata self-heal running flag - gates the background repair pass
    /// (`services::tidal::repair`) that backfills tracks persisted with a zero
    /// duration / missing album against double-runs.
    pub tidal_repair_running: Arc<AtomicBool>,
    /// Last.fm enrichment progress.
    pub lastfm_enrich_running: Arc<AtomicBool>,
    pub lastfm_enrich_cancel: Arc<AtomicBool>,
    pub lastfm_enrich_total: Arc<std::sync::atomic::AtomicUsize>,
    pub lastfm_enrich_processed: Arc<std::sync::atomic::AtomicUsize>,
    /// Pre-fetch phase: unique artist count and how many have been fetched.
    pub lastfm_prefetch_total: Arc<std::sync::atomic::AtomicUsize>,
    pub lastfm_prefetch_done: Arc<std::sync::atomic::AtomicUsize>,
    /// Epoch seconds when the current run started; 0 when idle. Used to
    /// compute an observed throughput rate for the frontend's ETA display.
    pub lastfm_enrich_started_at: Arc<std::sync::atomic::AtomicI64>,
    /// Discovery training cancel flag — flipped to true by POST /api/discovery/train/stop,
    /// reset to false at the start of each training run.
    pub discovery_train_cancel: Arc<AtomicBool>,
    /// Single-flight guard for the radio similarity index rebuild. Held while
    /// `compute_track_similarity` runs so overlapping `LibrarySynced` events and
    /// the daily catch-up ticker don't kick off a second multi-minute rebuild.
    pub radio_similarity_running: Arc<AtomicBool>,
    /// Seeds already refreshed this session, with model_id + timestamp.
    /// Entries expire after `REFRESH_TTL` or whenever the active model_id changes,
    /// so re-training or long sessions don't pin stale neighbor data.
    pub refreshed_seeds: Arc<
        std::sync::Mutex<std::collections::HashMap<i64, services::neighbor_refresh::RefreshEntry>>,
    >,
    /// Cached embedding load (per model) for the seed-refresh path.
    /// Avoids full table scans when several seeds are refreshed in sequence.
    pub embedding_cache: Arc<std::sync::Mutex<Option<services::neighbor_refresh::EmbeddingCache>>>,
    /// Symmetric key used to encrypt service secrets (currently only the
    /// Last.fm scrobble session_key — see `services/crypto.rs`).
    pub master_key: services::crypto::MasterKey,
    pub prepared_ephemeral_tidal_next: Option<PreparedEphemeralTidalNext>,
    /// Last.fm app shared secret, loaded once from `LASTFM_API_SECRET` env at
    /// boot. `None` disables every scrobble auth + scrobble call (endpoints
    /// return HTTP 501). Never serialized into responses, never logged.
    pub lastfm_api_secret: Option<String>,
    /// Shared bearer token for network auth
    pub server_token: String,
    /// `true` only while the CPAL callback is actively draining samples (set by the
    /// `Started` runtime event, cleared on `Stopped` / `Finished` / startup).
    /// Lets `get_playback_state` return `is_playing: false` during the buffering phase
    /// so the frontend doesn't show a running counter with no audio.
    pub audio_active: Arc<AtomicBool>,
    /// Epoch seconds of the most recent manual queue clear, or 0 if never.
    /// Read by `ensure_automix_queue_depth` to suppress auto-refill for ~60s
    /// after a user-initiated clear so the user doesn't see automix
    /// instantly negate their action. Reset on any new user-driven play
    /// (play_track, radio_start, etc.) so automix re-engages naturally.
    pub user_cleared_at: Arc<std::sync::atomic::AtomicI64>,
    /// Spotify partner-GraphQL client for the public-stats endpoints. Built
    /// once at boot when the `spotify-public` cargo feature is on; absent
    /// from the struct entirely in feature-off builds. Route handlers that
    /// touch it live behind matching `#[cfg]` blocks.
    #[cfg(feature = "spotify-public")]
    pub spotify_public: Arc<services::spotify_public::SpotifyPublicClient>,
    /// Sportify (anonymous Spotify metadata proxy) client used by the
    /// `/api/discovery/sportify/*` discovery routes. Constructed once at boot.
    pub sportify_client: Option<Arc<services::sportify::SportifyClient>>,
    /// Cache TTL config for Sportify metadata + TIDAL resolution maps.
    pub sportify_cache_config: services::sportify::cache::SportifyCacheConfig,
    /// Eager-first-N + lazy-rest tunables for discovery list endpoints.
    pub sportify_resolve_config: services::sportify::cache::SportifyResolveConfig,
    /// Unified track-download queue + worker status. One sequential worker drains it;
    /// single-track requests jump ahead of queued batch items. See `services::download`.
    pub downloads: services::download::DownloadManager,
}

/// Events broadcast across the application
#[derive(Debug, Clone)]
pub enum AppEvent {
    PlaybackStateChanged,
    LibrarySynced,
    /// Emitted when the radio similarity index (`track_similarity`) finishes
    /// rebuilding, manually or via the auto-rebuild listener. Carries the pair
    /// count so the Settings panel can refresh without polling.
    RadioSimilarityComputed {
        pairs: i64,
    },
    MusicBrainzEnriched,
    TrackChanged {
        track_id: i64,
    },
    SyncProgress {
        service: String,
        progress: f32,
    },
    SyncFailed {
        service: String,
        message: String,
    },
    QueueUpdated,
    ListenHistoryUpdated {
        track_id: i64,
    },
    PlaybackFailed {
        message: String,
    },
    /// A queued track couldn't be played (TIDAL pulled the asset, `4005`
    /// asset-not-ready, or a hard rejection) and playback skipped past it. Lets
    /// the UI toast which track dropped out and why, instead of freezing on a
    /// silent dead row.
    TrackSkipped {
        track_id: i64,
        title: String,
        reason: String,
    },
    TrainingProgress {
        stage: String,
        progress: f32,
        message: String,
        current_track_id: Option<i64>,
        current_track_title: Option<String>,
        tracks_done: u32,
        tracks_total: u32,
    },
    // Audio analysis events
    AudioAnalysisProgress {
        analyzed: u32,
        total: u32,
        mode: String,
    },
    AudioAnalysisComplete {
        analyzed: u32,
    },
    /// Emitted by the passive analysis actor once a single track's DSP
    /// features have been written. Carries the track id so the frontend
    /// can invalidate per-track caches (currentTrackFeatures, automix
    /// featureCache) without a page reload. Distinct from the progress
    /// counter event which is shape-stable for the status UI.
    TrackAnalyzed {
        track_id: i64,
    },
    // DiscoverSpace per-seed background refresh progress + complete
    DiscoverySpaceRefreshProgress {
        seed_track_id: i64,
        stage: String,
        progress: f32,
    },
    DiscoverySpaceRefreshed {
        seed_track_id: i64,
    },
    /// WASAPI exclusive grab succeeded on the audio engine.
    AudioExclusiveEngaged {
        device: String,
        transport_format: String,
    },
    /// WASAPI exclusive grab failed; runtime fell back to cpal shared. The
    /// `reason` is human-readable and surfaced in the settings red-pill banner.
    AudioExclusiveFailed {
        device: String,
        reason: String,
    },
    /// WASAPI exclusive render thread released the device after idle. Audio
    /// engine is currently shared until the next Resume / Play re-grabs.
    AudioExclusiveReleased {
        device: String,
    },
    /// Track-download worker progress. `current_title` is the track being encoded
    /// right now; `done`/`total` count the whole queued batch so the toast can show
    /// "Downloading: 3/15" and survive a UI navigation via the status endpoint.
    DownloadProgress {
        done: u32,
        total: u32,
        current_title: Option<String>,
    },
    /// One track finished downloading. Lets the single-download toast resolve with the
    /// saved path (for "Show in folder") without waiting for the whole batch.
    DownloadItemDone {
        track_id: i64,
        ok: bool,
        already: bool,
        path: Option<String>,
        error: Option<String>,
    },
    /// Track-download queue drained. `ok` succeeded, `failed` could not be saved.
    DownloadComplete {
        ok: u32,
        failed: u32,
    },
}

pub type SharedState = Arc<RwLock<AppState>>;

/// Read an env var holding a number of days; fall back to `default` on
/// missing or unparseable input. Returns the value in whole days as `i64`.
fn parse_days_env(var: &str, default: i64) -> i64 {
    std::env::var(var)
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|d| *d > 0)
        .unwrap_or(default)
}

fn parse_usize_env(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

fn resolve_bind_addr(db: &db::Database) -> String {
    // NOOR_ADDR env var always wins (power-user override)
    if let Ok(addr) = std::env::var("NOOR_ADDR")
        && !addr.trim().is_empty()
    {
        return addr;
    }
    let port = server::noor_port();
    // --host flag forces 0.0.0.0
    if std::env::args().any(|a| a == "--host") {
        return format!("0.0.0.0:{port}");
    }
    // DB preference (set by Tauri tray toggle or headless users)
    let host_mode = db
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
    if host_mode {
        format!("0.0.0.0:{port}")
    } else {
        format!("127.0.0.1:{port}")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn host_flag_detection() {
        // Simulate args: just test the parsing logic directly
        let args = ["noor-server".to_string(), "--host".to_string()];
        let has_host = args.iter().any(|a| a == "--host");
        assert!(has_host);

        let args_no_flag = ["noor-server".to_string()];
        let has_host = args_no_flag.iter().any(|a| a == "--host");
        assert!(!has_host);
    }

    // Characterization test for the boot-time wipe at main.rs:383-399. This
    // re-runs the exact SQL the boot path runs so a future refactor (e.g. extracting
    // it to a `reset_ephemeral_session` function) can be checked against the
    // current behavior. CLAUDE.md flags this wipe as load-bearing: queue and
    // current_track_id MUST be cleared, but user prefs (volume, shuffle, repeat,
    // automix flags) MUST survive.
    #[test]
    fn boot_wipe_clears_queue_resets_playback_state_and_marks_orphan_runs_failed() {
        let db = crate::db::Database::open_in_memory().expect("open in-memory db");
        db.run_migrations().expect("migrations");

        // Seed: a stale session (track playing mid-position, queue with rows),
        // a "running" training run that would otherwise be orphaned, AND user
        // prefs we expect the wipe to PRESERVE.
        db.with_conn(|conn| {
            conn.execute("INSERT INTO artists (id, name) VALUES (1, 'A')", [])?;
            conn.execute(
                "INSERT INTO tracks (
                    id, title, artist_id, duration_ms, source, fidelity_score
                 ) VALUES (1, 'T', 1, 180000, 'tidal_stream', 0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'user')",
                [],
            )?;
            conn.execute(
                "UPDATE playback_state
                 SET is_playing = 1, current_track_id = 1, position_ms = 12345,
                     volume = 0.73, shuffle_mode = 'weighted', repeat_mode = 'one',
                     automix_enabled = 1
                 WHERE id = 1",
                [],
            )?;
            conn.execute(
                "INSERT INTO training_runs (stage, status, started_at)
                 VALUES ('train', 'running', datetime('now'))",
                [],
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .expect("seed");

        // Run the boot-wipe SQL verbatim from main.rs:383-399.
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE playback_state SET is_playing = 0, current_track_id = NULL, current_queue_item_id = NULL, position_ms = 0 WHERE id = 1",
                [],
            )?;
            conn.execute("DELETE FROM queue", [])?;
            conn.execute(
                "UPDATE training_runs
                 SET status = 'failed',
                     finished_at = datetime('now'),
                     error_text = COALESCE(error_text, 'interrupted by server restart')
                 WHERE status = 'running'",
                [],
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .expect("wipe");

        // Post-conditions.
        db.with_conn(|conn| {
            let queue_count: i64 =
                conn.query_row("SELECT COUNT(*) FROM queue", [], |r| r.get(0))?;
            assert_eq!(queue_count, 0, "queue must be empty after boot wipe");

            let (
                is_playing,
                current_track_id,
                current_queue_item_id,
                position_ms,
                volume,
                shuffle_mode,
                repeat_mode,
                automix_enabled,
            ): (i64, Option<i64>, Option<i64>, i64, f64, String, String, i64) = conn.query_row(
                "SELECT is_playing, current_track_id, current_queue_item_id, position_ms,
                        volume, shuffle_mode, repeat_mode, automix_enabled
                 FROM playback_state WHERE id = 1",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                    ))
                },
            )?;
            // Cleared:
            assert_eq!(is_playing, 0, "is_playing must reset to 0");
            assert!(
                current_track_id.is_none(),
                "current_track_id must reset to NULL"
            );
            assert!(
                current_queue_item_id.is_none(),
                "current_queue_item_id must reset to NULL"
            );
            assert_eq!(position_ms, 0, "position_ms must reset to 0");
            // Preserved (CLAUDE.md guarantee):
            assert!(
                (volume - 0.73).abs() < 1e-9,
                "user prefs: volume must survive boot wipe, got {volume}"
            );
            assert_eq!(
                shuffle_mode, "weighted",
                "user prefs: shuffle_mode must survive"
            );
            assert_eq!(repeat_mode, "one", "user prefs: repeat_mode must survive");
            assert_eq!(automix_enabled, 1, "user prefs: automix flag must survive");

            let training_status: String =
                conn.query_row("SELECT status FROM training_runs LIMIT 1", [], |r| r.get(0))?;
            assert_eq!(
                training_status, "failed",
                "orphan training_runs must be marked failed"
            );

            Ok::<_, anyhow::Error>(())
        })
        .expect("assert");
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "noor_server=info".into()),
        )
        .init();

    info!("NOOR — Starting up...");

    // Resolve DB path: NOOR_DB, then NOOR_DATA_DIR/noor.db for installed
    // builds, then the existing dev/portable fallbacks.
    let db_path = paths::resolve_db_path_from_env()
        .to_string_lossy()
        .into_owned();
    info!("Database path: {}", db_path);

    // Initialize database
    let db = db::Database::open(&db_path)?;
    db.run_migrations()?;
    let genre_count = db.with_conn(genre::taxonomy::ensure_taxonomy_loaded)?;
    db.seed_genres_from_taxonomy()?;
    // The audio runtime is ephemeral — it never survives a process restart. Clear the
    // whole transient session (current track, position, queue) so the player boots fresh
    // instead of showing a stale track that "Play" can't actually resume. User prefs
    // (volume, shuffle/repeat/automix modes) stay put.
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE playback_state SET is_playing = 0, current_track_id = NULL, current_queue_item_id = NULL, position_ms = 0 WHERE id = 1",
            [],
        )?;
        conn.execute("DELETE FROM queue", [])?;
        // Discovery training runs in-process; if the previous process died mid-run
        // the row is left at status='running' forever and the UI's Stop button has
        // no live cancel handle to flip. Mark orphans failed so the user can retrain.
        let orphaned = conn.execute(
            "UPDATE training_runs
             SET status = 'failed',
                 finished_at = datetime('now'),
                 error_text = COALESCE(error_text, 'interrupted by server restart')
             WHERE status = 'running'",
            [],
        )?;
        if orphaned > 0 {
            info!("Reconciled {} orphaned training run(s) on startup", orphaned);
        }
        Ok(())
    })?;
    info!("Database initialized");
    info!("Genre taxonomy loaded: {} genres", genre_count);

    // Load (or generate) the master key used to encrypt service secrets.
    // Lives next to noor.db so it travels with the install.
    let master_key = services::crypto::MasterKey::load_or_generate(
        &services::crypto::secret_dir_for_db(&db_path),
    )?;

    // Last.fm shared app secret. Optional: when missing, all scrobble auth +
    // scrobble endpoints return 501 and the rest of the app keeps working.
    // Never round-tripped through the UI, never logged.
    let lastfm_api_secret = std::env::var("LASTFM_API_SECRET")
        .ok()
        .filter(|s| !s.is_empty());

    // Public Spotify stats: gated only on the `spotify-public` cargo feature
    // (default on). No runtime env var - the feature controls compile-in of
    // the whole module, and feature-off route handlers return an empty
    // payload.
    #[cfg(feature = "spotify-public")]
    let spotify_public_client = Arc::new(
        services::spotify_public::SpotifyPublicClient::new(db.clone())
            .expect("SpotifyPublicClient init: reqwest builder failure should be impossible"),
    );
    #[cfg(feature = "spotify-public")]
    info!("Public Spotify stats enabled (anonymous GraphQL)");

    if lastfm_api_secret.is_some() {
        info!("Last.fm scrobbling enabled (LASTFM_API_SECRET present)");
    } else {
        info!("Last.fm scrobbling disabled (set LASTFM_API_SECRET to enable)");
    }

    // Sportify (anonymous Spotify metadata proxy) powers /discover. Default
    // base URL ships in code; env vars let ops point at a self-hosted mirror.
    let configured_sportify_base_url = std::env::var("SPORTIFY_API_BASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let sportify_base_url = configured_sportify_base_url
        .clone()
        .unwrap_or_else(|| "https://spotify.xwolf.space".to_string());
    let sportify_fallback_base_urls = if configured_sportify_base_url.is_some() {
        Vec::new()
    } else {
        vec!["https://sportify.xcasper.space".to_string()]
    };
    let sportify_client =
        match services::sportify::SportifyClient::new(services::sportify::SportifyClientConfig {
            base_url: sportify_base_url.clone(),
            fallback_base_urls: sportify_fallback_base_urls.clone(),
            user_agent: format!(
                "noor-server/{} (sportify discovery)",
                env!("CARGO_PKG_VERSION")
            ),
        }) {
            Ok(client) => {
                if sportify_fallback_base_urls.is_empty() {
                    info!("Sportify discovery client ready ({})", sportify_base_url);
                } else {
                    info!(
                        "Sportify discovery client ready ({} with fallback {})",
                        sportify_base_url,
                        sportify_fallback_base_urls.join(", ")
                    );
                }
                Some(Arc::new(client))
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to construct Sportify client: {}; /discover will be degraded",
                    e
                );
                None
            }
        };
    let sportify_cache_config = services::sportify::cache::SportifyCacheConfig {
        meta_ttl_secs: parse_days_env("DISCOVERY_CACHE_TTL_DAYS", 30) * 86_400,
        resolve_ttl_secs: parse_days_env("RESOLVE_CACHE_TTL_DAYS", 30) * 86_400,
        unresolved_retry_after_secs: parse_days_env("RESOLVE_RETRY_AFTER_DAYS", 7) * 86_400,
    };
    let sportify_resolve_config = services::sportify::cache::SportifyResolveConfig {
        eager_n: parse_usize_env(
            "RESOLVE_EAGER_N",
            services::sportify::cache::DEFAULT_EAGER_N,
        ),
        bulk_concurrency: parse_usize_env(
            "RESOLVE_BULK_CONCURRENCY",
            services::sportify::cache::DEFAULT_BULK_CONCURRENCY,
        ),
    };

    // Event bus for real-time state sync
    let (event_tx, _) = broadcast::channel(256);

    // Load persisted TIDAL tokens if available
    let tidal_tokens: Option<services::tidal::auth::TidalTokens> = db
        .with_conn(|conn| {
            let result = conn.query_row(
                "SELECT access_token_enc FROM service_auth WHERE service='tidal'",
                [],
                |row| row.get::<_, Vec<u8>>(0),
            );
            Ok(match result {
                Ok(bytes) => {
                    services::tidal::auth::decode_persisted_tidal_tokens(&master_key, &bytes)
                        .ok()
                        .flatten()
                        .map(|persisted| {
                            let needs_rewrite = persisted.needs_encrypted_rewrite();
                            let tokens = persisted.into_tokens();
                            if needs_rewrite
                                && let Ok(blob) =
                                    services::tidal::auth::encode_persisted_tidal_tokens(
                                        &master_key,
                                        &tokens,
                                    )
                            {
                                let _ = conn.execute(
                            "UPDATE service_auth SET access_token_enc = ?1 WHERE service='tidal'",
                            rusqlite::params![blob],
                        );
                            }
                            tokens
                        })
                        .inspect(|t: &services::tidal::auth::TidalTokens| {
                            info!("Loaded persisted TIDAL tokens for user {}", t.user_id);
                        })
                }
                Err(_) => None,
            })
        })
        .unwrap_or(None);

    // Generate or load the server access token
    let server_token = db.with_conn(db::queries::ensure_server_token)?;
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  NOOR access token: {}", server_token);
    info!("  Copy this into the app on any new device.");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    // Resolve bind address before db is moved into AppState
    let addr = resolve_bind_addr(&db);

    // Shared client for Last.fm / MusicBrainz / Discogs / RSS / session
    // recovery. reqwest has no default timeout, so an unresponsive upstream
    // could otherwise hang the calling request indefinitely. Match the TIDAL
    // client's bounds (see TidalClient::build_http_client); 30s is ample for
    // any JSON API call. Streaming downloads use their own client.
    let http_client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let rss_aggregator = Arc::new(services::rss_feeds::FeedAggregator::new(
        http_client.clone(),
    ));

    // Spawn audio analysis actor
    let analysis_cancel = Arc::new(AtomicBool::new(false));
    let analysis_tx = services::audio_analysis::spawn_actor(
        db.clone(),
        event_tx.clone(),
        analysis_cancel.clone(),
        services::audio_analysis::AnalysisConfig::default(),
    );
    info!("Audio analysis actor spawned");
    let dj_analysis_tx = services::audio_analysis::dj_profile::spawn_dj_profile_actor(db.clone());
    info!("DJ profile analysis actor spawned");

    let state = Arc::new(RwLock::new(AppState {
        db,
        event_tx,
        http_client,
        tidal_http_client: services::tidal::client::TidalClient::build_http_client(),
        tidal_tokens,
        tidal_mixes_cache: Arc::new(std::sync::Mutex::new(None)),
        tidal_radio_stations_cache: Arc::new(std::sync::Mutex::new(None)),
        home_picks_cache: Arc::new(std::sync::Mutex::new(None)),
        tidal_moods_cache: Arc::new(std::sync::Mutex::new(None)),
        tidal_page_modules_cache: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        tidal_playlist_tracks_cache: Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        lastfm_similar_cache: services::radio::new_lastfm_similar_cache(),
        playback_runtime: None,
        playback_runtime_info: None,
        playback_generation: Arc::new(AtomicU64::new(1)),
        current_stream_display: None,
        pending_stream_display: None,
        next_prebuffer_inflight: None,
        last_drop_preview: None,
        active_listen_session: None,
        live_listen_session: None,
        external_playback_track: None,
        ephemeral_tidal_track: None,
        play_history: playback::history::PlayHistory::default(),
        tidal_login_cancel: Arc::new(AtomicBool::new(false)),
        tidal_sync_running: Arc::new(AtomicBool::new(false)),
        tidal_sync_cancel: Arc::new(AtomicBool::new(false)),
        rss_aggregator,
        analysis_tx: Some(analysis_tx),
        dj_analysis_tx: Some(dj_analysis_tx),
        dj_profile_rebuild_inflight: Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        audio_analysis_cancel: analysis_cancel,
        audio_analysis_running: Arc::new(AtomicBool::new(false)),
        musicbrainz_enrich_running: Arc::new(AtomicBool::new(false)),
        tidal_repair_running: Arc::new(AtomicBool::new(false)),
        lastfm_enrich_running: Arc::new(AtomicBool::new(false)),
        lastfm_enrich_cancel: Arc::new(AtomicBool::new(false)),
        lastfm_enrich_total: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        lastfm_enrich_processed: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        lastfm_prefetch_total: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        lastfm_prefetch_done: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        lastfm_enrich_started_at: Arc::new(std::sync::atomic::AtomicI64::new(0)),
        discovery_train_cancel: Arc::new(AtomicBool::new(false)),
        radio_similarity_running: Arc::new(AtomicBool::new(false)),
        refreshed_seeds: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        embedding_cache: Arc::new(std::sync::Mutex::new(None)),
        master_key,
        prepared_ephemeral_tidal_next: None,
        lastfm_api_secret,
        server_token,
        audio_active: Arc::new(AtomicBool::new(false)),
        user_cleared_at: Arc::new(std::sync::atomic::AtomicI64::new(0)),
        #[cfg(feature = "spotify-public")]
        spotify_public: spotify_public_client,
        sportify_client,
        sportify_cache_config,
        sportify_resolve_config,
        downloads: services::download::DownloadManager::new(),
    }));

    services::audio_analysis::queue_prescanner::spawn(state.clone());
    info!("Queue DSP prescanner spawned");
    services::scrobbling::spawn_periodic_drain(state.clone());
    services::scrobbling::spawn_drain(state.clone());
    info!("Scrobble outbox drain spawned");

    // Check for auto-sync daily services and trigger sync if needed
    {
        let state_read = state.read().await;
        let auto_sync_services = state_read
            .db
            .with_conn(db::queries::get_auto_sync_services)
            .unwrap_or_else(|e| {
                tracing::warn!("Failed to query auto-sync services: {}", e);
                vec![]
            });
        drop(state_read);

        if !auto_sync_services.is_empty() {
            for service in &auto_sync_services {
                tracing::info!(
                    target: "noor.auto_sync",
                    event = "startup_sync",
                    service = %service,
                    "Auto-sync daily enabled — triggering sync on startup"
                );
            }
            // Spawn background sync for TIDAL if enabled
            if auto_sync_services.iter().any(|s| s == "tidal") {
                let state_clone = state.clone();
                tokio::spawn(async move {
                    // Wait a bit for server to fully start
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

                    // "Daily" means once per 24h, not "every boot". Skip if a
                    // recorded sync is fresher than 24h.
                    let recent = {
                        let s = state_clone.read().await;
                        s.db.with_conn(|conn| {
                            db::queries::sync_within_window(conn, "tidal", 24 * 60 * 60)
                        })
                        .unwrap_or(false)
                    };
                    if recent {
                        tracing::info!(
                            target: "noor.auto_sync",
                            event = "startup_sync_skipped",
                            service = "tidal",
                            "Auto-sync daily skipped — last sync was <24h ago"
                        );
                        return;
                    }

                    match server::routes::trigger_auto_sync(&state_clone, "tidal").await {
                        Ok(stats) => {
                            tracing::info!(
                                target: "noor.auto_sync",
                                event = "startup_sync_complete",
                                service = "tidal",
                                tracks = stats.tracks,
                                albums = stats.albums,
                                "Auto-sync daily completed on startup"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                target: "noor.auto_sync",
                                event = "startup_sync_failed",
                                service = "tidal",
                                error = %e,
                                "Auto-sync daily failed on startup"
                            );
                        }
                    }
                });
            }
        }
    }

    // Auto-enrichment: opportunistic post-import + daily catch-up.
    //
    // The library has 50%+ of tracks with no genre tags whenever new music
    // arrives outside a manual enrichment run (see docs/genre-data-quality-2026-05-07.md).
    // These two background tasks close that gap automatically:
    //
    // 1. Listener — every `LibrarySynced` event triggers a no-op-if-idle pass,
    //    so any sync/import path that emits the event gets enrichment for
    //    free without changes to the handler.
    // 2. Daily loop — catches tracks added by paths that don't emit
    //    `LibrarySynced`, and retries any rows that errored on the previous run.
    //
    // Both are gated by the per-runner `*_enrich_running` atomics so they
    // never overlap with manual enrichment from the Settings UI or with each
    // other.

    // Real-time output spectrum for the wallpaper visualiser: FFTs the audio
    // that's actually playing (off the RT thread, ~30 Hz) so WS clients can
    // drive a true spectrum. Idle when silent.
    tokio::spawn(crate::playback::spectrum::run_spectrum_task());

    {
        let listener_state = state.clone();
        let mut event_rx = listener_state.read().await.event_tx.subscribe();
        tokio::spawn(async move {
            // Defer the first auto-enrich until 60s after boot so we don't
            // contend with the startup TIDAL sync's own work.
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            loop {
                match event_rx.recv().await {
                    Ok(AppEvent::LibrarySynced) => {
                        services::auto_enrich::run_if_idle(listener_state.clone()).await;
                        services::tidal::repair::run_if_idle(listener_state.clone()).await;
                        // Auto-dedupe: merges same-recording duplicates after
                        // every sync/import. Emits LibrarySynced only when it
                        // changed rows, so the retrigger converges.
                        services::library_dedupe::run_if_idle(listener_state.clone()).await;
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(target: "noor.auto_enrich", lagged = n, "event listener lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
    {
        let loop_state = state.clone();
        tokio::spawn(async move {
            // Wait 90s before the first sweep so listener-driven enrichment from
            // the boot-time TIDAL sync gets a head start.
            tokio::time::sleep(std::time::Duration::from_secs(90)).await;
            services::auto_enrich::run_if_idle(loop_state.clone()).await;
            services::tidal::repair::run_if_idle(loop_state.clone()).await;
            services::library_dedupe::run_if_idle(loop_state.clone()).await;

            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(86_400));
            ticker.tick().await; // consume the immediate first tick
            loop {
                ticker.tick().await;
                services::auto_enrich::run_if_idle(loop_state.clone()).await;
                services::tidal::repair::run_if_idle(loop_state.clone()).await;
                services::library_dedupe::run_if_idle(loop_state.clone()).await;
            }
        });
    }

    // Radio similarity index auto-rebuild.
    //
    // `track_similarity` feeds radio's Engine recall lane but has no trigger of
    // its own — left alone it stays empty and the Engine lane silently
    // contributes nothing. These two tasks keep it fresh:
    //
    // 1. Listener — every `LibrarySynced` event means the library changed, so
    //    the index is stale. `run_if_stale` debounces bursts and defers while
    //    the app is busy, marking a dirty flag so the change isn't lost.
    // 2. Hourly ticker — rebuilds a debounced/deferred change once its window
    //    clears, and catches an aging index on installs that rarely sync.
    //
    // Both short-circuit on the `radio_similarity_running` atomic, so a rebuild
    // in flight is never doubled up. See services::radio_similarity.
    {
        let listener_state = state.clone();
        let mut event_rx = listener_state.read().await.event_tx.subscribe();
        tokio::spawn(async move {
            // Defer past the boot-time sync so we don't contend with it.
            tokio::time::sleep(std::time::Duration::from_secs(120)).await;
            use services::radio_similarity::RebuildTrigger;
            loop {
                match event_rx.recv().await {
                    Ok(AppEvent::LibrarySynced) => {
                        services::radio_similarity::run_if_stale(
                            listener_state.clone(),
                            RebuildTrigger::LibrarySynced,
                        )
                        .await;
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(target: "noor.radio_similarity", lagged = n, "event listener lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
    {
        let loop_state = state.clone();
        tokio::spawn(async move {
            use services::radio_similarity::RebuildTrigger;
            // First sweep 150s after boot — behind the auto-enrich head start.
            tokio::time::sleep(std::time::Duration::from_secs(150)).await;
            services::radio_similarity::run_if_stale(loop_state.clone(), RebuildTrigger::Periodic)
                .await;

            // Hourly: frequent enough that a debounced change is picked up soon
            // after its 6h window clears, cheap enough to no-op the rest of the
            // time (one age check, then the idle gate).
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(3_600));
            ticker.tick().await; // consume the immediate first tick
            loop {
                ticker.tick().await;
                services::radio_similarity::run_if_stale(
                    loop_state.clone(),
                    RebuildTrigger::Periodic,
                )
                .await;
            }
        });
    }

    // Pending-queue GC: sweep stale locks and expired unresolved rows.
    {
        let gc_state = state.clone();
        tokio::spawn(async move {
            // Startup sweep — clear any locks left over from a previous crash.
            {
                let s = gc_state.read().await;
                if let Err(e) = s.db.with_conn(crate::playback::queue::gc_pending_queue) {
                    tracing::warn!("pending queue GC (startup): {e}");
                }
            }
            // Hourly sweeps.
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(3600));
            ticker.tick().await; // consume the immediate first tick
            loop {
                ticker.tick().await;
                let s = gc_state.read().await;
                match s.db.with_conn(crate::playback::queue::gc_pending_queue) {
                    Ok((expired, locks)) if expired > 0 || locks > 0 => {
                        tracing::info!(
                            expired,
                            stale_locks_cleared = locks,
                            "pending queue GC sweep"
                        );
                    }
                    Err(e) => tracing::warn!("pending queue GC: {e}"),
                    _ => {}
                }
            }
        });
    }

    // Start HTTP + WebSocket server
    info!("Starting server on http://{}", addr);
    // Boot-time cache warming: ~10s after start, hit our own home endpoints over
    // loopback so their in-process caches (TIDAL mixes/radio, picks, Last.fm
    // recommendations) are warm before the first real client request. This is the
    // "first startup caches stuff" behaviour - the first Home open reads a warm
    // cache instead of waiting on cold fetches. Best-effort: a 503 (e.g. TIDAL not
    // connected) just means there's nothing to warm yet, and it never blocks boot.
    async fn warm_home_caches(http: reqwest::Client, port: String, token: String) {
        let base = format!("http://127.0.0.1:{port}");
        for path in [
            "/api/home/picks",
            "/api/home/recommendations",
            "/api/tidal/mixes",
            "/api/tidal/radio-stations",
        ] {
            let url = format!("{base}{path}");
            match http.get(&url).bearer_auth(&token).send().await {
                Ok(resp) => tracing::info!(
                    target: "noor.warm",
                    event = "warmed",
                    path = path,
                    status = resp.status().as_u16(),
                    "home cache warm request sent"
                ),
                Err(e) => tracing::debug!(
                    target: "noor.warm",
                    event = "warm_skipped",
                    path = path,
                    error = %e,
                    "home cache warm request failed"
                ),
            }
        }
    }
    {
        let (warm_http, warm_token) = {
            let s = state.read().await;
            (s.http_client.clone(), s.server_token.clone())
        };
        let warm_port = addr.rsplit(':').next().unwrap_or("17600").to_string();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            warm_home_caches(warm_http, warm_port, warm_token).await;
        });
    }

    server::start(state, &addr).await?;

    Ok(())
}
