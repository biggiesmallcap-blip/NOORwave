use crate::sidecar::SidecarState;
use reqwest::Method;
use std::sync::Arc;
use tauri::Manager;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

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
        .on_shortcut("MediaPlayPause", move |_app, _sc, event| {
            if event.state() == ShortcutState::Pressed {
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
        .on_shortcut("MediaTrackNext", move |_app, _sc, event| {
            if event.state() == ShortcutState::Pressed {
                let state = s2.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = send(&state, Method::POST, "playback/next").await;
                });
            }
        })?;

    // MediaTrackPrevious
    let s3 = state.clone();
    app.global_shortcut()
        .on_shortcut("MediaTrackPrevious", move |_app, _sc, event| {
            if event.state() == ShortcutState::Pressed {
                let state = s3.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = send(&state, Method::POST, "playback/previous").await;
                });
            }
        })?;

    Ok(())
}
