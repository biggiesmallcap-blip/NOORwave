#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod config;
mod installed_updater;
mod media_keys;
mod migration;
mod paths;
mod server_url;
mod sidecar;
mod sidecar_paths;
mod tray;
mod updater;

use sidecar::SidecarState;
use sidecar_paths::SidecarPaths;
use std::sync::Arc;

fn main() {
    let cfg = config::load();
    let state = SidecarState::new(cfg.host_mode);
    let state_for_setup = state.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(state.clone() as Arc<SidecarState>)
        .invoke_handler(tauri::generate_handler![
            commands::check_for_updates_now,
            commands::get_update_state,
            commands::get_install_mode,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let installed = paths::is_installed_mode();

            let _ = state_for_setup.paths.set(SidecarPaths::resolve(&handle));

            if installed && !paths::data_dir().join("noor.db").exists() {
                migration::prompt_and_import(&handle);
                let reloaded = config::load();
                *state_for_setup.host_mode.lock().unwrap() = reloaded.host_mode;
            }

            sidecar::spawn_server(&state_for_setup);
            sidecar::wait_for_ready(&state_for_setup);

            tauri::WebviewWindowBuilder::new(
                &handle,
                "main",
                tauri::WebviewUrl::External(server_url::base().parse().expect("valid app url")),
            )
            .title("NOORwave")
            .inner_size(1280.0, 800.0)
            .min_inner_size(720.0, 500.0)
            .resizable(true)
            .decorations(true)
            .visible(true)
            .build()?;

            tray::setup_tray(app)?;
            media_keys::register(app)?;

            let update_handle = handle.clone();
            std::thread::spawn(move || {
                if installed {
                    installed_updater::background_check(&update_handle);
                } else if let Some(info) = updater::check() {
                    tray::notify_update(
                        &update_handle,
                        info.version,
                        tray::UpdateAction::OpenUrl(info.url),
                    );
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building NOORwave")
        .run(move |_app_handle, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                sidecar::kill_server(&state);
            }
        });
}
