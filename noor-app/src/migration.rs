use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

pub fn prompt_and_import(handle: &AppHandle) {
    let confirm = handle
        .dialog()
        .message("Import data from an existing portable NOORwave installation?")
        .title("NOORwave first run")
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::OkCancel)
        .blocking_show();
    if !confirm {
        return;
    }

    let picked = handle
        .dialog()
        .file()
        .add_filter("NOORwave database", &["db"])
        .set_title("Select your portable noor.db")
        .blocking_pick_file();
    let Some(src_db) = picked.and_then(|p| p.into_path().ok()) else {
        return;
    };
    let Some(src_dir) = src_db.parent() else {
        return;
    };

    let dst_dir = crate::paths::data_dir();
    let _ = std::fs::create_dir_all(&dst_dir);
    if let Err(err) = std::fs::copy(&src_db, dst_dir.join("noor.db")) {
        eprintln!("failed to import noor.db: {err}");
        return;
    }

    for sibling in [".noor_secret", "noor-config.json"] {
        let src = src_dir.join(sibling);
        if src.exists() {
            let _ = std::fs::copy(&src, dst_dir.join(sibling));
        }
    }
}
