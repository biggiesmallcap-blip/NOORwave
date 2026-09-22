use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

#[tauri::command]
pub async fn check_for_updates_now(
    handle: AppHandle,
) -> Result<Option<crate::tray::UpdateDetails>, String> {
    if !crate::paths::is_installed_mode() {
        let info = tauri::async_runtime::spawn_blocking(crate::updater::check)
            .await
            .unwrap_or(None);
        let Some(info) = info else {
            return Ok(None);
        };
        let details = crate::tray::UpdateDetails {
            version: info.version.clone(),
            notes: info.notes.clone(),
            action: "download",
        };
        crate::tray::notify_update(
            &handle,
            info.version,
            info.notes,
            crate::tray::UpdateAction::OpenUrl(info.url),
        );
        return Ok(Some(details));
    }

    let updater = handle.updater().map_err(|e| e.to_string())?;
    let update = updater.check().await.map_err(|e| e.to_string())?;
    let Some(update) = update else {
        return Ok(None);
    };
    let notes = update
        .body
        .as_deref()
        .map(str::trim)
        .filter(|notes| !notes.is_empty())
        .map(str::to_owned);
    let details = crate::tray::UpdateDetails {
        version: update.version.clone(),
        notes: notes.clone(),
        action: "install",
    };
    crate::tray::notify_update(
        &handle,
        update.version,
        notes,
        crate::tray::UpdateAction::Install,
    );
    Ok(Some(details))
}

#[tauri::command]
pub fn get_update_state(
    state: tauri::State<'_, crate::tray::TrayMenuItems>,
) -> Option<crate::tray::UpdateDetails> {
    state
        .pending
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|(details, _)| details.clone()))
}

#[tauri::command]
pub async fn install_pending_update(
    handle: AppHandle,
    state: tauri::State<'_, crate::tray::TrayMenuItems>,
) -> Result<(), String> {
    let pending = {
        let guard = state.pending.lock().map_err(|e| e.to_string())?;
        guard.clone()
    };

    match pending {
        Some((_, crate::tray::UpdateAction::OpenUrl(url))) => {
            tauri_plugin_opener::open_url(url, None::<&str>).map_err(|e| e.to_string())
        }
        Some((_, crate::tray::UpdateAction::Install)) => {
            crate::installed_updater::install_now(&handle)
                .await
                .map_err(|e| e.to_string())
        }
        None => Err("No update is available.".to_owned()),
    }
}

#[tauri::command]
pub fn get_install_mode() -> String {
    if crate::paths::is_installed_mode() {
        "Installed".to_owned()
    } else {
        "Portable".to_owned()
    }
}

#[tauri::command]
pub fn get_minimize_to_tray() -> bool {
    crate::config::load().minimize_to_tray
}

#[tauri::command]
pub fn set_minimize_to_tray(value: bool) {
    let mut cfg = crate::config::load();
    cfg.minimize_to_tray = value;
    crate::config::save(&cfg);
}
