fn main() {
    // The UI uses a remote loopback origin, so every custom command needs an
    // explicit permission under Tauri's remote-origin ACL enforcement.
    let manifest = tauri_build::AppManifest::new().commands(&[
        "check_for_updates_now",
        "get_update_state",
        "install_pending_update",
        "get_install_mode",
        "get_minimize_to_tray",
        "set_minimize_to_tray",
        "get_remote_host_state",
        "set_remote_host_mode",
        "restart_managed_server",
        "get_startup_state",
        "set_start_at_login",
    ]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("failed to build Tauri app permissions");
}
