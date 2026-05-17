use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

pub fn background_check(handle: &AppHandle) {
    tauri::async_runtime::block_on(async {
        let Ok(updater) = handle.updater() else {
            return;
        };

        match updater.check().await {
            Ok(Some(update)) => {
                let version = update.version.clone();
                crate::tray::notify_update(
                    handle,
                    version.clone(),
                    crate::tray::UpdateAction::Install,
                );
                let _ = handle.emit("update-available", &version);
            }
            Ok(None) => {}
            Err(err) => {
                let message = err.to_string();
                eprintln!("update check failed: {message}");
                let _ = handle.emit("update-error", &message);
            }
        }
    });
}

pub async fn install_now(handle: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let updater = handle.updater()?;
    let Some(update) = updater.check().await? else {
        return Ok(());
    };

    if let Some(state) = handle.try_state::<std::sync::Arc<crate::sidecar::SidecarState>>() {
        crate::sidecar::kill_server(state.inner());
    }

    update
        .download_and_install(|_chunk, _total| {}, || {})
        .await?;
    handle.restart()
}
