#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod media_keys;
mod sidecar;
mod tray;

use sidecar::SidecarState;
use std::sync::Arc;
use tauri::Manager;

fn main() {
    let cfg = config::load();
    let state = SidecarState::new(cfg.host_mode);

    // Spawn noor-server immediately, then wait for it in setup.
    sidecar::spawn_server(&state);

    let state_for_setup = state.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(state.clone() as Arc<SidecarState>)
        .setup(move |app| {
            // Wait for server on a background thread so we don't block setup.
            let handle = app.handle().clone();
            let state2 = state_for_setup.clone();
            std::thread::spawn(move || {
                sidecar::wait_for_ready(&state2);
                // Navigate only after the server is confirmed ready — avoids
                // the blank-page bug where WebView2 caches a connection-refused
                // error from the initial about:blank → server URL load.
                if let Some(win) = handle.get_webview_window("main") {
                    if let Ok(url) = "http://127.0.0.1:3334".parse() {
                        let _ = win.navigate(url);
                    }
                    let _ = win.show();
                    let _ = win.set_focus();
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
