use crate::server::remote::Principal;
use crate::{AppEvent, SharedState};
use axum::{
    Extension, Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    response::Response,
    routing::get,
};
use serde_json::json;
use std::time::Duration;
use tokio::sync::watch;
use tracing::info;

/// App-level heartbeat so clients can tell a live socket from one a phone's
/// OS silently killed while suspended (browsers hide protocol pings from JS).
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
/// A peer that cannot take a frame within this window (phone Wi-Fi asleep,
/// client gone without a FIN) is dropped instead of stalling every other
/// event behind a blocked send; the client reconnects and resyncs.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

async fn send_text(socket: &mut WebSocket, text: String) -> bool {
    matches!(
        tokio::time::timeout(SEND_TIMEOUT, socket.send(Message::Text(text.into()))).await,
        Ok(Ok(()))
    )
}

/// Clients opt in to the ~30 Hz visualiser stream with
/// `{"type":"spectrum","enabled":true}`. Only the desktop shader wallpaper
/// reads it; the phone remote never does, and pushing it over Wi-Fi starved
/// the events the remote actually needs.
fn spectrum_request(text: &str) -> Option<bool> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    if value.get("type")?.as_str()? != "spectrum" {
        return None;
    }
    value.get("enabled")?.as_bool()
}

pub fn ws_routes(state: SharedState, shutdown: watch::Receiver<bool>) -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state)
        .layer(Extension(shutdown))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<SharedState>,
    Extension(principal): Extension<Principal>,
    Extension(shutdown): Extension<watch::Receiver<bool>>,
) -> Response {
    ws.on_upgrade(|socket| handle_socket(socket, state, principal, shutdown))
}

