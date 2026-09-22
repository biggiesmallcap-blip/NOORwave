use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    pub host_mode: bool,
    #[serde(default)]
    pub minimize_to_tray: bool,
}

#[derive(Debug)]
pub struct ConfigError(String);

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

fn config_path() -> PathBuf {
    crate::paths::data_dir().join("noor-config.json")
}

fn write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub fn load() -> AppConfig {
    let Ok(_guard) = write_lock().lock() else {
        return AppConfig::default();
    };
    let path = config_path();
    let _ = recover_interrupted_replacement(&path);
    load_at(&path).unwrap_or_default()
}

fn recover_interrupted_replacement(path: &Path) -> Result<(), ConfigError> {
    let backup_path = path.with_extension("json.bak");
    if !backup_path.exists() {
        return Ok(());
    }

    if path.exists() {
        if load_at(path).is_ok() {
            return Ok(());
        }
    }
    load_at(&backup_path).map_err(|error| {
        ConfigError(format!(
            "cannot recover interrupted NOORwave config replacement because its backup is invalid: {error}"
        ))
    })?;
    if path.exists() {
        fs::remove_file(path).map_err(|error| {
            ConfigError(format!(
                "failed to remove an incomplete NOORwave config replacement: {error}"
            ))
        })?;
    }

    fs::rename(&backup_path, path).map_err(|error| {
        ConfigError(format!(
            "failed to recover prior NOORwave config after interrupted replacement: {error}"
        ))
    })
}

fn load_at(path: &Path) -> Result<AppConfig, ConfigError> {
    match fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents)
            .map_err(|error| ConfigError(format!("invalid NOORwave config: {error}"))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(AppConfig::default()),
        Err(error) => Err(ConfigError(format!(
            "failed to read NOORwave config: {error}"
        ))),
    }
}

pub fn update(mutator: impl FnOnce(&mut AppConfig)) -> Result<(), ConfigError> {
    update_at(&config_path(), mutator)
}

fn update_at(path: &Path, mutator: impl FnOnce(&mut AppConfig)) -> Result<(), ConfigError> {
    update_at_with_replace(path, mutator, |from, to| fs::rename(from, to))
}

fn update_at_with_replace(
    path: &Path,
    mutator: impl FnOnce(&mut AppConfig),
    replace: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<(), ConfigError> {
    let _guard = write_lock()
        .lock()
        .map_err(|_| ConfigError("NOORwave config writer is unavailable".to_owned()))?;
    recover_interrupted_replacement(path)?;

    let original = match fs::read_to_string(path) {
        Ok(contents) => Some(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(ConfigError(format!(
                "failed to read NOORwave config before update: {error}"
            )))
        }
    };
    let mut document = match original.as_deref() {
        Some(contents) => serde_json::from_str::<serde_json::Value>(contents)
            .map_err(|error| ConfigError(format!("invalid NOORwave config: {error}")))?,
        None => serde_json::json!({}),
    };
    let object = document
        .as_object_mut()
        .ok_or_else(|| ConfigError("NOORwave config must be a JSON object".to_owned()))?;
    let mut config = match original.as_deref() {
        Some(contents) => serde_json::from_str::<AppConfig>(contents)
            .map_err(|error| ConfigError(format!("invalid NOORwave config: {error}")))?,
        None => AppConfig::default(),
    };
    mutator(&mut config);
    let known = serde_json::to_value(config)
        .map_err(|error| ConfigError(format!("failed to serialize NOORwave config: {error}")))?;
    for (key, value) in known
        .as_object()
        .expect("AppConfig serializes to an object")
    {
        object.insert(key.clone(), value.clone());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| ConfigError(format!("failed to create config directory: {error}")))?;
    }
    let temp_path = path.with_extension("json.tmp");
    let backup_path = path.with_extension("json.bak");
    let bytes = serde_json::to_vec_pretty(&document)
        .map_err(|error| ConfigError(format!("failed to serialize NOORwave config: {error}")))?;
    let mut temp = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp_path)
        .map_err(|error| ConfigError(format!("failed to create temporary config: {error}")))?;
    temp.write_all(&bytes)
        .and_then(|_| temp.sync_all())
        .map_err(|error| ConfigError(format!("failed to write temporary config: {error}")))?;
    drop(temp);

    let had_original = path.exists();
    if backup_path.exists() {
        fs::remove_file(&backup_path).map_err(|error| {
            ConfigError(format!("failed to remove stale config backup: {error}"))
        })?;
    }
    if had_original {
        fs::rename(path, &backup_path)
            .map_err(|error| ConfigError(format!("failed to preserve prior config: {error}")))?;
    }
    if let Err(error) = replace(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        if had_original {
            fs::rename(&backup_path, path).map_err(|restore_error| {
                ConfigError(format!(
                    "config replacement failed ({error}) and prior config could not be restored ({restore_error})"
                ))
            })?;
        }
        return Err(ConfigError(format!(
            "failed to replace NOORwave config: {error}"
        )));
    }
    if had_original {
        // The replacement is already durable. A leftover backup is harmless
        // and can be removed by the next serialized write.
        let _ = fs::remove_file(&backup_path);
    }
    Ok(())
}

