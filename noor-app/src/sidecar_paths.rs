use std::path::PathBuf;
use tauri::Manager;

pub struct SidecarPaths {
    pub binary: PathBuf,
    pub log: PathBuf,
    pub data: Option<PathBuf>,
    pub www: Option<PathBuf>,
}

impl SidecarPaths {
    pub fn resolve(handle: &tauri::AppHandle) -> Self {
        if crate::paths::is_installed_mode() {
            let resource_dir = handle.path().resource_dir().expect("resource dir");
            let data = crate::paths::data_dir();
            return Self {
                binary: resource_dir.join("noor-server.exe"),
                log: data.join("noor-server.log"),
                data: Some(data),
                www: Some(resource_dir.join("www")),
            };
        }

        let exe_dir = std::env::current_exe()
            .expect("cannot determine current exe path")
            .parent()
            .expect("exe has no parent directory")
            .to_owned();

        Self {
            binary: exe_dir.join(if cfg!(windows) {
                "noor-server.exe"
            } else {
                "noor-server"
            }),
            log: exe_dir.join("noor-server.log"),
            data: None,
            www: None,
        }
    }
}
