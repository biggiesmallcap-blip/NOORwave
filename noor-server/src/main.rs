mod db;
mod genre;
mod library;
mod metadata;
mod playback;
mod server;
mod services;
mod smart;
mod tags;

use anyhow::Result;
use rusqlite::OptionalExtension;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
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
    pub spotify_tokens: Option<services::spotify::auth::SpotifyTokens>,
    pub playback_runtime: Option<PlaybackRuntimeState>,
    pub playback_runtime_info: Option<PlaybackRuntimeInfo>,
    pub playback_generation: Arc<AtomicU64>,
    pub current_stream_display: Option<StreamDisplayInfo>,
    pub pending_stream_display: Option<StreamDisplayInfo>,
    pub active_listen_session: Option<playback::player::ActiveListenSession>,
    pub live_listen_session: Option<playback::player::LiveListenSession>,
    pub external_playback_track: Option<db::models::Track>,
    pub ephemeral_tidal_track: Option<db::models::Track>,
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
    /// ACRCloud client for sample recognition (loaded from service_auth if configured)
    pub acrcloud_client: Option<services::acrcloud::AcrCloudClient>,
    // Audio analysis
    pub analysis_tx:
        Option<tokio::sync::mpsc::UnboundedSender<services::audio_analysis::AnalysisJob>>,
    pub audio_analysis_cancel: Arc<AtomicBool>,
    pub audio_analysis_running: Arc<AtomicBool>,
    pub acrcloud_scan_running: Arc<AtomicBool>,
    pub acrcloud_daily_count: Arc<std::sync::atomic::AtomicU32>,
    /// Spotify enrichment progress (visible to status endpoint, survives UI navigation).
    pub spotify_enrich_running: Arc<AtomicBool>,
    pub spotify_enrich_total: Arc<std::sync::atomic::AtomicUsize>,
    pub spotify_enrich_processed: Arc<std::sync::atomic::AtomicUsize>,
    /// MusicBrainz enrichment running flag — gates auto-enrich + manual-trigger
    /// handler against double-runs.
    pub musicbrainz_enrich_running: Arc<AtomicBool>,
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
    /// Seeds already refreshed this session, with model_id + timestamp.
    /// Entries expire after `REFRESH_TTL` or whenever the active model_id changes,
    /// so re-training or long sessions don't pin stale neighbor data.
    pub refreshed_seeds: Arc<
        std::sync::Mutex<std::collections::HashMap<i64, services::neighbor_refresh::RefreshEntry>>,
    >,
    /// Cached embedding load (per model) for the seed-refresh path.
    /// Avoids full table scans when several seeds are refreshed in sequence.
    pub embedding_cache:
        Arc<tokio::sync::Mutex<Option<services::neighbor_refresh::EmbeddingCache>>>,
    /// Symmetric key used to encrypt service secrets (currently only the
    /// Last.fm scrobble session_key — see `services/crypto.rs`).
    pub master_key: services::crypto::MasterKey,
    /// Legacy pending ephemeral TIDAL tracks queued behind the currently-playing
    /// ephemeral track (e.g. the rest of a TIDAL mix the user clicked into).
    /// New persistent external queue actions should use pending rows in the
    /// `queue` table instead of adding more producers to this in-memory queue.
    /// Auto-advanced by `handle_runtime_finished` when the active ephemeral
    /// track ends. Cleared on explicit stop or when the user starts a
    /// different ephemeral track (`play_tidal_ephemeral` clears before
    /// queuing). Stream URLs resolved lazily at advance time — TIDAL
    /// stream URLs expire (~30 min) so pre-resolving the whole mix is wasteful.
    pub pending_tidal_mix_queue:
        Arc<std::sync::Mutex<std::collections::VecDeque<PendingEphemeralTidalTrack>>>,
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
    /// Public Spotify stats (anonymous GraphQL) toggle. Read once from
    /// `NOOR_SPOTIFY_PUBLIC_STATS` at startup. When false, the stats endpoint
    /// returns empty fields and never hits Spotify. The feature also requires
    /// the `spotify-public` cargo feature; without it the env var is ignored
    /// and we log one warning at startup.
    pub spotify_public_stats_enabled: bool,
    /// Sportify (anonymous Spotify metadata proxy) client used by the
    /// `/api/discovery/sportify/*` discovery routes. Constructed once at boot.
    pub sportify_client: Option<Arc<services::sportify::SportifyClient>>,
    /// Cache TTL config for Sportify metadata + TIDAL resolution maps.
    pub sportify_cache_config: services::sportify::cache::SportifyCacheConfig,
    /// Eager-first-N + lazy-rest tunables for discovery list endpoints.
    pub sportify_resolve_config: services::sportify::cache::SportifyResolveConfig,
}

