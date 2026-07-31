use crate::library::duplicates as dup;
use crate::services::tidal::{auth as tidal_auth, client::TidalClient};
use crate::{AppEvent, SharedState};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;

#[derive(Default)]
pub struct SyncStats {
    pub artists: usize,
    pub albums: usize,
    /// Curated library tracks written (liked + playlist). Background
    /// enrichment fill is counted separately so "tracks synced" keeps meaning
    /// library tracks.
    pub tracks: usize,
    /// Same-recording copies skipped by import dedupe.
    pub duplicates_skipped: usize,
    /// Hidden album-fill rows written by the discovery-enrichment pass.
    pub background_tracks: usize,
    pub playlists: usize,
    pub sync_kind: String,
    pub favorite_artist_cursor: Option<String>,
    pub favorite_album_cursor: Option<String>,
    pub favorite_track_cursor: Option<String>,
}

const FULL_SYNC_INTERVAL_SECS: i64 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SyncModeRequest {
    Auto,
    Full,
    Incremental,
}

impl Default for SyncModeRequest {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncMode {
    Full,
    Incremental,
}

impl SyncMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Incremental => "incremental",
        }
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct TidalSyncQuery {
    mode: Option<SyncModeRequest>,
}

fn normalize_sync_service(service: Option<&str>) -> Result<&'static str, StatusCode> {
    let Some(service) = service else {
        return Ok("tidal");
    };
    let service = service.trim();
    if service.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    if service.eq_ignore_ascii_case("tidal") {
        Ok("tidal")
    } else {
        Err(StatusCode::BAD_REQUEST)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct IncrementalPagePlan {
    process_count: usize,
    hit_cursor: bool,
    newest_created: Option<String>,
}

/// Get sync info (last sync time, auto-sync settings).
pub(super) async fn get_sync_info(
    State(state): State<SharedState>,
    Query(params): Query<serde_json::Map<String, serde_json::Value>>,
) -> Result<Json<Value>, StatusCode> {
    let service = normalize_sync_service(params.get("service").and_then(|v| v.as_str()))?;
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            let info = crate::db::queries::get_sync_info(conn, service)
                .map_err(|e| anyhow::anyhow!("sync info failed: {e}"))?;
            Ok(Json(json!({ "sync": info })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Set auto-sync daily toggle.
#[derive(Debug, Deserialize)]
pub(super) struct AutoSyncRequest {
    service: Option<String>,
    enabled: bool,
}

pub(super) async fn set_auto_sync(
    State(state): State<SharedState>,
    Json(payload): Json<AutoSyncRequest>,
) -> Result<Json<Value>, StatusCode> {
    let service = normalize_sync_service(payload.service.as_deref())?;
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            crate::db::queries::set_auto_sync_daily(conn, service, payload.enabled)
                .map_err(|e| anyhow::anyhow!("set auto sync failed: {e}"))?;
            Ok(Json(
                json!({ "service": service, "auto_sync_daily": payload.enabled }),
            ))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Toggle the discovery-enrichment pass (hidden album-fill import).
pub(super) async fn set_sync_enrichment(
    State(state): State<SharedState>,
    Json(payload): Json<AutoSyncRequest>,
) -> Result<Json<Value>, StatusCode> {
    let service = normalize_sync_service(payload.service.as_deref())?;
    let state = state.read().await;
    state
        .db
        .with_conn(|conn| {
            crate::db::queries::set_enrich_from_favorite_albums(conn, service, payload.enabled)
                .map_err(|e| anyhow::anyhow!("set sync enrichment failed: {e}"))?;
            Ok(Json(json!({
                "service": service,
                "enrich_from_favorite_albums": payload.enabled,
            })))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Reclean the library: demote album-fill that predates the bookmark-only
/// sync to hidden background rows, then run the auto-dedupe pass. Explicitly
/// user-triggered - a shipped install never silently hides library rows.
pub(super) async fn tidal_reclean_library(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let demoted = {
        let s = state.read().await;
        s.db.with_conn(|conn| {
            // Likes, local files, and playlist members stay visible; only
            // un-liked TIDAL fill inside favorited albums goes background.
            Ok(conn.execute(
                "UPDATE tracks SET is_library = 0
                 WHERE is_favorite = 0
                   AND source = 'tidal'
                   AND album_id IN (SELECT id FROM albums WHERE is_favorite = 1)
                   AND id NOT IN (SELECT track_id FROM playlist_tracks)",
                [],
            )?)
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };

    let summary = crate::services::library_dedupe::run_dedupe_pass(&state)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let Some(summary) = summary else {
        // A background pass is mid-flight; the demotion above still applied.
        return Err(StatusCode::CONFLICT);
    };

    // The demotion alone changes the library even when nothing merged.
    {
        let s = state.read().await;
        let _ = s.event_tx.send(AppEvent::LibrarySynced);
    }

    Ok(Json(json!({
        "demoted": demoted,
        "duplicate_groups_found": summary.groups_found,
        "merged_groups": summary.merged_groups,
        "removed_tracks": summary.removed_tracks,
        "skipped_groups": summary.skipped_groups,
    })))
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
    let persisted_tokens = super::load_persisted_tidal_tokens(state).await?;
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
    let result =
        run_tidal_sync_with_reauth(&client, state, tokens, &cancel_flag, SyncModeRequest::Auto)
            .await;
    match result {
        Ok(stats) => {
            // Record sync timestamp
            state.read().await.db.with_conn(|conn| {
                crate::db::queries::update_sync_timestamp_with_metadata(
                    conn,
                    "tidal",
                    stats.tracks as i64,
                    stats.albums as i64,
                    &stats.sync_kind,
                    stats.favorite_artist_cursor.as_deref(),
                    stats.favorite_album_cursor.as_deref(),
                    stats.favorite_track_cursor.as_deref(),
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
pub(super) async fn tidal_sync_library(
    State(state): State<SharedState>,
    Query(params): Query<TidalSyncQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use std::sync::atomic::Ordering;

    // Get tokens and reentrancy/cancel flags
    let persisted_tokens = super::load_persisted_tidal_tokens(&state)
        .await
        .map_err(|error| {
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

    // Reentrancy guard: refuse to start a second concurrent sync.
    if running_flag
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(TidalSyncStartError::AlreadyRunning.into_response());
    }
    cancel_flag.store(false, Ordering::SeqCst);

    // From here on, any early return MUST release the running flag. Wrap the
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
            // setup_guard drops here on early-return path and releases running.
            drop(setup_guard.take());
            error.into_response()
        })?;

    // Hand the guard off to the background task so the flag stays set for the
    // entire sync duration (and is released on completion or panic).
    let task_guard = setup_guard.take().expect("guard still held");

    // Run sync in background
    let state_clone = state.clone();
    let sync_tokens = session.clone();
    let requested_mode = params.mode.unwrap_or_default();
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
        match run_tidal_sync_with_reauth(
            &client,
            &state_clone,
            sync_tokens,
            &cancel_for_task,
            requested_mode,
        )
        .await
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
                    crate::db::queries::update_sync_timestamp_with_metadata(
                        conn,
                        "tidal",
                        stats.tracks as i64,
                        stats.albums as i64,
                        &stats.sync_kind,
                        stats.favorite_artist_cursor.as_deref(),
                        stats.favorite_album_cursor.as_deref(),
                        stats.favorite_track_cursor.as_deref(),
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
/// observes it between pages and returns early. Always returns 200: the
/// frontend uses this idempotently and doesn't care whether a sync was actually
/// running.
pub(super) async fn tidal_sync_cancel(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
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
    requested_mode: SyncModeRequest,
) -> anyhow::Result<SyncStats> {
    use crate::services::tidal::client::TidalClient as TC;
    use std::sync::atomic::Ordering;

    let check_cancel = || -> anyhow::Result<()> {
        if cancel.load(Ordering::SeqCst) {
            anyhow::bail!("TIDAL sync cancelled");
        }
        Ok(())
    };

    let sync_info = {
        let s = state.read().await;
        s.db.with_conn(|conn| Ok(crate::db::queries::get_sync_info(conn, "tidal")?))?
    };
    let sync_mode =
        choose_effective_sync_mode(requested_mode, sync_info.as_ref(), current_unix_epoch());

    let mut stats = SyncStats {
        sync_kind: sync_mode.as_str().to_string(),
        favorite_artist_cursor: sync_info
            .as_ref()
            .and_then(|info| info.tidal_favorite_artist_cursor.clone()),
        favorite_album_cursor: sync_info
            .as_ref()
            .and_then(|info| info.tidal_favorite_album_cursor.clone()),
        favorite_track_cursor: sync_info
            .as_ref()
            .and_then(|info| info.tidal_favorite_track_cursor.clone()),
        ..Default::default()
    };
    let mut favorite_album_ids = HashSet::new();
    let mut favorite_track_ids = HashSet::new();

    // Read previous run's counts so `apply_tidal_favorite_flags` can refuse to
    // wipe favorites if this run somehow returns zero items.
    let (prev_track_count, prev_album_count) = {
        let s = state.read().await;
        s.db.with_conn(|conn| {
            Ok(crate::db::queries::get_sync_info(conn, "tidal")?
                .map(|i| (i.last_sync_track_count, i.last_sync_album_count))
                .unwrap_or((0, 0)))
        })?
    };

    // ── Sync favorite artists ────────────────────────
    tracing::info!("Syncing TIDAL artists...");
    let mut offset = 0;
    let artist_cursor = if matches!(sync_mode, SyncMode::Incremental) {
        stats.favorite_artist_cursor.clone()
    } else {
        None
    };
    loop {
        check_cancel()?;
        let resp = client.get_favorite_artists(user_id, 100, offset).await?;
        if resp.items.is_empty() {
            break;
        }
        let page_plan = plan_incremental_page(&resp.items, artist_cursor.as_deref());
        if page_plan.process_count > 0 {
            advance_cursor(
                &mut stats.favorite_artist_cursor,
                page_plan.newest_created.as_deref(),
            );
        }
        let page_items = &resp.items[..page_plan.process_count];
        let artist_total = resp
            .total_number_of_items
            .unwrap_or((offset + resp.items.len() as i32) as i64)
            .max(1) as f32;
        {
            let s = state.read().await;
            s.db.with_conn(|conn| {
                let tx = conn.unchecked_transaction()?;
                for fav in page_items {
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
        // Artists phase shows up as 0.0 to 0.05: small but non-zero so users see
        // movement during what used to be a silent phase.
        let artist_progress = ((offset as f32 / artist_total) * 0.05).clamp(0.0, 0.05);
        send_tidal_sync_progress(state, artist_progress).await;
        if matches!(sync_mode, SyncMode::Incremental) && page_plan.hit_cursor {
            break;
        }
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
    let album_cursor = if matches!(sync_mode, SyncMode::Incremental) {
        stats.favorite_album_cursor.clone()
    } else {
        None
    };
    loop {
        check_cancel()?;
        let resp = client.get_favorite_albums(user_id, 100, offset).await?;
        if resp.items.is_empty() {
            break;
        }
        let page_plan = plan_incremental_page(&resp.items, album_cursor.as_deref());
        if page_plan.process_count > 0 {
            advance_cursor(
                &mut stats.favorite_album_cursor,
                page_plan.newest_created.as_deref(),
            );
        }
        let page_items = &resp.items[..page_plan.process_count];
        // Batch the page's album upserts in one transaction.
        {
            let s = state.read().await;
            s.db.with_conn(|conn| {
                let tx = conn.unchecked_transaction()?;
                for fav in page_items {
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
        for fav in page_items {
            stats.albums += 1;
            favorite_album_ids.insert(fav.item.id);
        }

        // Albums are bookmarks only: the row above keeps the shelf and
        // metadata intact, but the album's tracks are NOT pulled into the
        // library any more. The discovery-enrichment pass (after the sync,
        // when enabled) imports them as hidden is_library=0 rows instead.
        let album_total = resp
            .total_number_of_items
            .unwrap_or((offset + resp.items.len() as i32) as i64)
            .max(1) as f32;

        offset += resp.items.len() as i32;
        // Albums phase: 0.05 to 0.10. Artists phase ate 0.0 to 0.05.
        let album_progress = (0.05 + (offset as f32 / album_total) * 0.05).clamp(0.05, 0.10);
        send_tidal_sync_progress(state, album_progress).await;
        if matches!(sync_mode, SyncMode::Incremental) && page_plan.hit_cursor {
            break;
        }
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
    let track_cursor = if matches!(sync_mode, SyncMode::Incremental) {
        stats.favorite_track_cursor.clone()
    } else {
        None
    };
    loop {
        check_cancel()?;
        let resp = client.get_favorite_tracks(user_id, 100, offset).await?;
        if resp.items.is_empty() {
            break;
        }
        let page_plan = plan_incremental_page(&resp.items, track_cursor.as_deref());
        if page_plan.process_count > 0 {
            advance_cursor(
                &mut stats.favorite_track_cursor,
                page_plan.newest_created.as_deref(),
            );
        }
        let page_items = &resp.items[..page_plan.process_count];
        {
            let s = state.read().await;
            let mut page_inserted = 0usize;
            let mut page_skipped = 0usize;
            s.db.with_conn(|conn| {
                let tx = conn.unchecked_transaction()?;
                for fav in page_items {
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
                    // Same-recording dedupe: a copy of this liked track may
                    // already exist under another tidal_id (single vs album
                    // release, enrichment fill). Transfer the like to that
                    // row instead of inserting a visible duplicate.
                    let incoming = dup::IncomingTrack {
                        tidal_id: track.id,
                        title: &track.title,
                        artist_name: &track.artist.name,
                        isrc: track.isrc.as_deref(),
                        duration_ms: track.duration * 1000,
                    };
                    let candidates = dup::fetch_import_candidates(
                        &tx,
                        track.id,
                        track.artist.id,
                        track.isrc.as_deref(),
                        incoming.duration_ms,
                    )?;
                    match dup::decide_import(&incoming, &candidates) {
                        dup::ImportDecision::Insert => {
                            super::insert_tidal_track(&tx, track, true, true, fav.created.as_deref())?;
                            page_inserted += 1;
                        }
                        dup::ImportDecision::SkipDuplicate {
                            existing_track_id,
                            existing_tidal_id,
                        } => {
                            tx.execute(
                                "UPDATE tracks SET is_favorite = 1, is_library = 1 WHERE id = ?1",
                                rusqlite::params![existing_track_id],
                            )?;
                            // Keep the transferred like stable across the
                            // Full-mode reconciliation, which resets and
                            // re-sets is_favorite by tidal_id.
                            if let Some(existing_tidal_id) = existing_tidal_id {
                                favorite_track_ids.insert(existing_tidal_id);
                            }
                            page_skipped += 1;
                        }
                    }
                }
                tx.commit()?;
                Ok(())
            })?;
            stats.tracks += page_inserted;
            stats.duplicates_skipped += page_skipped;
        }
        offset += resp.items.len() as i32;
        let processed_tracks = offset as f32;
        // Liked tracks phase: 0.10 to 0.45.
        let track_progress = resp
            .total_number_of_items
            .map(|t| 0.10 + (processed_tracks / t.max(1) as f32) * 0.35)
            .unwrap_or(0.40)
            .clamp(0.10, 0.45);
        send_tidal_sync_progress(state, track_progress).await;
        if matches!(sync_mode, SyncMode::Incremental) && page_plan.hit_cursor {
            break;
        }
        if resp
            .total_number_of_items
            .is_none_or(|t| offset as i64 >= t)
        {
            break;
        }
    }
    tracing::info!("Synced {} tracks total", stats.tracks);

    // ── Sync playlists ───────────────────────────────
    //
    // Runs on incremental syncs too. It used to be gated on `SyncMode::Full`,
    // which meant playlists only refreshed once a week (Auto resolves to
    // Incremental for FULL_SYNC_INTERVAL_SECS after a full sync), so tracks
    // added to a playlist in the TIDAL app never showed up here. The index
    // fetch below is one request per 100 playlists, and `playlist_needs_pull`
    // keeps the expensive part - a page of tracks per playlist - to the
    // playlists that actually changed.
    {
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
            let remote_last_updated = normalize_tidal_timestamp(playlist.last_updated.as_deref());
            let remote_count = playlist.number_of_tracks.unwrap_or(0);

            // Upsert the playlist row up front so metadata sticks even if the
            // track-fetch errors out partway.
            //
            // This was an INSERT OR REPLACE, which conflicts on `tidal_uuid
            // UNIQUE` and therefore DELETEd the existing row and inserted a new
            // one on every full sync: a fresh `id`, `is_favorite` reset to 0,
            // `created_at` reset, and - because foreign keys are ON - the whole
            // playlist's `playlist_tracks` cascade-deleted. Smart rules that
            // reference a playlist id (`not_in_playlist`) silently stopped
            // matching. ON CONFLICT DO UPDATE touches only the columns TIDAL
            // owns and leaves the row's identity alone.
            let (pid, needs_pull) = {
                let s = state.read().await;
                s.db.with_conn(|conn| {
                    let previous: Option<(i64, Option<String>, i64)> = conn
                        .query_row(
                            "SELECT id, tidal_last_updated, track_count FROM playlists WHERE tidal_uuid = ?1",
                            rusqlite::params![playlist.uuid],
                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                        )
                        .ok();
                    conn.execute(
                        "INSERT INTO playlists (tidal_uuid, name, description, track_count, tidal_last_updated, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, COALESCE(?5, datetime('now')))
                         ON CONFLICT(tidal_uuid) DO UPDATE SET
                           name = excluded.name,
                           description = excluded.description,
                           track_count = excluded.track_count,
                           tidal_last_updated = excluded.tidal_last_updated,
                           updated_at = COALESCE(excluded.tidal_last_updated, playlists.updated_at)",
                        rusqlite::params![
                            playlist.uuid,
                            playlist.title,
                            playlist.description,
                            remote_count,
                            remote_last_updated,
                        ],
                    )?;
                    let id: i64 = conn.query_row(
                        "SELECT id FROM playlists WHERE tidal_uuid = ?1",
                        rusqlite::params![playlist.uuid],
                        |row| row.get(0),
                    )?;
                    let needs_pull = matches!(sync_mode, SyncMode::Full)
                        || playlist_needs_pull(
                            previous.as_ref().map(|(_, seen, count)| (seen.as_deref(), *count)),
                            remote_last_updated.as_deref(),
                            remote_count as i64,
                        );
                    Ok((id, needs_pull))
                })?
            };

            if needs_pull {
                let tracks =
                    fetch_tidal_playlist_tracks(client, &playlist.uuid, &check_cancel).await?;
                let s = state.read().await;
                s.db.with_conn(|conn| replace_playlist_tracks(conn, pid, &tracks))?;
            }
            stats.playlists += 1;
            // Playlists phase: 0.45 to 0.55; enrichment takes 0.55 to 0.99.
            let playlist_progress =
                0.45 + (((playlist_index + 1) as f32 / total_playlists as f32) * 0.10);
            send_tidal_sync_progress(state, playlist_progress.clamp(0.45, 0.55)).await;
        }

        // Drop local mirrors of playlists the user deleted on TIDAL. Full only:
        // an incremental run can legitimately see a short list if the index
        // fetch was truncated, and deleting on that basis is unrecoverable.
        // Local and smart playlists have a NULL tidal_uuid and are never touched.
        if matches!(sync_mode, SyncMode::Full) {
            let remote_uuids: Vec<String> = all_playlists.iter().map(|p| p.uuid.clone()).collect();
            let s = state.read().await;
            let removed = s.db.with_conn(|conn| {
                let local: Vec<(i64, String)> = {
                    let mut stmt = conn.prepare(
                        "SELECT id, tidal_uuid FROM playlists WHERE tidal_uuid IS NOT NULL",
                    )?;
                    stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                        .collect::<Result<_, _>>()?
                };
                let keep: std::collections::HashSet<&str> =
                    remote_uuids.iter().map(String::as_str).collect();
                let mut removed = 0usize;
                for (id, uuid) in local {
                    if !keep.contains(uuid.as_str()) {
                        conn.execute("DELETE FROM playlists WHERE id = ?1", rusqlite::params![id])?;
                        removed += 1;
                    }
                }
                Ok(removed)
            })?;
            if removed > 0 {
                tracing::info!("Removed {removed} playlists no longer on TIDAL");
            }
        }
    }
    tracing::info!("Synced {} playlists", stats.playlists);
    {
        let s = state.read().await;
        let _ = s.event_tx.send(crate::AppEvent::PlaylistsChanged);
    }

    if sync_mode_reconciles_favorites(sync_mode) {
        let s = state.read().await;
        s.db.with_conn(|conn| {
            super::apply_tidal_favorite_flags(
                conn,
                "albums",
                &favorite_album_ids,
                prev_album_count,
            )?;
            super::apply_tidal_favorite_flags(
                conn,
                "tracks",
                &favorite_track_ids,
                prev_track_count,
            )?;
            Ok(())
        })?;
    }

    // Discovery enrichment: import the rest of each favorited album's tracks
    // as hidden background rows for radio/similarity. Best-effort: a failure
    // (or cancel) here must not fail the completed sync - unfinished albums
    // keep enrich_completed_at NULL and are picked up next run.
    let enrich_enabled = sync_info
        .as_ref()
        .map(|info| info.enrich_from_favorite_albums)
        .unwrap_or(true);
    if enrich_enabled {
        if let Err(e) = run_favorite_album_enrichment(client, state, cancel, &mut stats).await {
            tracing::warn!("Discovery enrichment stopped early: {e}");
        }
    }
    send_tidal_sync_progress(state, 0.99).await;

    Ok(stats)
}

/// Import the rest of each favorited album's tracks as hidden discovery fill
/// (is_library = 0): invisible in the Library grid and Genre Galaxy, but
/// feeding radio, similarity, and DiscoverSpace. DB-driven so it also covers
/// albums favorited before this feature existed, albums synced while the
/// toggle was off, and interrupted runs. Same-recording copies are skipped
/// via decide_import; variants (remix/live/...) are kept.
async fn run_favorite_album_enrichment(
    client: &TidalClient,
    state: &SharedState,
    cancel: &std::sync::atomic::AtomicBool,
    stats: &mut SyncStats,
) -> anyhow::Result<()> {
    use futures::stream::{self, StreamExt};
    use std::sync::atomic::Ordering;

    let check_cancel = || -> anyhow::Result<()> {
        if cancel.load(Ordering::SeqCst) {
            anyhow::bail!("TIDAL sync cancelled");
        }
        Ok(())
    };

    let album_ids: Vec<i64> = {
        let s = state.read().await;
        s.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT tidal_id FROM albums
                 WHERE is_favorite = 1
                   AND tidal_id IS NOT NULL
                   AND enrich_completed_at IS NULL
                 ORDER BY id DESC",
            )?;
            let ids = stmt
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<i64>>>()?;
            Ok(ids)
        })?
    };
    if album_ids.is_empty() {
        return Ok(());
    }

    tracing::info!(
        "Enriching {} favorited albums with hidden discovery fill...",
        album_ids.len()
    );
    let total = album_ids.len().max(1) as f32;
    let mut processed = 0usize;

    for album_chunk in album_ids.chunks(10) {
        check_cancel()?;
        // Bound each per-album fetch so a single hung TIDAL request can't
        // stall the chunk; one retry handles transient blips (same pattern
        // the old in-sync hydration used).
        let album_fetch_timeout = std::time::Duration::from_secs(15);
        let mut fetches = stream::iter(album_chunk.iter().copied())
            .map(|album_id| async move {
                let first = tokio::time::timeout(
                    album_fetch_timeout,
                    client.get_all_album_tracks(album_id),
                )
                .await;
                let result = match first {
                    Ok(Ok(resp)) => Ok(resp),
                    _ => match tokio::time::timeout(
                        album_fetch_timeout,
                        client.get_all_album_tracks(album_id),
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(_) => Err(anyhow::anyhow!(
                            "get_album_tracks timed out twice for album {album_id}"
                        )),
                    },
                };
                (album_id, result)
            })
            .buffer_unordered(10);

        while let Some((album_id, result)) = fetches.next().await {
            if let Ok(tracks) = result {
                let s = state.read().await;
                let mut inserted = 0usize;
                let mut skipped = 0usize;
                s.db.with_conn(|conn| {
                    let tx = conn.unchecked_transaction()?;
                    for track in &tracks {
                        let incoming = dup::IncomingTrack {
                            tidal_id: track.id,
                            title: &track.title,
                            artist_name: &track.artist.name,
                            isrc: track.isrc.as_deref(),
                            duration_ms: track.duration * 1000,
                        };
                        let candidates = dup::fetch_import_candidates(
                            &tx,
                            track.id,
                            track.artist.id,
                            track.isrc.as_deref(),
                            incoming.duration_ms,
                        )?;
                        match dup::decide_import(&incoming, &candidates) {
                            dup::ImportDecision::Insert => {
                                super::insert_tidal_track(&tx, track, false, false, None)?;
                                inserted += 1;
                            }
                            dup::ImportDecision::SkipDuplicate { .. } => {
                                skipped += 1;
                            }
                        }
                    }
                    // Mark done inside the same tx: a crash re-runs the whole
                    // album (idempotent upserts), never half-marks it.
                    tx.execute(
                        "UPDATE albums SET enrich_completed_at = datetime('now') WHERE tidal_id = ?1",
                        rusqlite::params![album_id],
                    )?;
                    tx.commit()?;
                    Ok(())
                })?;
                stats.background_tracks += inserted;
                stats.duplicates_skipped += skipped;
            }

            processed += 1;
            // Enrichment phase: 0.55 to 0.99.
            let progress = (0.55 + (processed as f32 / total) * 0.44).clamp(0.55, 0.99);
            send_tidal_sync_progress(state, progress).await;
        }
    }

    tracing::info!(
        "Enrichment complete: {} hidden tracks added, {} duplicate copies skipped",
        stats.background_tracks,
        stats.duplicates_skipped
    );
    Ok(())
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
    requested_mode: SyncModeRequest,
) -> anyhow::Result<SyncStats> {
    match do_tidal_sync(client, state, &tokens.user_id, cancel, requested_mode).await {
        Ok(stats) => Ok(stats),
        Err(err) if super::error_looks_like_auth(&err) => {
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

            let refreshed = match super::recover_tidal_session(state, &http, &tokens).await {
                Ok(tokens) => tokens,
                Err(recover_err) => {
                    // Do NOT clear the session. A transient network error during refresh
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
            do_tidal_sync(
                &retry_client,
                state,
                &refreshed.user_id,
                cancel,
                requested_mode,
            )
            .await
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
        Err(err) if super::error_looks_like_auth(&err) => {
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
            match super::recover_tidal_session(state, &http, tokens).await {
                Ok(tokens) => Ok((tokens, TidalSyncSessionState::Recovered)),
                Err(recover_err) => {
                    // Do NOT clear. A transient refresh failure should not log the user out.
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

fn choose_effective_sync_mode(
    requested_mode: SyncModeRequest,
    sync_info: Option<&crate::db::queries::SyncInfo>,
    now_epoch: i64,
) -> SyncMode {
    match requested_mode {
        SyncModeRequest::Full => SyncMode::Full,
        SyncModeRequest::Incremental => {
            if sync_info.is_some_and(|info| sync_info_ready_for_incremental(info, now_epoch)) {
                SyncMode::Incremental
            } else {
                SyncMode::Full
            }
        }
        SyncModeRequest::Auto => {
            if sync_info.is_some_and(|info| sync_info_ready_for_incremental(info, now_epoch)) {
                SyncMode::Incremental
            } else {
                SyncMode::Full
            }
        }
    }
}

fn sync_info_ready_for_incremental(info: &crate::db::queries::SyncInfo, now_epoch: i64) -> bool {
    let Some(last_full_sync_at) = info.last_full_sync_at.as_deref() else {
        return false;
    };
    if info.tidal_favorite_artist_cursor.is_none()
        || info.tidal_favorite_album_cursor.is_none()
        || info.tidal_favorite_track_cursor.is_none()
    {
        return false;
    }
    let Some(last_full_epoch) = parse_sync_epoch(last_full_sync_at) else {
        return false;
    };
    now_epoch.saturating_sub(last_full_epoch) <= FULL_SYNC_INTERVAL_SECS
}

fn parse_sync_epoch(value: &str) -> Option<i64> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(value) {
        return Some(dt.timestamp());
    }
    chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|dt| dt.and_utc().timestamp())
}

fn current_unix_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn plan_incremental_page<T>(
    items: &[crate::services::tidal::client::FavoriteItem<T>],
    cursor: Option<&str>,
) -> IncrementalPagePlan {
    let mut process_count = 0;
    let mut hit_cursor = false;
    let mut newest_created = None;

    for item in items {
        let Some(created) = item.created.as_deref() else {
            process_count += 1;
            continue;
        };
        if cursor.is_some_and(|cursor| created <= cursor) {
            hit_cursor = true;
            break;
        }
        if newest_created.is_none() {
            newest_created = Some(created.to_string());
        }
        process_count += 1;
    }

    IncrementalPagePlan {
        process_count,
        hit_cursor,
        newest_created,
    }
}

fn advance_cursor(cursor: &mut Option<String>, candidate: Option<&str>) {
    let Some(candidate) = candidate else {
        return;
    };
    if cursor
        .as_deref()
        .is_none_or(|existing| candidate > existing)
    {
        *cursor = Some(candidate.to_string());
    }
}

fn sync_mode_reconciles_favorites(sync_mode: SyncMode) -> bool {
    matches!(sync_mode, SyncMode::Full)
}

/// Page through every track in a TIDAL playlist.
///
/// Deliberately collects the whole list before any write happens: if the API
/// errors or the user cancels part way, the caller never gets to the DELETE and
/// the existing playlist contents stay intact.
pub(super) async fn fetch_tidal_playlist_tracks<F>(
    client: &crate::services::tidal::client::TidalClient,
    playlist_uuid: &str,
    check_cancel: &F,
) -> anyhow::Result<Vec<crate::services::tidal::client::TidalTrack>>
where
    F: Fn() -> anyhow::Result<()> + Sync,
{
    let mut all_tracks: Vec<crate::services::tidal::client::TidalTrack> = Vec::new();
    let mut track_offset = 0;
    loop {
        check_cancel()?;
        let tracks_resp = client
            .get_playlist_tracks(playlist_uuid, 100, track_offset)
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
    Ok(all_tracks)
}

/// Replace a playlist's contents with `tracks`, in one transaction, importing
/// any artists/albums/tracks that are not local yet. Returns the number of rows
/// that actually landed.
///
/// Shared by the library sync and the per-playlist refresh route so both write
/// playlists exactly the same way.
pub(super) fn replace_playlist_tracks(
    conn: &rusqlite::Connection,
    playlist_id: i64,
    tracks: &[crate::services::tidal::client::TidalTrack],
) -> anyhow::Result<i64> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM playlist_tracks WHERE playlist_id=?1",
        rusqlite::params![playlist_id],
    )?;
    let mut position = 0i64;
    for track in tracks {
        tx.execute(
            "INSERT INTO artists (tidal_id, name) VALUES (?1, ?2) ON CONFLICT(tidal_id) DO UPDATE SET name=excluded.name",
            rusqlite::params![track.artist.id, track.artist.name],
        )?;
        if let Some(ref album_ref) = track.album {
            let artwork =
                crate::services::tidal::client::TidalClient::get_artwork_url(&album_ref.cover, 640);
            tx.execute(
                "INSERT OR IGNORE INTO albums (tidal_id, title, artist_id, artwork_url, is_favorite, source)
                 VALUES (?1, ?2, (SELECT id FROM artists WHERE tidal_id=?3), ?4, 0, 'tidal')",
                rusqlite::params![album_ref.id, album_ref.title, track.artist.id, artwork],
            )?;
        }
        // insert_tidal_track already resolves the local row id, so reuse it
        // instead of issuing a second lookup per track. Playlist members are
        // curated (is_library=1) and are NOT deduped: playlist_tracks needs a
        // concrete row per position, and tidal_id conflicts already upsert.
        let track_id = super::insert_tidal_track(&tx, track, false, true, None)?;
        if let Some(tid) = track_id {
            tx.execute(
                "INSERT OR IGNORE INTO playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
                rusqlite::params![playlist_id, tid, position],
            )?;
            position += 1;
        }
    }
    // TIDAL's numberOfTracks counts rows we may not have been able to resolve
    // locally, so trust what actually landed. Without this the count on the card
    // disagrees with the list under it.
    tx.execute(
        "UPDATE playlists SET track_count = ?2 WHERE id = ?1",
        rusqlite::params![playlist_id, position],
    )?;
    tx.commit()?;
    Ok(position)
}

/// Reduce a TIDAL ISO8601 timestamp to SQLite's `YYYY-MM-DD HH:MM:SS` form.
///
/// `playlists.updated_at` is written by `datetime('now')` everywhere else, and
/// the "Last updated" sort is a plain string comparison. Storing TIDAL's
/// `2026-07-30T11:22:33.444+0000` alongside those would sort every TIDAL
/// playlist above every local one, because 'T' > ' '. Anything that does not
/// look like a timestamp is dropped rather than stored badly.
fn normalize_tidal_timestamp(raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim();
    if raw.len() < 19 {
        return None;
    }
    let (date, rest) = raw.split_at(10);
    let time = &rest[1..9];
    let separator_ok = matches!(rest.as_bytes().first(), Some(b'T') | Some(b' '));
    let shaped = date.as_bytes()[4] == b'-'
        && date.as_bytes()[7] == b'-'
        && time.as_bytes()[2] == b':'
        && time.as_bytes()[5] == b':';
    if !separator_ok || !shaped {
        return None;
    }
    Some(format!("{date} {time}"))
}

/// Whether a playlist's tracks need re-fetching from TIDAL.
///
/// `previous` is the locally stored `(tidal_last_updated, track_count)`, absent
/// for a playlist we have never seen. Re-pull whenever TIDAL reports a change,
/// whenever the counts disagree, and whenever either side's timestamp is
/// missing - an unknown is not evidence that nothing changed.
fn playlist_needs_pull(
    previous: Option<(Option<&str>, i64)>,
    remote_last_updated: Option<&str>,
    remote_track_count: i64,
) -> bool {
    let Some((local_last_updated, local_track_count)) = previous else {
        return true;
    };
    let (Some(local), Some(remote)) = (local_last_updated, remote_last_updated) else {
        return true;
    };
    local != remote || local_track_count != remote_track_count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::SyncInfo;
    use crate::services::tidal::client::TidalArtist;
    use std::collections::HashMap;

    fn sync_info(
        last_full_sync_at: Option<&str>,
        artist_cursor: Option<&str>,
        album_cursor: Option<&str>,
        track_cursor: Option<&str>,
    ) -> SyncInfo {
        SyncInfo {
            service: "tidal".to_string(),
            last_sync_at: "2026-05-10 00:00:00".to_string(),
            auto_sync_daily: false,
            enrich_from_favorite_albums: true,
            last_sync_track_count: 0,
            last_sync_album_count: 0,
            last_full_sync_at: last_full_sync_at.map(str::to_string),
            last_sync_kind: None,
            tidal_favorite_artist_cursor: artist_cursor.map(str::to_string),
            tidal_favorite_album_cursor: album_cursor.map(str::to_string),
            tidal_favorite_track_cursor: track_cursor.map(str::to_string),
        }
    }

    #[test]
    fn auto_mode_uses_incremental_after_recent_full_sync_with_cursors() {
        let now = parse_sync_epoch("2026-05-12 00:00:00").unwrap();
        let info = sync_info(
            Some("2026-05-10 00:00:00"),
            Some("2026-05-10T01:00:00Z"),
            Some("2026-05-10T02:00:00Z"),
            Some("2026-05-10T03:00:00Z"),
        );

        assert_eq!(
            choose_effective_sync_mode(SyncModeRequest::Auto, Some(&info), now),
            SyncMode::Incremental
        );
    }

    #[test]
    fn auto_mode_uses_incremental_when_full_sync_is_exactly_seven_days_old() {
        let now = parse_sync_epoch("2026-05-12 00:00:00").unwrap();
        let info = sync_info(
            Some("2026-05-05 00:00:00"),
            Some("2026-05-10T01:00:00Z"),
            Some("2026-05-10T02:00:00Z"),
            Some("2026-05-10T03:00:00Z"),
        );

        assert_eq!(
            choose_effective_sync_mode(SyncModeRequest::Auto, Some(&info), now),
            SyncMode::Incremental
        );
    }

    #[test]
    fn auto_mode_uses_full_without_recent_full_sync_or_cursors() {
        let now = parse_sync_epoch("2026-05-12 00:00:00").unwrap();
        let stale = sync_info(
            Some("2026-05-01 00:00:00"),
            Some("2026-05-10T01:00:00Z"),
            Some("2026-05-10T02:00:00Z"),
            Some("2026-05-10T03:00:00Z"),
        );
        let missing_cursor = sync_info(
            Some("2026-05-10 00:00:00"),
            Some("2026-05-10T01:00:00Z"),
            None,
            Some("2026-05-10T03:00:00Z"),
        );

        assert_eq!(
            choose_effective_sync_mode(SyncModeRequest::Auto, None, now),
            SyncMode::Full
        );
        assert_eq!(
            choose_effective_sync_mode(SyncModeRequest::Auto, Some(&stale), now),
            SyncMode::Full
        );
        assert_eq!(
            choose_effective_sync_mode(SyncModeRequest::Auto, Some(&missing_cursor), now),
            SyncMode::Full
        );
    }

    #[test]
    fn explicit_incremental_falls_back_to_full_without_cursors() {
        let now = parse_sync_epoch("2026-05-12 00:00:00").unwrap();
        let missing_cursor = sync_info(
            Some("2026-05-10 00:00:00"),
            Some("2026-05-10T01:00:00Z"),
            None,
            Some("2026-05-10T03:00:00Z"),
        );

        assert_eq!(
            choose_effective_sync_mode(SyncModeRequest::Incremental, Some(&missing_cursor), now),
            SyncMode::Full
        );
    }

    #[test]
    fn incremental_page_processing_stops_at_cursor() {
        let page = vec![
            favorite_artist(1, "2026-05-12T10:00:00Z"),
            favorite_artist(2, "2026-05-11T10:00:00Z"),
            favorite_artist(3, "2026-05-10T10:00:00Z"),
        ];

        let plan = plan_incremental_page(&page, Some("2026-05-11T10:00:00Z"));

        assert_eq!(plan.process_count, 1);
        assert!(plan.hit_cursor);
        assert_eq!(plan.newest_created.as_deref(), Some("2026-05-12T10:00:00Z"));
    }

    #[test]
    fn full_mode_reconciles_favorites_but_incremental_mode_does_not() {
        assert!(sync_mode_reconciles_favorites(SyncMode::Full));
        assert!(!sync_mode_reconciles_favorites(SyncMode::Incremental));
    }

    #[test]
    fn tidal_timestamps_normalize_to_sqlite_datetime_form() {
        // The sort compares `updated_at` as a string, so the separator matters:
        // 'T' sorts after ' ', which would float every TIDAL playlist to the top.
        assert_eq!(
            normalize_tidal_timestamp(Some("2026-07-30T11:22:33.444+0000")).as_deref(),
            Some("2026-07-30 11:22:33")
        );
        assert_eq!(
            normalize_tidal_timestamp(Some("2026-07-30T11:22:33Z")).as_deref(),
            Some("2026-07-30 11:22:33")
        );
        // Already-normalized input passes through unchanged.
        assert_eq!(
            normalize_tidal_timestamp(Some("2026-07-30 11:22:33")).as_deref(),
            Some("2026-07-30 11:22:33")
        );
    }

    #[test]
    fn malformed_tidal_timestamps_are_dropped_rather_than_stored() {
        assert_eq!(normalize_tidal_timestamp(None), None);
        assert_eq!(normalize_tidal_timestamp(Some("")), None);
        assert_eq!(normalize_tidal_timestamp(Some("2026-07-30")), None);
        assert_eq!(
            normalize_tidal_timestamp(Some("not a timestamp at all")),
            None
        );
    }

    #[test]
    fn unchanged_playlists_skip_the_track_pull() {
        assert!(!playlist_needs_pull(
            Some((Some("2026-07-30 11:22:33"), 42)),
            Some("2026-07-30 11:22:33"),
            42
        ));
    }

    #[test]
    fn changed_playlists_are_pulled() {
        // A newer remote timestamp is the normal "user added a track" case.
        assert!(playlist_needs_pull(
            Some((Some("2026-07-30 11:22:33"), 42)),
            Some("2026-07-31 09:00:00"),
            43
        ));
        // Counts disagreeing is enough on its own: TIDAL does not always move
        // lastUpdated, and a partially-resolved previous pull leaves us short.
        assert!(playlist_needs_pull(
            Some((Some("2026-07-30 11:22:33"), 41)),
            Some("2026-07-30 11:22:33"),
            42
        ));
    }

    #[test]
    fn unknown_state_always_pulls() {
        // Never seen before.
        assert!(playlist_needs_pull(None, Some("2026-07-30 11:22:33"), 42));
        // Stored without a timestamp (rows that predate migration 061).
        assert!(playlist_needs_pull(
            Some((None, 42)),
            Some("2026-07-30 11:22:33"),
            42
        ));
        // TIDAL did not send one. An unknown is not evidence nothing changed.
        assert!(playlist_needs_pull(
            Some((Some("2026-07-30 11:22:33"), 42)),
            None,
            42
        ));
    }

    fn favorite_artist(
        id: i64,
        created: &str,
    ) -> crate::services::tidal::client::FavoriteItem<TidalArtist> {
        crate::services::tidal::client::FavoriteItem {
            item: TidalArtist {
                id,
                name: format!("Artist {id}"),
                picture: None,
                extra: HashMap::new(),
            },
            created: Some(created.to_string()),
        }
    }
}
