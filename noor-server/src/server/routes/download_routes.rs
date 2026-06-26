//! HTTP surface + background worker for downloading tracks to disk as FLAC/MP3.
//!
//! The pure encode/decode engine lives in [`crate::services::download`]. This module
//! owns the orchestration that needs server context: the sequential worker that drains
//! the unified download queue, TIDAL token refresh on session expiry, and the routes.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::AppEvent;
use crate::SharedState;
use crate::services::download::{
    self, DownloadError, DownloadFormat, DownloadJobItem, DownloadOutcome, DownloadStatus,
};

// ─── Worker ──────────────────────────────────────────────────────────────────────

async fn broadcast(state: &SharedState, event: AppEvent) {
    let _ = state.read().await.event_tx.send(event);
}

async fn broadcast_progress(state: &SharedState) {
    let (done, total, current_title) = state.read().await.downloads.progress();
    broadcast(
        state,
        AppEvent::DownloadProgress {
            done,
            total,
            current_title,
        },
    )
    .await;
}

/// Refresh the TIDAL access token (reusing the server's refresh+persist path) and
/// return the new access token, or `None` if refresh failed.
async fn refresh_access_token(state: &SharedState) -> Option<String> {
    let (http, tokens) = {
        let s = state.read().await;
        (s.http_client.clone(), s.tidal_tokens.clone())
    };
    let tokens = tokens?;
    match super::recover_tidal_session(state, &http, &tokens).await {
        Ok(refreshed) => Some(refreshed.access_token),
        Err(e) => {
            tracing::warn!(target = "noor.download", "TIDAL token refresh failed: {e}");
            None
        }
    }
}

/// Download one track with the agreed retry policy: refresh + retry once on session
/// expiry, retry once on a transient (network) error, fail immediately otherwise.
async fn attempt_download(
    state: &SharedState,
    http_client: &reqwest::Client,
    access_token: &str,
    track: &crate::db::models::Track,
    dest_root: &std::path::Path,
    format: DownloadFormat,
) -> Result<DownloadOutcome, DownloadError> {
    match download::download_track(http_client, access_token, track, dest_root, format).await {
        Ok(outcome) => Ok(outcome),
        Err(DownloadError::SessionExpired) => match refresh_access_token(state).await {
            Some(new_token) => {
                download::download_track(http_client, &new_token, track, dest_root, format).await
            }
            None => Err(DownloadError::SessionExpired),
        },
        Err(e) if e.is_transient() => {
            download::download_track(http_client, access_token, track, dest_root, format).await
        }
        Err(e) => Err(e),
    }
}

/// Drain the unified download queue sequentially until it's empty, broadcasting
/// progress + per-item completion as it goes.
async fn run_download_worker(state: SharedState) {
    let manager = { state.read().await.downloads.clone() };

    while let Some(item) = manager.next_item() {
        let track = {
            let s = state.read().await;
            s.db.with_conn(|conn| crate::playback::queue::get_track_by_id(conn, item.track_id))
                .ok()
                .flatten()
        };

        let Some(track) = track else {
            let reason = "Track not found".to_string();
            manager.record_failure(
                item.track_id,
                format!("Track {}", item.track_id),
                reason.clone(),
            );
            broadcast(
                &state,
                AppEvent::DownloadItemDone {
                    track_id: item.track_id,
                    ok: false,
                    already: false,
                    path: None,
                    error: Some(reason),
                },
            )
            .await;
            broadcast_progress(&state).await;
            continue;
        };

        manager.set_current(Some(track.title.clone()));
        broadcast_progress(&state).await;

        let (http_client, token_opt, dest_root) = {
            let s = state.read().await;
            let dest =
                s.db.with_conn(|conn| Ok(download::read_download_folder(conn)))
                    .unwrap_or_else(|_| download::default_download_folder());
            (
                s.http_client.clone(),
                s.tidal_tokens.as_ref().map(|t| t.access_token.clone()),
                dest,
            )
        };

        let Some(access_token) = token_opt else {
            let reason = "Not signed in to TIDAL".to_string();
            manager.record_failure(track.id, track.title.clone(), reason.clone());
            broadcast(
                &state,
                AppEvent::DownloadItemDone {
                    track_id: track.id,
                    ok: false,
                    already: false,
                    path: None,
                    error: Some(reason),
                },
            )
            .await;
            broadcast_progress(&state).await;
            continue;
        };

        match attempt_download(
            &state,
            &http_client,
            &access_token,
            &track,
            &dest_root,
            item.format,
        )
        .await
        {
            Ok(outcome) => {
                let already = matches!(outcome, DownloadOutcome::AlreadyExists(_));
                let path = outcome.path().to_string_lossy().to_string();
                manager.record_success();
                broadcast(
                    &state,
                    AppEvent::DownloadItemDone {
                        track_id: track.id,
                        ok: true,
                        already,
                        path: Some(path),
                        error: None,
                    },
                )
                .await;
            }
            Err(e) => {
                let reason = e.reason();
                manager.record_failure(track.id, track.title.clone(), reason.clone());
                broadcast(
                    &state,
                    AppEvent::DownloadItemDone {
                        track_id: track.id,
                        ok: false,
                        already: false,
                        path: None,
                        error: Some(reason),
                    },
                )
                .await;
            }
        }
        broadcast_progress(&state).await;
    }

    let (ok, failed) = manager.final_counts();
    broadcast(&state, AppEvent::DownloadComplete { ok, failed }).await;
}