/// Events broadcast across the application
#[derive(Debug, Clone)]
pub enum AppEvent {
    PlaybackStateChanged,
    LibrarySynced,
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
    // ACRCloud events
    AcrCloudScanProgress {
        scanned: u32,
        total: u32,
        matches_found: u32,
    },
    AcrCloudScanComplete {
        scanned: u32,
        matches_found: u32,
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
    // --host flag forces 0.0.0.0
    if std::env::args().any(|a| a == "--host") {
        return "0.0.0.0:3334".to_string();
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
        "0.0.0.0:3334".to_string()
    } else {
        "127.0.0.1:3334".to_string()
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

    // Resolve DB path: NOOR_DB env var, then exe-dir/noor.db (portable/installed),
    // with a dev-only fallback to workspace-root/noor.db when the exe is in target/{debug,release}.
    let db_path = std::env::var("NOOR_DB").unwrap_or_else(|_| {
        let exe = std::env::current_exe().ok();
        let exe_dir = exe.as_ref().and_then(|p| p.parent());

        let dev_db = exe_dir.and_then(|d| {
            let profile = d.file_name()?.to_str()?;
            if profile != "debug" && profile != "release" {
                return None;
            }
            let target = d.parent()?;
            if target.file_name()?.to_str()? != "target" {
                return None;
            }
            target.parent().map(|root| root.join("noor.db"))
        });

        dev_db
            .or_else(|| exe_dir.map(|d| d.join("noor.db")))
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "noor.db".to_string())
    });
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

    // Public Spotify stats are gated on (a) the env var being set and (b) the
    // `spotify-public` cargo feature being compiled in. The feature pulls in
    // `rquest` (Chrome TLS fingerprint) which we don't want in lean builds.
    let env_spotify_public = std::env::var("NOOR_SPOTIFY_PUBLIC_STATS")
        .ok()
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false);
    #[cfg(feature = "spotify-public")]
    let spotify_public_stats_enabled = env_spotify_public;
    #[cfg(not(feature = "spotify-public"))]
    let spotify_public_stats_enabled = {
        if env_spotify_public {
            warn!(
                "NOOR_SPOTIFY_PUBLIC_STATS=1 but binary built without `spotify-public` cargo feature; ignoring"
            );
        }
        false
    };
    if spotify_public_stats_enabled {
        info!("Public Spotify stats enabled (anonymous GraphQL)");
    }
    if lastfm_api_secret.is_some() {
        info!("Last.fm scrobbling enabled (LASTFM_API_SECRET present)");
    } else {
        info!("Last.fm scrobbling disabled (set LASTFM_API_SECRET to enable)");
    }

    // Sportify (anonymous Spotify metadata proxy) — powers /discover. Default
    // base URL ships in code; env var lets ops point at a self-hosted mirror.
    let sportify_base_url = std::env::var("SPORTIFY_API_BASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "https://sportify.xcasper.space".to_string());
    let sportify_client =
        match services::sportify::SportifyClient::new(services::sportify::SportifyClientConfig {
            base_url: sportify_base_url.clone(),
            user_agent: format!(
                "noor-server/{} (sportify discovery)",
                env!("CARGO_PKG_VERSION")
            ),
        }) {
            Ok(client) => {
                info!("Sportify discovery client ready ({})", sportify_base_url);
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

    // Spotify tokens are fetched on demand via the Client Credentials flow
    // using the user-supplied client_id/secret stored in service_auth.extra_data.
    let spotify_tokens: Option<services::spotify::auth::SpotifyTokens> = None;

    // Generate or load the server access token
    let server_token = db.with_conn(db::queries::ensure_server_token)?;
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  NOOR access token: {}", server_token);
    info!("  Copy this into the app on any new device.");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    // Resolve bind address before db is moved into AppState
    let addr = resolve_bind_addr(&db);

    let http_client = reqwest::Client::new();
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

    let state = Arc::new(RwLock::new(AppState {
        db,
        event_tx,
        http_client,
        tidal_http_client: services::tidal::client::TidalClient::build_http_client(),
        tidal_tokens,
        tidal_mixes_cache: Arc::new(std::sync::Mutex::new(None)),
        tidal_radio_stations_cache: Arc::new(std::sync::Mutex::new(None)),
        spotify_tokens,
        playback_runtime: None,
        playback_runtime_info: None,
        playback_generation: Arc::new(AtomicU64::new(1)),
        current_stream_display: None,
        pending_stream_display: None,
        active_listen_session: None,
        live_listen_session: None,
        external_playback_track: None,
        ephemeral_tidal_track: None,
        tidal_login_cancel: Arc::new(AtomicBool::new(false)),
        tidal_sync_running: Arc::new(AtomicBool::new(false)),
        tidal_sync_cancel: Arc::new(AtomicBool::new(false)),
        rss_aggregator,
        acrcloud_client: None,
        analysis_tx: Some(analysis_tx),
        audio_analysis_cancel: analysis_cancel,
        audio_analysis_running: Arc::new(AtomicBool::new(false)),
        acrcloud_scan_running: Arc::new(AtomicBool::new(false)),
        acrcloud_daily_count: Arc::new(AtomicU32::new(0)),
        spotify_enrich_running: Arc::new(AtomicBool::new(false)),
        spotify_enrich_total: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        spotify_enrich_processed: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        musicbrainz_enrich_running: Arc::new(AtomicBool::new(false)),
        lastfm_enrich_running: Arc::new(AtomicBool::new(false)),
        lastfm_enrich_cancel: Arc::new(AtomicBool::new(false)),
        lastfm_enrich_total: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        lastfm_enrich_processed: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        lastfm_prefetch_total: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        lastfm_prefetch_done: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        lastfm_enrich_started_at: Arc::new(std::sync::atomic::AtomicI64::new(0)),
        discovery_train_cancel: Arc::new(AtomicBool::new(false)),
        refreshed_seeds: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        embedding_cache: Arc::new(tokio::sync::Mutex::new(None)),
        master_key,
        pending_tidal_mix_queue: Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
        lastfm_api_secret,
        server_token,
        audio_active: Arc::new(AtomicBool::new(false)),
        user_cleared_at: Arc::new(std::sync::atomic::AtomicI64::new(0)),
        spotify_public_stats_enabled,
        sportify_client,
        sportify_cache_config,
        sportify_resolve_config,
    }));

    services::audio_analysis::queue_prescanner::spawn(state.clone());
    info!("Queue DSP prescanner spawned");

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

            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(86_400));
            ticker.tick().await; // consume the immediate first tick
            loop {
                ticker.tick().await;
                services::auto_enrich::run_if_idle(loop_state.clone()).await;
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
    server::start(state, &addr).await?;

    Ok(())
}