#[cfg(test)]
mod stage2_tests {
    use super::*;

    #[test]
    fn atomic_update_preserves_unrelated_fields() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("noor-config.json");
        fs::write(
            &path,
            r#"{"host_mode":false,"minimize_to_tray":true,"future_setting":{"x":1}}"#,
        )
        .expect("seed config");

        update_at(&path, |config| config.host_mode = true).expect("atomic update");

        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).expect("read saved config"))
                .expect("valid json");
        assert_eq!(saved["host_mode"], true);
        assert_eq!(saved["minimize_to_tray"], true);
        assert_eq!(saved["future_setting"]["x"], 1);
    }

    #[test]
    fn failed_replacement_preserves_prior_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("noor-config.json");
        fs::write(&path, r#"{"host_mode":false}"#).expect("seed config");

        let error = update_at_with_replace(
            &path,
            |config| config.host_mode = true,
            |_, _| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected replacement failure",
                ))
            },
        )
        .expect_err("replacement must fail");

        assert!(error.to_string().contains("injected replacement failure"));
        assert_eq!(
            fs::read_to_string(path).expect("read preserved config"),
            r#"{"host_mode":false}"#
        );
    }

    #[test]
    fn interrupted_replacement_recovers_the_prior_backup() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("noor-config.json");
        let backup = path.with_extension("json.bak");
        fs::write(&backup, r#"{"host_mode":true,"minimize_to_tray":true}"#).expect("seed backup");

        recover_interrupted_replacement(&path).expect("recover backup");

        let restored = load_at(&path).expect("load restored config");
        assert!(restored.host_mode);
        assert!(restored.minimize_to_tray);
        assert!(!backup.exists());
    }

    #[test]
    fn interrupted_replacement_recovers_backup_when_replacement_is_corrupt() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("noor-config.json");
        let backup = path.with_extension("json.bak");
        fs::write(&path, r#"{"host_mode":"#).expect("seed corrupt replacement");
        fs::write(&backup, r#"{"host_mode":false,"minimize_to_tray":true}"#)
            .expect("seed valid backup");

        recover_interrupted_replacement(&path).expect("recover valid backup");

        let restored = load_at(&path).expect("load restored config");
        assert!(!restored.host_mode);
        assert!(restored.minimize_to_tray);
        assert!(!backup.exists());
    }

    #[test]
    fn interrupted_replacement_does_not_promote_an_invalid_backup() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("noor-config.json");
        let backup = path.with_extension("json.bak");
        fs::write(&backup, r#"{"host_mode":"#).expect("seed corrupt backup");

        let error = recover_interrupted_replacement(&path).expect_err("reject corrupt backup");

        assert!(error.to_string().contains("backup"));
        assert!(!path.exists());
        assert!(backup.exists());
    }
}
