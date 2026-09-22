use crate::remote_lifecycle::{DesktopRemoteError, RemoteLifecycle};
use crate::sidecar::ServerRemoteStatus;
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
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

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

struct NetworkConfirmation {
    message: String,
    accept_label: &'static str,
    cancel_label: &'static str,
}

fn network_confirmation(
    enabled: bool,
    impact: crate::sidecar::PlaybackRestartImpact,
) -> NetworkConfirmation {
    let action = if enabled { "Enable" } else { "Disable" };
    let playback = if impact.playback_active {
        "active"
    } else {
        "not active"
    };
    NetworkConfirmation {
        message: format!(
            "{action} phone remote and restart NOORwave's server? Playback is {playback} and {} queued track{} will be cleared.",
            impact.queue_count,
            if impact.queue_count == 1 { "" } else { "s" }
        ),
        accept_label: if enabled {
            "Enable and restart"
        } else {
            "Disable and restart"
        },
        cancel_label: "Cancel",
    }
}

pub fn setup_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let lifecycle: Arc<RemoteLifecycle> = app.state::<Arc<RemoteLifecycle>>().inner().clone();
    let host_mode = lifecycle.snapshot().configured_host_mode;

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
            let lifecycle = lifecycle.clone();
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
                    let desired = !lifecycle.snapshot().configured_host_mode;
                    let lifecycle = lifecycle.clone();
                    let handle = handle.clone();
                    tauri::async_runtime::spawn(async move {
                        let impact_sidecar = lifecycle.sidecar().clone();
                        let impact = tauri::async_runtime::spawn_blocking(move || {
                            crate::sidecar::fetch_playback_restart_impact(&impact_sidecar)
                        })
                        .await
                        .map_err(|error| error.to_string())
                        .and_then(|result| result);
                        let impact = match impact {
                            Ok(impact) => impact,
                            Err(error) => {
                                handle
                                    .dialog()
                                    .message(format!(
                                        "Network access was not changed because playback and queue status could not be read: {error}"
                                    ))
                                    .title("NOORwave network access")
                                    .kind(MessageDialogKind::Error)
                                    .blocking_show();
                                set_network_checked(
                                    &handle,
                                    lifecycle.snapshot().configured_host_mode,
                                );
                                return;
                            }
                        };
                        let prompt = network_confirmation(desired, impact);
                        let confirmed = handle
                            .dialog()
                            .message(prompt.message)
                            .title("NOORwave network access")
                            .kind(MessageDialogKind::Warning)
                            .buttons(MessageDialogButtons::OkCancelCustom(
                                prompt.accept_label.to_owned(),
                                prompt.cancel_label.to_owned(),
                            ))
                            .blocking_show();
                        if !confirmed {
                            set_network_checked(
                                &handle,
                                lifecycle.snapshot().configured_host_mode,
                            );
                            return;
                        }
                        let result = crate::remote_lifecycle::set_remote_host_mode_native(
                            handle.clone(),
                            lifecycle,
                            desired,
                        )
                        .await;
                        if let Err(error) = &result {
                            eprintln!("phone remote transition failed: {}", error.message);
                        }
                        set_network_checked(&handle, network_checked_from_transition(&result));
                    });
                }
                "restart" => {
                    let lifecycle = lifecycle.clone();
                    let handle = handle.clone();
                    tauri::async_runtime::spawn(async move {
                        let result = crate::remote_lifecycle::restart_managed_server_native(
                            handle.clone(),
                            lifecycle,
                        )
                        .await;
                        if let Err(error) = &result {
                            eprintln!("server restart failed: {}", error.message);
                        }
                        set_network_checked(&handle, network_checked_from_transition(&result));
                    });
                }
                "exit" => {
                    crate::sidecar::kill_server(lifecycle.sidecar());
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

pub fn set_network_checked(handle: &tauri::AppHandle, checked: bool) {
    if let Some(items) = handle.try_state::<TrayMenuItems>() {
        let _ = items.network_item.set_checked(checked);
    }
}

fn network_checked_from_transition(
    result: &Result<ServerRemoteStatus, DesktopRemoteError>,
) -> bool {
    match result {
        Ok(status) => status.configured_host_mode,
        Err(error) => error.state.configured_host_mode,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_lifecycle::{
        DesktopRemoteError, DesktopRemoteState, RemoteErrorCode, RemotePhase,
    };

    #[test]
    fn failure_before_a_state_event_rolls_the_checkbox_back() {
        let result = Err(DesktopRemoteError {
            code: RemoteErrorCode::ConfigWriteFailed,
            message: "injected write failure".to_owned(),
            state: DesktopRemoteState {
                configured_host_mode: false,
                phase: RemotePhase::Idle,
                last_error: None,
            },
        });

        assert!(!network_checked_from_transition(&result));
    }

    #[test]
    fn tray_confirmation_discloses_exact_live_impact_and_explicit_action() {
        let prompt = network_confirmation(
            true,
            crate::sidecar::PlaybackRestartImpact {
                playback_active: true,
                queue_count: 7,
            },
        );
        assert!(prompt.message.contains("Playback is active"));
        assert!(prompt.message.contains("7 queued tracks"));
        assert_eq!(prompt.accept_label, "Enable and restart");
        assert_eq!(prompt.cancel_label, "Cancel");
    }

    #[test]
    fn disable_confirmation_uses_an_explicit_disable_restart_label() {
        let prompt = network_confirmation(
            false,
            crate::sidecar::PlaybackRestartImpact {
                playback_active: false,
                queue_count: 1,
            },
        );
        assert!(prompt.message.contains("Playback is not active"));
        assert!(prompt.message.contains("1 queued track will be cleared"));
        assert_eq!(prompt.accept_label, "Disable and restart");
    }
}
