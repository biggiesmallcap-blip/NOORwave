#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod media_keys;
mod sidecar;
mod tray;
mod updater;

use sidecar::SidecarState;
use std::sync::Arc;
use std::time::Duration;
use tauri::Manager;

#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    open::that(url).map_err(|e| e.to_string())
}

// Browser-style content zoom on the main webview. Exposed as a custom command
// rather than via @tauri-apps/api so we don't need a capabilities/ file or a
// frontend npm dep — matches the open_external pattern.
#[tauri::command]
fn set_ui_zoom(window: tauri::WebviewWindow, factor: f64) -> Result<(), String> {
    let clamped = factor.clamp(0.5, 2.0);
    window.set_zoom(clamped).map_err(|e| e.to_string())
}

fn main() {
    let cfg = config::load();
    let state = SidecarState::new(cfg.host_mode);

    // Spawn noor-server FIRST, then block briefly until it answers /api/ping
    // BEFORE Tauri opens the webview. This avoids the WebView2 cold-start
    // race entirely: by the time the window's initial URL is hit, the
    // server is already serving, so there's no connection-refused failure
    // to cache and no need for a splash + post-ready re-navigate.
    sidecar::spawn_server(&state);
    sidecar::wait_for_ready(&state);

    let state_for_setup = state.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![open_external, set_ui_zoom])
        .manage(state.clone() as Arc<SidecarState>)
        .setup(move |app| {
            let handle = app.handle().clone();
            let _state2 = state_for_setup.clone();

            // WebView2 on Windows occasionally paints a blank window on the
            // FIRST navigation even when the URL responded successfully. Only
            // a hard refresh (Ctrl+Shift+R) gets the renderer to actually paint.
            // Reproducible across releases; manual reload is the only known
            // reliable trigger. Force it once shortly after launch — the brief
            // flicker is preferable to a permanently blank window.
            let reload_handle = handle.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(1200));
                if let Some(win) = reload_handle.get_webview_window("main") {
                    let _ = win.eval("location.reload();");
                }
            });

            // Check for updates in background.
            let update_handle = handle.clone();
            std::thread::spawn(move || {
                if let Some(info) = updater::check() {
                    tray::notify_update(&update_handle, &info.version, info.url);
                }
            });

            tray::setup_tray(app)?;
            media_keys::register(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running NOORwave");
}
