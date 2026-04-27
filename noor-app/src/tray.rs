use crate::{config, sidecar::SidecarState};
use std::sync::Arc;
use tauri::{
    image::Image,
    menu::{CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Manager,
};

pub fn setup_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let state: Arc<SidecarState> = app.state::<Arc<SidecarState>>().inner().clone();
    let host_mode = *state.host_mode.lock().unwrap();

    // Menu items
    let show_item = MenuItemBuilder::with_id("show", "Show NOORwave").build(app)?;
    let network_item = CheckMenuItemBuilder::with_id("network", "Network access")
        .checked(host_mode)
        .enabled(true)
        .build(app)?;
    let restart_item = MenuItemBuilder::with_id("restart", "Restart server").build(app)?;
    let exit_item = MenuItemBuilder::with_id("exit", "Exit").build(app)?;

    let menu = MenuBuilder::new(app)
        .item(&show_item)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&network_item)
        .item(&restart_item)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&exit_item)
        .build()?;

    // Icon embedded at compile time
    let icon = Image::from_bytes(include_bytes!("../icons/icon.png"))?;
    let network_item_clone = network_item.clone();

    TrayIconBuilder::new()
        .icon(icon)
        .menu(&menu)
        .tooltip("NOORwave")
        // Left-click shows the window
        .on_tray_icon_event({
            let handle = app.handle().clone();
            move |_tray, event| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    ..
                } = event
                {
                    if let Some(win) = handle.get_webview_window("main") {
                        let _ = win.show();
                        let _ = win.set_focus();
                    }
                }
            }
        })
        // Right-click menu events
        .on_menu_event({
            let handle = app.handle().clone();
            let state = state.clone();
            move |_app, event| match event.id().as_ref() {
                "show" => {
                    if let Some(win) = handle.get_webview_window("main") {
                        let _ = win.show();
                        let _ = win.set_focus();
                    }
                }
                "network" => {
                    let current = network_item_clone.is_checked().unwrap_or(false);
                    let new_mode = !current;
                    let _ = network_item_clone.set_checked(new_mode);
                    *state.host_mode.lock().unwrap() = new_mode;

                    // Persist to noor-config.json
                    let mut cfg = config::load();
                    cfg.host_mode = new_mode;
                    config::save(&cfg);

                    // Also tell noor-server so headless users see the change
                    if let Some(token) = state.server_token.lock().unwrap().clone() {
                        let body = serde_json::json!({ "host_mode": new_mode });
                        let _ = reqwest::blocking::Client::new()
                            .put("http://127.0.0.1:3334/api/server/host_mode")
                            .header("authorization", format!("Bearer {token}"))
                            .json(&body)
                            .send();
                    }

                    // Restart server with new flag
                    let state2 = state.clone();
                    let handle2 = handle.clone();
                    std::thread::spawn(move || {
                        crate::sidecar::restart_server(&state2);
                        crate::sidecar::wait_for_ready(&state2);
                        if let Some(win) = handle2.get_webview_window("main") {
                            let _ = win.eval("window.location.reload()");
                        }
                    });
                }
                "restart" => {
                    let state2 = state.clone();
                    let handle2 = handle.clone();
                    std::thread::spawn(move || {
                        crate::sidecar::restart_server(&state2);
                        crate::sidecar::wait_for_ready(&state2);
                        if let Some(win) = handle2.get_webview_window("main") {
                            let _ = win.eval("window.location.reload()");
                        }
                    });
                }
                "exit" => {
                    crate::sidecar::kill_server(&state);
                    handle.exit(0);
                }
                _ => {}
            }
        })
        .build(app)?;

    Ok(())
}
