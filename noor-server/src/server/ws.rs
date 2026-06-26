use crate::{AppEvent, SharedState};
use axum::{
    Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
    routing::get,
};
use serde_json::json;
use tracing::info;

pub fn ws_routes(state: SharedState) -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state)
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<SharedState>) -> Response {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: SharedState) {
    info!("WebSocket client connected");

    // Subscribe to the event bus
    let mut rx = {
        let state = state.read().await;
        state.event_tx.subscribe()
    };

    // Send initial state
    let init_msg = json!({
        "type": "connected",
        "message": "Welcome to NOOR"
    });
    if socket
        .send(Message::Text(init_msg.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    // Forward events to the client
    loop {
        tokio::select! {
            // Events from the app
            recv_result = rx.recv() => {
                let event = match recv_result {
                    Ok(e) => e,
                    // Channel lagged (burst of events exceeded capacity) — re-sync client state.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let _ = socket.send(Message::Text(json!({"type": "playback_changed"}).to_string().into())).await;
                        continue;
                    }
                    // All senders dropped — server shutting down.
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                let msg = match event {
                    AppEvent::PlaybackStateChanged => json!({"type": "playback_changed"}),
                    AppEvent::LibrarySynced => json!({"type": "library_synced"}),
                    AppEvent::RadioSimilarityComputed { pairs } => json!({"type": "radio_similarity_computed", "pairs": pairs}),
                    AppEvent::MusicBrainzEnriched => json!({"type": "musicbrainz_enriched"}),
                    AppEvent::TrackChanged { track_id } => json!({"type": "track_changed", "track_id": track_id}),
                    AppEvent::SyncProgress { service, progress } => json!({"type": "sync_progress", "service": service, "progress": progress}),
                    AppEvent::SyncFailed { service, message } => json!({"type": "sync_failed", "service": service, "message": message}),
                    AppEvent::QueueUpdated => json!({"type": "queue_updated"}),
                    AppEvent::ListenHistoryUpdated { track_id } => json!({"type": "listen_history_updated", "track_id": track_id}),
                    AppEvent::PlaybackFailed { message } => json!({"type": "playback_failed", "message": message}),
                    AppEvent::TrainingProgress { stage, progress, message, current_track_id, current_track_title, tracks_done, tracks_total } => json!({
                        "type": "training_progress",
                        "stage": stage,
                        "progress": progress,
                        "message": message,
                        "current_track_id": current_track_id,
                        "current_track_title": current_track_title,
                        "tracks_done": tracks_done,
                        "tracks_total": tracks_total
                    }),
                    AppEvent::AudioAnalysisProgress { analyzed, total, mode } => json!({
                        "type": "audio_analysis_progress",
                        "analyzed": analyzed,
                        "total": total,
                        "mode": mode
                    }),
                    AppEvent::AudioAnalysisComplete { analyzed } => json!({
                        "type": "audio_analysis_complete",
                        "analyzed": analyzed
                    }),
                    AppEvent::TrackAnalyzed { track_id } => json!({
                        "type": "track_analyzed",
                        "track_id": track_id
                    }),
                    AppEvent::AcrCloudScanProgress { scanned, total, matches_found } => json!({
                        "type": "acrcloud_scan_progress",
                        "scanned": scanned,
                        "total": total,
                        "matches_found": matches_found
                    }),
                    AppEvent::AcrCloudScanComplete { scanned, matches_found } => json!({
                        "type": "acrcloud_scan_complete",
                        "scanned": scanned,
                        "matches_found": matches_found
                    }),
                    AppEvent::DiscoverySpaceRefreshProgress { seed_track_id, stage, progress } => json!({
                        "type": "discovery_space_refresh_progress",
                        "seed_track_id": seed_track_id,
                        "stage": stage,
                        "progress": progress,
                    }),
                    AppEvent::DiscoverySpaceRefreshed { seed_track_id } => json!({
                        "type": "discovery_space_refreshed",
                        "seed_track_id": seed_track_id,
                    }),
                    AppEvent::AudioExclusiveEngaged { device, transport_format } => json!({
                        "type": "audio_exclusive_engaged",
                        "device": device,
                        "transport_format": transport_format,
                    }),
                    AppEvent::AudioExclusiveFailed { device, reason } => json!({
                        "type": "audio_exclusive_failed",
                        "device": device,
                        "reason": reason,
                    }),
                    AppEvent::AudioExclusiveReleased { device } => json!({
                        "type": "audio_exclusive_released",
                        "device": device,
                    }),
                    AppEvent::DownloadProgress { done, total, current_title } => json!({
                        "type": "download_progress",
                        "done": done,
                        "total": total,
                        "current_title": current_title,
                    }),
                    AppEvent::DownloadItemDone { track_id, ok, already, path, error } => json!({
                        "type": "download_item_done",
                        "track_id": track_id,
                        "ok": ok,
                        "already": already,
                        "path": path,
                        "error": error,
                    }),
                    AppEvent::DownloadComplete { ok, failed } => json!({
                        "type": "download_complete",
                        "ok": ok,
                        "failed": failed,
                    }),
                };
                if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                    break;
                }
            }
            // Messages from the client
            client_msg = socket.recv() => {
                match client_msg {
                    Some(Ok(Message::Text(text))) => tracing::debug!("WS client: {}", text),
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }

    info!("WebSocket client disconnected");
}
