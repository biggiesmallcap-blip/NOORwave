use crate::sidecar::SidecarState;
use std::sync::Arc;
use tauri::Manager;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

pub fn register(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let state: Arc<SidecarState> = app.state::<Arc<SidecarState>>().inner().clone();

    // MediaPlayPause — check is_playing, call pause or resume
    let s1 = state.clone();
    app.global_shortcut().on_shortcut("MediaPlayPause", move |_app, _sc, event| {
        if event.state() == ShortcutState::Pressed {
            let state = s1.clone();
            tauri::async_runtime::spawn(async move {
                let token = state.server_token.lock().unwrap().clone();
                let Some(token) = token else { return };
                let client = reqwest::Client::new();
                let auth = format!("Bearer {token}");

                // Determine current play state
                let Ok(resp) = client
                    .get("http://127.0.0.1:3334/api/playback/state")
                    .header("authorization", &auth)
                    .send()
                    .await
                else {
                    return;
                };
                let Ok(body) = resp.json::<serde_json::Value>().await else { return };
                let is_playing = body["state"]["is_playing"].as_bool().unwrap_or(false);

                let endpoint = if is_playing { "pause" } else { "resume" };
                let _ = client
                    .post(format!("http://127.0.0.1:3334/api/playback/{endpoint}"))
                    .header("authorization", &auth)
                    .send()
                    .await;
            });
        }
    })?;

    // MediaTrackNext
    let s2 = state.clone();
    app.global_shortcut().on_shortcut("MediaTrackNext", move |_app, _sc, event| {
        if event.state() == ShortcutState::Pressed {
            let state = s2.clone();
            tauri::async_runtime::spawn(async move {
                let token = state.server_token.lock().unwrap().clone();
                let Some(token) = token else { return };
                let _ = reqwest::Client::new()
                    .post("http://127.0.0.1:3334/api/playback/next")
                    .header("authorization", format!("Bearer {token}"))
                    .send()
                    .await;
            });
        }
    })?;

    // MediaTrackPrevious
    let s3 = state.clone();
    app.global_shortcut().on_shortcut("MediaTrackPrevious", move |_app, _sc, event| {
        if event.state() == ShortcutState::Pressed {
            let state = s3.clone();
            tauri::async_runtime::spawn(async move {
                let token = state.server_token.lock().unwrap().clone();
                let Some(token) = token else { return };
                let _ = reqwest::Client::new()
                    .post("http://127.0.0.1:3334/api/playback/previous")
                    .header("authorization", format!("Bearer {token}"))
                    .send()
                    .await;
            });
        }
    })?;

    Ok(())
}
