use crate::{config, sidecar::SidecarState};
use std::sync::{Arc, Mutex};
use tauri::{
    image::Image,
    menu::{
        CheckMenuItem, CheckMenuItemBuilder, MenuBuilder, MenuItem, MenuItemBuilder,
        PredefinedMenuItem,
    },
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, Theme, WindowEvent, Wry,
};

// Multi-resolution ICOs (16/24/32/48/256). Tauri's image decoder picks the
// largest frame, so the OS scales DOWN to whatever the system tray asks for —
// avoiding the upscaling blur that 32-px PNG sources caused.
const TRAY_BLACK_ICO: &[u8] = include_bytes!("../icons/noor-tray-black.ico");
const TRAY_WHITE_ICO: &[u8] = include_bytes!("../icons/noor-tray-white.ico");

// Light theme -> black icon (visible on a light system tray).
// Dark theme (default) -> white icon (visible on a dark system tray).
fn tray_icon_bytes_for_theme(theme: Option<Theme>) -> &'static [u8] {
    match theme {
        Some(Theme::Light) => TRAY_BLACK_ICO,
        _ => TRAY_WHITE_ICO,
    }
}

// Holds clones of tray menu items so the menu can be rebuilt when an update
// is found, without losing checkbox state or event handler references.
pub struct TrayMenuItems {
    pub show_item: MenuItem<Wry>,
    pub network_item: CheckMenuItem<Wry>,
    pub restart_item: MenuItem<Wry>,
    pub exit_item: MenuItem<Wry>,
    pub pending: Mutex<Option<(String, UpdateAction)>>,
}

#[derive(Clone)]
pub enum UpdateAction {
    OpenUrl(String),
    Install,
}

pub fn setup_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let state: Arc<SidecarState> = app.state::<Arc<SidecarState>>().inner().clone();
    let host_mode = *state.host_mode.lock().unwrap();

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

    // Store clones so notify_update can rebuild the menu reusing the same items.
    app.manage(TrayMenuItems {
        show_item: show_item.clone(),
        network_item: network_item.clone(),
        restart_item: restart_item.clone(),
        exit_item: exit_item.clone(),
        pending: Mutex::new(None),
    });

    let initial_theme = app.get_webview_window("main").and_then(|w| w.theme().ok());
    let icon = Image::from_bytes(tray_icon_bytes_for_theme(initial_theme))?;
    let network_item_clone = network_item.clone();

    TrayIconBuilder::with_id("noorwave-tray")
        .icon(icon)
        .menu(&menu)
        .tooltip("NOORwave")
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
        .on_menu_event({
            let handle = app.handle().clone();
            let state = state.clone();
            move |app_handle, event| match event.id().as_ref() {
                "show" => {
                    if let Some(win) = handle.get_webview_window("main") {
                        let _ = win.show();
                        let _ = win.set_focus();
                    }
                }
                "update" => {
                    let action = app_handle
                        .state::<TrayMenuItems>()
                        .pending
                        .lock()
                        .unwrap()
                        .clone();
                    match action {
                        Some((_, UpdateAction::OpenUrl(url))) => {
                            let _ = tauri_plugin_opener::open_url(url, None::<&str>);
                        }
                        Some((_, UpdateAction::Install)) => {
                            let handle = app_handle.clone();
                            tauri::async_runtime::spawn(async move {
                                if let Err(err) =
                                    crate::installed_updater::install_now(&handle).await
                                {
                                    let message = err.to_string();
                                    eprintln!("update install failed: {message}");
                                    let _ = handle.emit("update-error", &message);
                                }
                            });
                        }
                        None => {}
                    }
                }
                "network" => {
                    let current = network_item_clone.is_checked().unwrap_or(false);
                    let new_mode = !current;
                    let _ = network_item_clone.set_checked(new_mode);
                    *state.host_mode.lock().unwrap() = new_mode;

                    let mut cfg = config::load();
                    cfg.host_mode = new_mode;
                    config::save(&cfg);

                    if let Some(token) = state.server_token.lock().unwrap().clone() {
                        let body = serde_json::json!({ "host_mode": new_mode });
                        let _ = reqwest::blocking::Client::new()
                            .put(crate::server_url::api("server/host_mode"))
                            .header("authorization", format!("Bearer {token}"))
                            .json(&body)
                            .send();
                    }

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

    if let Some(window) = app.get_webview_window("main") {
        let handle_for_theme = app.handle().clone();
        window.on_window_event(move |event| {
            if let WindowEvent::ThemeChanged(theme) = event {
                if let Some(tray) = handle_for_theme.tray_by_id("noorwave-tray") {
                    if let Ok(new_icon) = Image::from_bytes(tray_icon_bytes_for_theme(Some(*theme)))
                    {
                        let _ = tray.set_icon(Some(new_icon));
                    }
                }
            }
        });
    }

    Ok(())
}

// Called from a background thread when a newer release is found.
// Rebuilds the tray menu with an update item at the top and updates the tooltip.
pub fn notify_update(handle: &tauri::AppHandle, version: String, action: UpdateAction) {
    let items = handle.state::<TrayMenuItems>();
    *items.pending.lock().unwrap() = Some((version.clone(), action.clone()));

    let verb = match action {
        UpdateAction::OpenUrl(_) => "download",
        UpdateAction::Install => "install",
    };
    let label = format!("v{version} available - click to {verb}");
    let Ok(update_item) = MenuItemBuilder::with_id("update", &label).build(handle) else {
        return;
    };

    let sep = |h: &tauri::AppHandle| PredefinedMenuItem::separator(h).unwrap();

    let Ok(menu) = MenuBuilder::new(handle)
        .item(&update_item)
        .item(&sep(handle))
        .item(&items.show_item)
        .item(&sep(handle))
        .item(&items.network_item)
        .item(&items.restart_item)
        .item(&sep(handle))
        .item(&items.exit_item)
        .build()
    else {
        return;
    };

    if let Some(tray) = handle.tray_by_id("noorwave-tray") {
        let _ = tray.set_menu(Some(menu));
        let _ = tray.set_tooltip(Some(format!("NOORwave - v{version} update available")));
    }
}
