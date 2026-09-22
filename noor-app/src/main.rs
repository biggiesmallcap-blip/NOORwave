#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod config;
mod installed_updater;
mod media_keys;
mod migration;
mod paths;
mod remote_lifecycle;
mod server_url;
mod sidecar;
mod sidecar_paths;
mod startup;
mod tray;
mod updater;

use sidecar::SidecarState;
use sidecar_paths::SidecarPaths;
use std::sync::Arc;
use tauri::Manager;

fn main() {
    let launch_mode = startup::launch_mode_from(std::env::args());
    let silent_launch = launch_mode == startup::LaunchMode::Autostart;
    let cfg = config::load();
    let sidecar_state = SidecarState::new(cfg.host_mode);
    let lifecycle = remote_lifecycle::RemoteLifecycle::new(sidecar_state.clone(), cfg.host_mode);
    let lifecycle_for_setup = lifecycle.clone();

    tauri::Builder::default()
        // Must be first: arbitration happens before setup can spawn a sidecar.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if startup::launch_mode_from(args.iter()) == startup::LaunchMode::Normal {
                startup::request_activation();
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![startup::AUTOSTART_MARKER]),
        ))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(sidecar_state.clone() as Arc<SidecarState>)
        .manage(lifecycle.clone() as Arc<remote_lifecycle::RemoteLifecycle>)
        .manage(startup::StartupRuntime::new(launch_mode))
        .invoke_handler(tauri::generate_handler![
            commands::check_for_updates_now,
            commands::get_update_state,
            commands::install_pending_update,
            commands::get_install_mode,
            commands::get_minimize_to_tray,
            commands::set_minimize_to_tray,
            remote_lifecycle::get_remote_host_state,
            remote_lifecycle::set_remote_host_mode,
            remote_lifecycle::restart_managed_server,
            startup::get_startup_state,
            startup::set_start_at_login,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let installed = paths::is_installed_mode();

            let state_for_setup = lifecycle_for_setup.sidecar().clone();
            let _ = state_for_setup.paths.set(SidecarPaths::resolve(&handle));

            if installed && !paths::data_dir().join("noor.db").exists() {
                migration::prompt_and_import(&handle);
                let reloaded = config::load();
                lifecycle_for_setup.adopt_initial_config(reloaded.host_mode);
            }

            let expected_mode = *state_for_setup.host_mode.lock().unwrap();
            lifecycle_for_setup.mark_initial_start();
            let spawn_result = sidecar::spawn_server(&state_for_setup);

            let window = tauri::WebviewWindowBuilder::new(
                &handle,
                "main",
                tauri::WebviewUrl::External(server_url::base().parse().expect("valid app url")),
            )
            .title("NOORwave")
            .inner_size(1280.0, 800.0)
            .min_inner_size(720.0, 500.0)
            .resizable(true)
            .decorations(true)
            // Keep creation deterministic and unfocused. A normal launch is
            // shown after bounded readiness; autostart remains tray-only.
            .visible(false)
            .focused(false)
            // Tauri's OS-level drag-drop handler is enabled by default, which
            // makes WebView2 capture native drag events for file-drop and
            // suppresses HTML5 drag-and-drop inside the page. That is why the
            // Up Next queue could never be dragged in the packaged app no matter
            // what the DOM did (it works in a plain dev browser). Disable it so
            // the webview's own dragstart/dragover/drop fire for queue reorder.
            .disable_drag_drop_handler()
            .build()?;

            if startup::take_activation_request() {
                let _ = window.show();
                let _ = window.set_focus();
            }

            tray::setup_tray(app)?;
            media_keys::register(app)?;

            let ready_handle = handle.clone();
            let ready_lifecycle = lifecycle_for_setup.clone();
            std::thread::spawn(move || {
                let result = spawn_result.and_then(|_| {
                    sidecar::wait_for_remote_ready(
                        ready_lifecycle.sidecar(),
                        expected_mode,
                        std::time::Instant::now() + std::time::Duration::from_secs(15),
                    )
                });
                ready_lifecycle.publish_initial_result(&ready_handle, &result);
                if let Err(error) = &result {
                    eprintln!("managed server startup failed: {error}");
                }
                if !silent_launch {
                    if let Some(window) = ready_handle.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                        let _ = window.eval("window.location.reload()");
                    }
                }
            });

            let update_handle = handle.clone();
            std::thread::spawn(move || {
                if installed {
                    installed_updater::background_check(&update_handle);
                } else if let Some(info) = updater::check() {
                    tray::notify_update(
                        &update_handle,
                        info.version,
                        info.notes,
                        tray::UpdateAction::OpenUrl(info.url),
                    );
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Read the preference fresh each close so a settings change
                // takes effect without a restart.
                if config::load().minimize_to_tray {
                    // Opt-in: keep running in the tray.
                    let _ = window.hide();
                    api.prevent_close();
                } else {
                    // Default: quit. With a tray icon present the app stays
                    // alive after the last window closes, so request a full exit
                    // explicitly. RunEvent::Exit then shuts the sidecar down.
                    window.app_handle().exit(0);
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building NOORwave")
        .run(move |_app_handle, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                sidecar::kill_server(&sidecar_state);
            }
        });
}
