use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

#[tauri::command]
pub async fn check_for_updates_now(handle: AppHandle) -> Result<Option<String>, String> {
    if !crate::paths::is_installed_mode() {
        let info = tauri::async_runtime::spawn_blocking(crate::updater::check)
            .await
            .unwrap_or(None);
        return Ok(info.map(|i| i.version));
    }

    let updater = handle.updater().map_err(|e| e.to_string())?;
    updater
        .check()
        .await
        .map_err(|e| e.to_string())
        .map(|update| update.map(|u| u.version))
}

#[tauri::command]
pub fn get_update_state(state: tauri::State<'_, crate::tray::TrayMenuItems>) -> Option<String> {
    state
        .pending
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|(version, _)| version.clone()))
}

#[tauri::command]
pub fn get_install_mode() -> String {
    if crate::paths::is_installed_mode() {
        "Installed".to_owned()
    } else {
        "Portable".to_owned()
    }
}