async fn handle_socket(
    mut socket: WebSocket,
    state: SharedState,
    principal: Principal,
    mut shutdown: watch::Receiver<bool>,
) {
    info!("WebSocket client connected");

    let remote = state.read().await.remote.clone();
    let Some((mut global_revocation, mut device_revocation)) =
        remote.subscribe_if_current(&principal).await
    else {
        let _ = socket
            .send(Message::Close(Some(CloseFrame {
                code: 4001,
                reason: "Authentication required".into(),
            })))
            .await;
        return;
    };

    if *shutdown.borrow() {
        let _ = socket
            .send(Message::Close(Some(CloseFrame {
                code: 1001,
                reason: "Server shutting down".into(),
            })))
            .await;
        return;
    }

    // Subscribe to the event bus
    let mut rx = {
        let state = state.read().await;
        state.event_tx.subscribe()
    };

    // Send initial state
    // heartbeat_ms lets the client hold this socket to the heartbeat
    // deadline from the start, before the first heartbeat arrives.
    let init_msg = json!({
        "type": "connected",
        "message": "Welcome to NOOR",
        "heartbeat_ms": HEARTBEAT_INTERVAL.as_millis() as u64,
    });
    if socket
        .send(Message::Text(init_msg.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    // Poll the real-time audio spectrum at ~30 Hz for the wallpaper visualiser.
    // `poll` returns a frame only when a new one was computed (silence produces
    // nothing), so an idle player generates no visualiser traffic.
    let mut spectrum_ticker = tokio::time::interval(std::time::Duration::from_millis(33));
    spectrum_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_spectrum_seq: u64 = 0;
    let mut spectrum_enabled = false;
    let mut heartbeat = tokio::time::interval_at(
        tokio::time::Instant::now() + HEARTBEAT_INTERVAL,
        HEARTBEAT_INTERVAL,
    );
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // Forward events to the client
    loop {
        tokio::select! {
            biased;
            _ = global_revocation.recv() => {
                let _ = socket.send(Message::Close(Some(CloseFrame {
                    code: 4001,
                    reason: "Authentication required".into(),
                }))).await;
                break;
            }
            _ = async {
                match device_revocation.as_mut() {
                    Some(receiver) => { let _ = receiver.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {
                let _ = socket.send(Message::Close(Some(CloseFrame {
                    code: 4001,
                    reason: "Authentication required".into(),
                }))).await;
                break;
            }
            _ = async {
                while !*shutdown.borrow() {
                    if shutdown.changed().await.is_err() {
                        std::future::pending::<()>().await;
                    }
                }
            } => {
                let _ = socket.send(Message::Close(Some(CloseFrame {
                    code: 1001,
                    reason: "Server shutting down".into(),
                }))).await;
                break;
            }
            // Real-time audio spectrum frames
            _ = spectrum_ticker.tick(), if spectrum_enabled => {
                if let Some(bands) = crate::playback::spectrum::global().poll(&mut last_spectrum_seq) {
                    let msg = json!({"type": "audio_spectrum", "bands": bands});
                    if !send_text(&mut socket, msg.to_string()).await {
                        break;
                    }
                }
            }
            _ = heartbeat.tick() => {
                if !send_text(&mut socket, json!({"type": "heartbeat"}).to_string()).await {
                    break;
                }
            }
            // Events from the app
            recv_result = rx.recv() => {
                let event = match recv_result {
                    Ok(e) => e,
                    // Channel lagged (burst of events exceeded capacity) — re-sync client state.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        if !send_text(&mut socket, json!({"type": "playback_changed"}).to_string()).await {
                            break;
                        }
                        continue;
                    }
                    // All senders dropped — server shutting down.
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                let msg = match event {
                    AppEvent::PlaybackStateChanged => json!({"type": "playback_changed"}),
                    AppEvent::LibrarySynced => json!({"type": "library_synced"}),
                    AppEvent::TidalContentSettingsChanged => json!({"type": "tidal_content_settings_changed"}),
                    AppEvent::RadioSimilarityComputed { pairs } => json!({"type": "radio_similarity_computed", "pairs": pairs}),
                    AppEvent::MusicBrainzEnriched => json!({"type": "musicbrainz_enriched"}),
                    AppEvent::TrackChanged { track_id } => json!({"type": "track_changed", "track_id": track_id}),
                    AppEvent::SyncProgress { service, progress } => json!({"type": "sync_progress", "service": service, "progress": progress}),
                    AppEvent::SyncFailed { service, message } => json!({"type": "sync_failed", "service": service, "message": message}),
                    AppEvent::QueueUpdated => json!({"type": "queue_updated"}),
                    AppEvent::PlaylistsChanged => json!({"type": "playlists_changed"}),
                    AppEvent::ListenHistoryUpdated { track_id } => json!({"type": "listen_history_updated", "track_id": track_id}),
                    AppEvent::PlaybackFailed { message } => json!({"type": "playback_failed", "message": message}),
                    AppEvent::TrackSkipped { track_id, title, reason } => json!({"type": "track_skipped", "track_id": track_id, "title": title, "reason": reason}),
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
                    AppEvent::HomeRecommendationsUpdated { provider, complete } => json!({
                        "type": "home_recommendations_updated",
                        "provider": provider,
                        "complete": complete,
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
                if !send_text(&mut socket, msg.to_string()).await {
                    break;
                }
            }
            // Messages from the client
            client_msg = socket.recv() => {
                match client_msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Some(enabled) = spectrum_request(&text) {
                            spectrum_enabled = enabled;
                        } else {
                            tracing::debug!("WS client: {}", text);
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }

    info!("WebSocket client disconnected");
}

#[cfg(test)]
mod tests {
    use super::spectrum_request;

    #[test]
    fn spectrum_subscription_parses_only_the_opt_in_message() {
        assert_eq!(
            spectrum_request(r#"{"type":"spectrum","enabled":true}"#),
            Some(true)
        );
        assert_eq!(
            spectrum_request(r#"{"type":"spectrum","enabled":false}"#),
            Some(false)
        );
        assert_eq!(spectrum_request(r#"{"type":"spectrum"}"#), None);
        assert_eq!(spectrum_request(r#"{"type":"other","enabled":true}"#), None);
        assert_eq!(spectrum_request("not json"), None);
    }
}
