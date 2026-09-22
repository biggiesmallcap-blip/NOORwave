use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;

#[cfg(windows)]
const WINDOWS_RUN_KEY: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run";

pub const AUTOSTART_MARKER: &str = "--noor-autostart";

static ACTIVATION_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn request_activation() {
    ACTIVATION_REQUESTED.store(true, Ordering::Release);
}

pub fn take_activation_request() -> bool {
    ACTIVATION_REQUESTED.swap(false, Ordering::AcqRel)
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LaunchMode {
    Normal,
    Autostart,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StartupUnavailableReason {
    PortableBuild,
    UnsupportedPlatform,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DesktopStartupState {
    pub supported: bool,
    pub enabled: bool,
    pub launch_mode: LaunchMode,
    pub unavailable_reason: Option<StartupUnavailableReason>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StartupErrorCode {
    UnsupportedInstallMode,
    AutostartQueryFailed,
    AutostartEnableFailed,
    AutostartDisableFailed,
}

#[derive(Debug, Clone, Serialize)]
pub struct DesktopStartupError {
    pub code: StartupErrorCode,
    pub message: String,
    pub state: DesktopStartupState,
}

pub struct StartupRuntime {
    launch_mode: LaunchMode,
    last_observed: Mutex<Option<bool>>,
}

impl StartupRuntime {
    pub fn new(launch_mode: LaunchMode) -> Self {
        Self {
            launch_mode,
            last_observed: Mutex::new(None),
        }
    }

    fn unavailable_state(&self) -> DesktopStartupState {
        unavailable_state_for(
            cfg!(windows),
            crate::paths::is_installed_mode(),
            self.launch_mode,
        )
    }

    fn observed_state(&self, enabled: bool) -> DesktopStartupState {
        DesktopStartupState {
            supported: true,
            enabled,
            launch_mode: self.launch_mode,
            unavailable_reason: None,
        }
    }

    fn fallback_state(&self) -> DesktopStartupState {
        self.observed_state(self.last_observed.lock().unwrap().unwrap_or(false))
    }
}

pub fn launch_mode_from<I, S>(args: I) -> LaunchMode
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    if args
        .into_iter()
        .any(|argument| argument.as_ref() == AUTOSTART_MARKER)
    {
        LaunchMode::Autostart
    } else {
        LaunchMode::Normal
    }
}

fn unavailable_state_for(
    windows: bool,
    installed: bool,
    launch_mode: LaunchMode,
) -> DesktopStartupState {
    DesktopStartupState {
        supported: windows && installed,
        enabled: false,
        launch_mode,
        unavailable_reason: if !windows {
            Some(StartupUnavailableReason::UnsupportedPlatform)
        } else if !installed {
            Some(StartupUnavailableReason::PortableBuild)
        } else {
            None
        },
    }
}

fn registration_needs_repair(observed: &str, expected: &str) -> bool {
    observed.trim() != expected.trim()
}

#[cfg(windows)]
fn expected_registration_command() -> Result<String, String> {
    std::env::current_exe()
        .map(|path| format!("{} {AUTOSTART_MARKER}", path.display()))
        .map_err(|error| format!("could not resolve the installed executable: {error}"))
}

#[cfg(windows)]
fn registered_command(app_name: &str) -> Result<Option<String>, String> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    use winreg::RegKey;

    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(WINDOWS_RUN_KEY, KEY_READ)
        .map_err(|error| format!("could not inspect the NOORwave startup registration: {error}"))?;
    match key.get_value::<String, _>(app_name) {
        Ok(command) => Ok(Some(command)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "could not read the NOORwave startup registration: {error}"
        )),
    }
}

#[cfg(windows)]
fn repair_stale_registration(
    handle: &AppHandle,
    manager: &tauri_plugin_autostart::AutoLaunchManager,
) -> Result<bool, String> {
    let expected = expected_registration_command()?;
    let observed = registered_command(&handle.package_info().name)?;
    if observed
        .as_deref()
        .is_some_and(|command| registration_needs_repair(command, &expected))
    {
        // The official plugin owns this exact per-user value. Disable/enable
        // rewrites only NOORwave's registration and its explicit marker.
        manager.disable().map_err(|error| {
            format!("failed to remove stale start-at-sign-in registration: {error}")
        })?;
        manager
            .enable()
            .map_err(|error| format!("failed to repair start-at-sign-in registration: {error}"))?;
        let repaired = registered_command(&handle.package_info().name)?;
        if repaired
            .as_deref()
            .is_none_or(|command| registration_needs_repair(command, &expected))
        {
            return Err(
                "the repaired start-at-sign-in registration did not match this installation"
                    .to_owned(),
            );
        }
        return Ok(true);
    }
    Ok(false)
}

#[tauri::command]
pub fn get_startup_state(
    handle: AppHandle,
    runtime: tauri::State<'_, StartupRuntime>,
) -> Result<DesktopStartupState, DesktopStartupError> {
    if !cfg!(windows) || !crate::paths::is_installed_mode() {
        return Ok(runtime.unavailable_state());
    }
    match handle.autolaunch().is_enabled() {
        Ok(enabled) => {
            #[cfg(windows)]
            if enabled {
                let manager = handle.autolaunch();
                let repaired = repair_stale_registration(&handle, &manager).map_err(|error| {
                    DesktopStartupError {
                        code: StartupErrorCode::AutostartQueryFailed,
                        message: error,
                        state: runtime.fallback_state(),
                    }
                })?;
                if repaired {
                    match manager.is_enabled() {
                        Ok(true) => {}
                        Ok(false) => {
                            return Err(DesktopStartupError {
                                code: StartupErrorCode::AutostartQueryFailed,
                                message: "The stale start-at-sign-in registration was replaced but could not be verified."
                                    .to_owned(),
                                state: runtime.observed_state(false),
                            });
                        }
                        Err(error) => {
                            return Err(DesktopStartupError {
                                code: StartupErrorCode::AutostartQueryFailed,
                                message: format!(
                                    "The stale registration was replaced but verification failed: {error}"
                                ),
                                state: runtime.fallback_state(),
                            });
                        }
                    }
                }
            }
            *runtime.last_observed.lock().unwrap() = Some(enabled);
            Ok(runtime.observed_state(enabled))
        }
        Err(error) => Err(DesktopStartupError {
            code: StartupErrorCode::AutostartQueryFailed,
            message: format!("Failed to query start-at-sign-in registration: {error}"),
            state: runtime.fallback_state(),
        }),
    }
}

#[tauri::command]
pub fn set_start_at_login(
    handle: AppHandle,
    runtime: tauri::State<'_, StartupRuntime>,
    enabled: bool,
) -> Result<DesktopStartupState, DesktopStartupError> {
    if !cfg!(windows) || !crate::paths::is_installed_mode() {
        return Err(DesktopStartupError {
            code: StartupErrorCode::UnsupportedInstallMode,
            message: "Install NOORwave to enable start-at-sign-in".to_owned(),
            state: runtime.unavailable_state(),
        });
    }

    let manager = handle.autolaunch();
    let previous = match manager.is_enabled() {
        Ok(value) => {
            *runtime.last_observed.lock().unwrap() = Some(value);
            value
        }
        Err(error) => {
            return Err(DesktopStartupError {
                code: StartupErrorCode::AutostartQueryFailed,
                message: format!("Failed to query start-at-sign-in registration: {error}"),
                state: runtime.fallback_state(),
            })
        }
    };
    if previous == enabled {
        #[cfg(windows)]
        if enabled {
            repair_stale_registration(&handle, &manager).map_err(|error| DesktopStartupError {
                code: StartupErrorCode::AutostartQueryFailed,
                message: error,
                state: runtime.observed_state(previous),
            })?;
        }
        return Ok(runtime.observed_state(previous));
    }

    let operation = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    if let Err(error) = operation {
        return Err(DesktopStartupError {
            code: if enabled {
                StartupErrorCode::AutostartEnableFailed
            } else {
                StartupErrorCode::AutostartDisableFailed
            },
            message: format!("Failed to update start-at-sign-in registration: {error}"),
            state: runtime.observed_state(previous),
        });
    }

    match manager.is_enabled() {
        Ok(observed) if observed == enabled => {
            *runtime.last_observed.lock().unwrap() = Some(observed);
            Ok(runtime.observed_state(observed))
        }
        Ok(observed) => Err(DesktopStartupError {
            code: if enabled {
                StartupErrorCode::AutostartEnableFailed
            } else {
                StartupErrorCode::AutostartDisableFailed
            },
            message: "The operating system did not retain the requested start-at-sign-in state."
                .to_owned(),
            state: runtime.observed_state(observed),
        }),
        Err(error) => Err(DesktopStartupError {
            code: StartupErrorCode::AutostartQueryFailed,
            message: format!("Registration changed but could not be verified: {error}"),
            state: runtime.observed_state(previous),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_marker_is_the_only_silent_launch_mode() {
        assert_eq!(
            launch_mode_from(["noor-app.exe", AUTOSTART_MARKER]),
            LaunchMode::Autostart
        );
        assert_eq!(
            launch_mode_from(["noor-app.exe", "--some-other-flag"]),
            LaunchMode::Normal
        );
    }

    #[test]
    fn portable_windows_build_is_explicitly_unsupported() {
        assert_eq!(
            unavailable_state_for(true, false, LaunchMode::Normal),
            DesktopStartupState {
                supported: false,
                enabled: false,
                launch_mode: LaunchMode::Normal,
                unavailable_reason: Some(StartupUnavailableReason::PortableBuild),
            }
        );
    }

    #[test]
    fn stale_registration_detection_is_scoped_to_the_exact_expected_command() {
        assert!(!registration_needs_repair(
            r#"C:\Program Files\NOORwave\noor-app.exe --noor-autostart"#,
            r#"C:\Program Files\NOORwave\noor-app.exe --noor-autostart"#,
        ));
        assert!(registration_needs_repair(
            r#"C:\Old\noor-app.exe --noor-autostart"#,
            r#"C:\Program Files\NOORwave\noor-app.exe --noor-autostart"#,
        ));
        assert!(registration_needs_repair(
            r#"\"C:\Program Files\NOORwave\noor-app.exe\" --noor-autostart"#,
            r#"C:\Program Files\NOORwave\noor-app.exe --noor-autostart"#,
        ));
    }
}
