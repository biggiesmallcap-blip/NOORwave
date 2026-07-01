use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    pub host_mode: bool,
    // When true, closing the window hides NOORwave to the tray instead of
    // quitting. Default false: the window close button quits the app. serde
    // default keeps existing configs (which only have host_mode) loading.
    #[serde(default)]
    pub minimize_to_tray: bool,
}

fn config_path() -> PathBuf {
    crate::paths::data_dir().join("noor-config.json")
}

pub fn load() -> AppConfig {
    let path = config_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(cfg: &AppConfig) {
    let path = config_path();
    if let Ok(json) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(path, json);
    }
}
