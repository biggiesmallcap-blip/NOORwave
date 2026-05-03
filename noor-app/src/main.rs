#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod media_keys;
mod sidecar;
mod tray;
mod updater;

use sidecar::SidecarState;
use std::sync::Arc;
use tauri::Manager;

#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    open::that(url).map_err(|e| e.to_string())
}

fn main() {
    let cfg = config::load();
    let state = SidecarState::new(cfg.host_mode);

    // Spawn noor-server immediately, then wait for it in setup.
    sidecar::spawn_server(&state);

    let state_for_setup = state.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![open_external])
        .manage(state.clone() as Arc<SidecarState>)
        .setup(move |app| {
            // Wait for server on a background thread so we don't block setup.
            let handle = app.handle().clone();
            let state2 = state_for_setup.clone();
            std::thread::spawn(move || {
                sidecar::wait_for_ready(&state2);
                if let Some(win) = handle.get_webview_window("main") {
                    // Navigate explicitly now that the server is confirmed ready.
                    // WebView2 may have cached a connection-refused failure from
                    // the initial load attempt while the server was starting up.
                    if let Ok(url) = "http://127.0.0.1:3334".parse() {
                        let _ = win.navigate(url);
                    }
                    let _ = win.show();
                    let _ = win.set_focus();
                }

                // Check for updates in background after app is visible.
                let update_handle = handle.clone();
                std::thread::spawn(move || {
                    if let Some(info) = updater::check() {
                        tray::notify_update(&update_handle, &info.version, info.url);
                    }
                });
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
