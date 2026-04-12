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
use tokio::time::{Duration, interval};
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

    // Keepalive ping every 30 seconds to prevent idle TCP connections from
    // being reaped by NAT/proxies/OS when there are no playback events.
    let mut ping_ticker = interval(Duration::from_secs(30));
    ping_ticker.tick().await; // consume the immediate first tick

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
                    AppEvent::MusicBrainzEnriched => json!({"type": "musicbrainz_enriched"}),
                    AppEvent::TrackChanged { track_id } => json!({"type": "track_changed", "track_id": track_id}),
                    AppEvent::SyncProgress { service, progress } => json!({"type": "sync_progress", "service": service, "progress": progress}),
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
                };
                if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                    break;
                }
            }
            // Keepalive ping
            _ = ping_ticker.tick() => {
                if socket.send(Message::Ping(vec![].into())).await.is_err() {
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
