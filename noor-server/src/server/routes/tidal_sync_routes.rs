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
    pub tracks: usize,
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
    let service = params
        .get("service")
        .and_then(|v| v.as_str())
        .unwrap_or("tidal");
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
    let service = payload.service.as_deref().unwrap_or("tidal");
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
    use futures::stream::{self, StreamExt};
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

        // Hydrate album tracks with bounded concurrency so the UI keeps moving
        // instead of stalling on one giant page-wide batch.
        let album_ids: Vec<i64> = page_items.iter().map(|f| f.item.id).collect();
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
                        client.get_all_album_tracks(album_id),
                    )
                    .await;
                    match first {
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
                    }
                })
                .buffer_unordered(10);

            while let Some(result) = fetches.next().await {
                if let Ok(tracks) = result {
                    let s = state.read().await;
                    s.db.with_conn(|conn| {
                        let tx = conn.unchecked_transaction()?;
                        for track in &tracks {
                            super::insert_tidal_track(&tx, track, false, None)?;
                            stats.tracks += 1;
                        }
                        tx.commit()?;
                        Ok(())
                    })?;
                }

                albums_hydrated_in_page += 1;
                let processed_albums = offset as usize + albums_hydrated_in_page;
                // Albums phase: 0.05 to 0.5. Artists phase ate 0.0 to 0.05.
                let progress_fraction =
                    (0.05 + (processed_albums as f32 / album_total) * 0.45).clamp(0.05, 0.5);
                send_tidal_sync_progress(state, progress_fraction).await;
            }
        }

        offset += resp.items.len() as i32;
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
                    super::insert_tidal_track(&tx, track, true, fav.created.as_deref())?;
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
    if matches!(sync_mode, SyncMode::Full) {
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
                    // insert_tidal_track already resolves the local row id, so
                    // reuse it instead of issuing a second lookup per track.
                    let track_id = super::insert_tidal_track(&tx, track, false, None)?;
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
    }
    tracing::info!("Synced {} playlists", stats.playlists);

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
