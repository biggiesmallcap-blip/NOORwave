use crate::sidecar::SidecarState;
use reqwest::Method;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

#[derive(Default)]
pub struct VideoMediaKeys(AtomicBool);

#[tauri::command]
pub fn set_video_media_active(state: tauri::State<'_, VideoMediaKeys>, active: bool) {
    state.0.store(active, Ordering::SeqCst);
}

fn route_video_key<R: tauri::Runtime>(app: &tauri::AppHandle<R>, action: &str) -> bool {
    if !app.state::<VideoMediaKeys>().0.load(Ordering::SeqCst) {
        return false;
    }
    // The webview owns video playback. Keep the music sidecar untouched even
    // at the end of a video queue or while the next stream is loading.
    let _ = app.emit_to("main", "video-media-key", action);
    true
}

/// Send with the cached PIN; on 401 (the PIN was rotated in Settings) fetch
/// the current one from the loopback setup endpoint and retry once.
async fn send(state: &SidecarState, method: Method, path: &str) -> Option<reqwest::Response> {
    let client = reqwest::Client::new();
    let token = state.server_token.lock().unwrap().clone()?;
    let response = client
        .request(method.clone(), crate::server_url::api(path))
        .bearer_auth(&token)
        .send()
        .await
        .ok()?;
    if response.status() != reqwest::StatusCode::UNAUTHORIZED {
        return Some(response);
    }
    let fresh = crate::sidecar::reacquire_server_token_async(state).await?;
    client
        .request(method, crate::server_url::api(path))
        .bearer_auth(fresh)
        .send()
        .await
        .ok()
}

pub fn register(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let state: Arc<SidecarState> = app.state::<Arc<SidecarState>>().inner().clone();

    // MediaPlayPause — check is_playing, call pause or resume
    let s1 = state.clone();
    app.global_shortcut()
        .on_shortcut("MediaPlayPause", move |app, _sc, event| {
            if event.state() == ShortcutState::Pressed {
                if route_video_key(app, "toggle") {
                    return;
                }
                let state = s1.clone();
                tauri::async_runtime::spawn(async move {
                    // Determine current play state
                    let Some(resp) = send(&state, Method::GET, "playback/state").await else {
                        return;
                    };
                    let Ok(body) = resp.json::<serde_json::Value>().await else {
                        return;
                    };
                    let is_playing = body["state"]["is_playing"].as_bool().unwrap_or(false);

                    let endpoint = if is_playing { "pause" } else { "resume" };
                    let _ = send(&state, Method::POST, &format!("playback/{endpoint}")).await;
                });
            }
        })?;

    // MediaTrackNext
    let s2 = state.clone();
    app.global_shortcut()
        .on_shortcut("MediaTrackNext", move |app, _sc, event| {
            if event.state() == ShortcutState::Pressed {
                if route_video_key(app, "next") {
                    return;
                }
                let state = s2.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = send(&state, Method::POST, "playback/next").await;
                });
            }
        })?;

    // MediaTrackPrevious
    let s3 = state.clone();
    app.global_shortcut()
        .on_shortcut("MediaTrackPrevious", move |app, _sc, event| {
            if event.state() == ShortcutState::Pressed {
                if route_video_key(app, "previous") {
                    return;
                }
                let state = s3.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = send(&state, Method::POST, "playback/previous").await;
                });
            }
        })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tauri::Listener;

    #[test]
    fn video_session_owns_keys_until_it_is_cleared() {
        let app = tauri::test::mock_builder()
            .manage(VideoMediaKeys::default())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let received = Arc::new(Mutex::new(Vec::new()));
        let captured = received.clone();
        app.listen_any("video-media-key", move |event| {
            captured.lock().unwrap().push(event.payload().to_string());
        });
        assert!(!route_video_key(app.handle(), "next"));
        set_video_media_active(app.state(), true);
        for action in ["next", "previous", "toggle"] {
            assert!(route_video_key(app.handle(), action));
        }
        assert_eq!(
            *received.lock().unwrap(),
            ["\"next\"", "\"previous\"", "\"toggle\""]
        );
        set_video_media_active(app.state(), false);
        assert!(!route_video_key(app.handle(), "next"));
        assert_eq!(received.lock().unwrap().len(), 3);
    }
}
