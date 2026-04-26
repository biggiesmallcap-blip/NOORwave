mod db;
mod genre;
mod library;
mod metadata;
mod playback;
mod server;
mod services;
mod smart;

use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32};
use tokio::sync::{RwLock, broadcast};
use tracing::info;

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
}

/// Shared application state accessible by all modules
pub struct AppState {
    pub db: db::Database,
    pub event_tx: broadcast::Sender<AppEvent>,
    pub http_client: reqwest::Client,
    pub tidal_tokens: Option<services::tidal::auth::TidalTokens>,
    pub spotify_tokens: Option<services::spotify::auth::SpotifyTokens>,
    pub playback_runtime: Option<PlaybackRuntimeState>,
    pub playback_runtime_info: Option<PlaybackRuntimeInfo>,
    pub active_listen_session: Option<playback::player::ActiveListenSession>,
    pub external_playback_track: Option<db::models::Track>,
    /// Cancellation flag for in-flight TIDAL device code login polling.
    pub tidal_login_cancel: Arc<AtomicBool>,
    /// RSS feed aggregator for music news and articles
    pub rss_aggregator: Arc<services::rss_feeds::FeedAggregator>,
    /// ACRCloud client for sample recognition (loaded from service_auth if configured)
    pub acrcloud_client: Option<services::acrcloud::AcrCloudClient>,
    // Audio analysis
    pub analysis_tx: Option<tokio::sync::mpsc::UnboundedSender<services::audio_analysis::AnalysisJob>>,
    pub audio_analysis_cancel: Arc<AtomicBool>,
    pub audio_analysis_running: Arc<AtomicBool>,
    pub acrcloud_scan_running: Arc<AtomicBool>,
    pub acrcloud_daily_count: Arc<std::sync::atomic::AtomicU32>,
    /// Spotify enrichment progress (visible to status endpoint, survives UI navigation).
    pub spotify_enrich_running: Arc<AtomicBool>,
    pub spotify_enrich_total: Arc<std::sync::atomic::AtomicUsize>,
    pub spotify_enrich_processed: Arc<std::sync::atomic::AtomicUsize>,
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
    /// Shared bearer token for network auth
    pub server_token: String,
}

/// Events broadcast across the application
#[derive(Debug, Clone)]
pub enum AppEvent {
    PlaybackStateChanged,
    LibrarySynced,
    MusicBrainzEnriched,
    TrackChanged { track_id: i64 },
    SyncProgress { service: String, progress: f32 },
    QueueUpdated,
    ListenHistoryUpdated { track_id: i64 },
    PlaybackFailed { message: String },
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
    AudioAnalysisProgress { analyzed: u32, total: u32, mode: String },
    AudioAnalysisComplete { analyzed: u32 },
    // ACRCloud events
    AcrCloudScanProgress { scanned: u32, total: u32, matches_found: u32 },
    AcrCloudScanComplete { scanned: u32, matches_found: u32 },
}

pub type SharedState = Arc<RwLock<AppState>>;

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

    // Resolve DB path: NOOR_DB env var, or noor.db next to the binary (workspace root in dev).
    let db_path = std::env::var("NOOR_DB").unwrap_or_else(|_| {
        std::env::current_exe()
            .ok()
            .and_then(|p| {
                // Binary is at workspace/target/{profile}/noor-server — go up 3 levels.
                p.parent()?
                    .parent()?
                    .parent()
                    .map(|root| root.join("noor.db").to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "noor.db".to_string())
    });
    info!("Database path: {}", db_path);

    // Initialize database
    let db = db::Database::open(&db_path)?;
    db.run_migrations()?;
    let genre_count = db.with_conn(genre::taxonomy::ensure_taxonomy_loaded)?;
    info!("Database initialized");
    info!("Genre taxonomy loaded: {} genres", genre_count);

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
                Ok(bytes) => String::from_utf8(bytes)
                    .ok()
                    .and_then(|json| serde_json::from_str(&json).ok())
                    .inspect(|t: &services::tidal::auth::TidalTokens| {
                        info!("Loaded persisted TIDAL tokens for user {}", t.user_id);
                    }),
                Err(_) => None,
            })
        })
        .unwrap_or(None);

    // Spotify tokens are fetched on demand via the Client Credentials flow
    // using the user-supplied client_id/secret stored in service_auth.extra_data.
    let spotify_tokens: Option<services::spotify::auth::SpotifyTokens> = None;

    // Generate or load the server access token
    let server_token = db
        .with_conn(|conn| db::queries::ensure_server_token(conn))?;
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  NOOR access token: {}", server_token);
    info!("  Copy this into the app on any new device.");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    let http_client = reqwest::Client::new();
    let rss_aggregator = Arc::new(services::rss_feeds::FeedAggregator::new(http_client.clone()));

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
        tidal_tokens,
        spotify_tokens,
        playback_runtime: None,
        playback_runtime_info: None,
        active_listen_session: None,
        external_playback_track: None,
        tidal_login_cancel: Arc::new(AtomicBool::new(false)),
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
        lastfm_enrich_running: Arc::new(AtomicBool::new(false)),
        lastfm_enrich_cancel: Arc::new(AtomicBool::new(false)),
        lastfm_enrich_total: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        lastfm_enrich_processed: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        lastfm_prefetch_total: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        lastfm_prefetch_done: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        lastfm_enrich_started_at: Arc::new(std::sync::atomic::AtomicI64::new(0)),
        server_token,
    }));

    // Check for auto-sync daily services and trigger sync if needed
    {
        let state_read = state.read().await;
        let auto_sync_services = state_read
            .db
            .with_conn(|conn| db::queries::get_auto_sync_services(conn))
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

    // Start HTTP + WebSocket server
    let addr = std::env::var("NOOR_ADDR").unwrap_or_else(|_| "0.0.0.0:3334".to_string());
    info!("Starting server on http://{}", addr);
    server::start(state, &addr).await?;

    Ok(())
}