/// Enqueue items and ensure exactly one worker is draining the queue.
async fn enqueue_and_spawn(state: &SharedState, items: Vec<DownloadJobItem>, prioritize: bool) {
    let need_spawn = { state.read().await.downloads.enqueue(items, prioritize) };
    broadcast_progress(state).await;
    if need_spawn {
        let state = state.clone();
        tokio::spawn(async move { run_download_worker(state).await });
    }
}

// ─── Settings ────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct DownloadSettings {
    folder: String,
    format: String,
}

async fn current_settings(state: &SharedState) -> DownloadSettings {
    let s = state.read().await;
    let (folder, format) =
        s.db.with_conn(|conn| {
            Ok((
                download::read_download_folder(conn)
                    .to_string_lossy()
                    .to_string(),
                download::read_default_format(conn),
            ))
        })
        .unwrap_or_else(|_| {
            (
                download::default_download_folder()
                    .to_string_lossy()
                    .to_string(),
                DownloadFormat::Flac,
            )
        });
    DownloadSettings {
        folder,
        format: format.as_str().to_string(),
    }
}

pub async fn get_download_settings(State(state): State<SharedState>) -> Json<DownloadSettings> {
    Json(current_settings(&state).await)
}

#[derive(Deserialize)]
pub struct UpdateDownloadSettings {
    folder: Option<String>,
    format: Option<String>,
}

pub async fn set_download_settings(
    State(state): State<SharedState>,
    Json(body): Json<UpdateDownloadSettings>,
) -> Result<Json<DownloadSettings>, StatusCode> {
    {
        let s = state.read().await;
        s.db.with_conn(|conn| {
            if let Some(folder) = body.folder.as_deref().filter(|f| !f.trim().is_empty()) {
                download::write_download_folder(conn, folder)?;
            }
            if let Some(format) = body.format.as_deref().and_then(DownloadFormat::from_query) {
                download::write_default_format(conn, format)?;
            }
            Ok(())
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    Ok(Json(current_settings(&state).await))
}

// ─── Download triggers ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct DownloadQuery {
    format: Option<String>,
}

async fn resolve_format(state: &SharedState, requested: Option<&str>) -> DownloadFormat {
    if let Some(format) = requested.and_then(DownloadFormat::from_query) {
        return format;
    }
    state
        .read()
        .await
        .db
        .with_conn(|conn| Ok(download::read_default_format(conn)))
        .unwrap_or(DownloadFormat::Flac)
}

/// `POST /api/tracks/{id}/download?format=flac|mp3` — queue a single track (jumps ahead
/// of any in-progress batch). Completion + saved path arrive via the `download_item_done`
/// WebSocket event.
pub async fn download_track(
    State(state): State<SharedState>,
    Path(track_id): Path<i64>,
    Query(query): Query<DownloadQuery>,
) -> Result<Json<Value>, StatusCode> {
    let format = resolve_format(&state, query.format.as_deref()).await;

    let track = {
        let s = state.read().await;
        s.db.with_conn(|conn| crate::playback::queue::get_track_by_id(conn, track_id))
            .ok()
            .flatten()
    };
    let Some(track) = track else {
        return Err(StatusCode::NOT_FOUND);
    };
    if track.tidal_id.is_none() {
        return Ok(Json(json!({
            "status": "unavailable",
            "message": "This track isn't on TIDAL, so it can't be downloaded."
        })));
    }

    enqueue_and_spawn(&state, vec![DownloadJobItem { track_id, format }], true).await;
    Ok(Json(json!({ "status": "queued" })))
}

#[derive(Deserialize)]
pub struct BatchDownloadRequest {
    ids: Vec<i64>,
    format: Option<String>,
}

/// `POST /api/downloads/batch` — queue many tracks (e.g. a whole album/playlist) to run
/// sequentially in the background.
pub async fn download_batch(
    State(state): State<SharedState>,
    Json(body): Json<BatchDownloadRequest>,
) -> Result<Json<Value>, StatusCode> {
    if body.ids.is_empty() {
        return Ok(Json(json!({ "status": "empty", "count": 0 })));
    }
    let format = resolve_format(&state, body.format.as_deref()).await;
    let count = body.ids.len();
    let items = body
        .ids
        .into_iter()
        .map(|track_id| DownloadJobItem { track_id, format })
        .collect();
    enqueue_and_spawn(&state, items, false).await;
    Ok(Json(json!({ "status": "queued", "count": count })))
}

/// `POST /api/downloads/cancel` — stop after the current track; clears the rest of the queue.
pub async fn cancel_downloads(State(state): State<SharedState>) -> Json<Value> {
    state.read().await.downloads.request_cancel();
    Json(json!({ "status": "cancelling" }))
}

/// `GET /api/downloads/status` — current worker snapshot (survives UI navigation).
pub async fn download_status(State(state): State<SharedState>) -> Json<DownloadStatus> {
    Json(state.read().await.downloads.snapshot())
}
