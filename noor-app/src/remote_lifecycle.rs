use crate::sidecar::{self, ServerRemoteStatus, SidecarState};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

const TRANSITION_TIMEOUT: Duration = Duration::from_secs(15);
const ENABLE_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(6);
const CLEANUP_RESERVE: Duration = Duration::from_secs(4);
const DESKTOP_CAPABILITY_PORT: u16 = 17600;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RemotePhase {
    Idle,
    Restarting,
    Recovering,
    Failed,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DesktopRemoteState {
    pub configured_host_mode: bool,
    pub phase: RemotePhase,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RemoteErrorCode {
    TransitionInProgress,
    ExternalBindOverride,
    ConfigWriteFailed,
    ServerStartFailed,
    StateMismatch,
}

#[derive(Debug, Clone, Serialize)]
pub struct DesktopRemoteError {
    pub code: RemoteErrorCode,
    pub message: String,
    pub state: DesktopRemoteState,
}

pub struct RemoteLifecycle {
    sidecar: Arc<SidecarState>,
    transitioning: AtomicBool,
    state: Mutex<DesktopRemoteState>,
}

#[derive(Debug)]
struct TransitionPermit<'a>(&'a AtomicBool);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailureRecovery {
    RestoreLocal,
    FailClosed,
}

fn failure_recovery(previous: bool, requested: bool) -> FailureRecovery {
    if !previous && requested {
        FailureRecovery::RestoreLocal
    } else {
        FailureRecovery::FailClosed
    }
}

fn desktop_capability_override(
    noor_addr: Option<&str>,
    noor_port: Option<&str>,
) -> Option<&'static str> {
    if noor_addr.is_some_and(|value| !value.trim().is_empty()) {
        return Some("NOOR_ADDR");
    }
    noor_port
        .and_then(|value| value.trim().parse::<u16>().ok())
        .filter(|port| *port != DESKTOP_CAPABILITY_PORT)
        .map(|_| "NOOR_PORT")
}

fn enable_attempt_deadline(operation_started: Instant, deadline: Instant) -> Instant {
    std::cmp::min(
        operation_started + ENABLE_ATTEMPT_TIMEOUT,
        recovery_attempt_deadline(deadline),
    )
}

fn recovery_attempt_deadline(deadline: Instant) -> Instant {
    deadline.checked_sub(CLEANUP_RESERVE).unwrap_or(deadline)
}

fn bounded_cleanup_deadline(deadline: Instant) -> Instant {
    std::cmp::min(Instant::now() + Duration::from_secs(3), deadline)
}

fn reload_main_window_after_response(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        if let Some(window) = handle.get_webview_window("main") {
            // location.reload preserves the current URL, including the
            // Settings anchor that initiated the native transition.
            let _ = window.eval("window.location.reload()");
        }
    });
}

impl Drop for TransitionPermit<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl RemoteLifecycle {
    pub fn new(sidecar: Arc<SidecarState>, configured_host_mode: bool) -> Arc<Self> {
        Arc::new(Self {
            sidecar,
            transitioning: AtomicBool::new(false),
            state: Mutex::new(DesktopRemoteState {
                configured_host_mode,
                phase: RemotePhase::Idle,
                last_error: None,
            }),
        })
    }

    pub fn sidecar(&self) -> &Arc<SidecarState> {
        &self.sidecar
    }

    pub fn snapshot(&self) -> DesktopRemoteState {
        self.state.lock().unwrap().clone()
    }

    fn try_begin(&self) -> Result<TransitionPermit<'_>, DesktopRemoteError> {
        self.transitioning
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| TransitionPermit(&self.transitioning))
            .map_err(|_| {
                self.error(
                    RemoteErrorCode::TransitionInProgress,
                    "A server restart is already in progress.",
                )
            })
    }

    fn error(&self, code: RemoteErrorCode, message: impl Into<String>) -> DesktopRemoteError {
        DesktopRemoteError {
            code,
            message: message.into(),
            state: self.snapshot(),
        }
    }

    fn publish(
        &self,
        app: &AppHandle,
        configured: bool,
        phase: RemotePhase,
        last_error: Option<String>,
    ) {
        let state = DesktopRemoteState {
            configured_host_mode: configured,
            phase,
            last_error,
        };
        *self.state.lock().unwrap() = state.clone();
        let _ = app.emit("remote-host-state-changed", &state);
        crate::tray::set_network_checked(app, configured);
    }

    pub fn record_initial_result(&self, result: &Result<ServerRemoteStatus, String>) {
        let configured = *self.sidecar.host_mode.lock().unwrap();
        *self.state.lock().unwrap() = DesktopRemoteState {
            configured_host_mode: configured,
            phase: if result.is_ok() {
                RemotePhase::Idle
            } else {
                RemotePhase::Failed
            },
            last_error: result.as_ref().err().cloned(),
        };
    }

    pub fn mark_initial_start(&self) {
        let configured = *self.sidecar.host_mode.lock().unwrap();
        *self.state.lock().unwrap() = DesktopRemoteState {
            configured_host_mode: configured,
            phase: RemotePhase::Restarting,
            last_error: None,
        };
    }

    pub fn publish_initial_result(
        &self,
        app: &AppHandle,
        result: &Result<ServerRemoteStatus, String>,
    ) {
        self.record_initial_result(result);
        let state = self.snapshot();
        let _ = app.emit("remote-host-state-changed", &state);
        crate::tray::set_network_checked(app, state.configured_host_mode);
    }

    pub fn adopt_initial_config(&self, configured: bool) {
        *self.sidecar.host_mode.lock().unwrap() = configured;
        self.state.lock().unwrap().configured_host_mode = configured;
    }
}

#[tauri::command]
pub fn get_remote_host_state(
    lifecycle: tauri::State<'_, Arc<RemoteLifecycle>>,
) -> DesktopRemoteState {
    lifecycle.snapshot()
}

#[tauri::command]
pub async fn set_remote_host_mode(
    handle: AppHandle,
    lifecycle: tauri::State<'_, Arc<RemoteLifecycle>>,
    enabled: bool,
) -> Result<ServerRemoteStatus, DesktopRemoteError> {
    set_remote_host_mode_native(handle, lifecycle.inner().clone(), enabled).await
}

pub async fn set_remote_host_mode_native(
    handle: AppHandle,
    lifecycle: Arc<RemoteLifecycle>,
    enabled: bool,
) -> Result<ServerRemoteStatus, DesktopRemoteError> {
    transition(handle, lifecycle, enabled, false).await
}

#[tauri::command]
pub async fn restart_managed_server(
    handle: AppHandle,
    lifecycle: tauri::State<'_, Arc<RemoteLifecycle>>,
) -> Result<ServerRemoteStatus, DesktopRemoteError> {
    restart_managed_server_native(handle, lifecycle.inner().clone()).await
}

pub async fn restart_managed_server_native(
    handle: AppHandle,
    lifecycle: Arc<RemoteLifecycle>,
) -> Result<ServerRemoteStatus, DesktopRemoteError> {
    let enabled = lifecycle.snapshot().configured_host_mode;
    transition(handle, lifecycle, enabled, true).await
}

async fn transition(
    handle: AppHandle,
    lifecycle: Arc<RemoteLifecycle>,
    enabled: bool,
    force_restart: bool,
) -> Result<ServerRemoteStatus, DesktopRemoteError> {
    let _permit = lifecycle.try_begin()?;
    let operation_started = Instant::now();
    let noor_addr = std::env::var("NOOR_ADDR").ok();
    let noor_port = std::env::var("NOOR_PORT").ok();
    if let Some(source) = desktop_capability_override(noor_addr.as_deref(), noor_port.as_deref()) {
        return Err(lifecycle.error(
            RemoteErrorCode::ExternalBindOverride,
            format!(
                "Server binding is controlled by {source}; desktop controls require http://127.0.0.1:{DESKTOP_CAPABILITY_PORT}."
            ),
        ));
    }

    let previous = lifecycle.snapshot().configured_host_mode;
    if previous == enabled && !force_restart {
        let sidecar = lifecycle.sidecar.clone();
        return tauri::async_runtime::spawn_blocking(move || {
            sidecar::fetch_remote_status(&sidecar)
        })
        .await
        .map_err(|error| lifecycle.error(RemoteErrorCode::StateMismatch, error.to_string()))?
        .map_err(|error| lifecycle.error(RemoteErrorCode::StateMismatch, error));
    }

    if previous != enabled {
        tauri::async_runtime::spawn_blocking(move || {
            crate::config::update(|config| config.host_mode = enabled)
        })
        .await
        .map_err(|error| lifecycle.error(RemoteErrorCode::ConfigWriteFailed, error.to_string()))?
        .map_err(|error| lifecycle.error(RemoteErrorCode::ConfigWriteFailed, error.to_string()))?;
    }

    lifecycle.publish(&handle, enabled, RemotePhase::Restarting, None);
    *lifecycle.sidecar.host_mode.lock().unwrap() = enabled;
    let deadline = operation_started + TRANSITION_TIMEOUT;
    let attempt_deadline = if failure_recovery(previous, enabled) == FailureRecovery::RestoreLocal {
        enable_attempt_deadline(operation_started, deadline)
    } else {
        recovery_attempt_deadline(deadline)
    };
    let sidecar = lifecycle.sidecar.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        sidecar::kill_server_until(&sidecar, attempt_deadline)?;
        sidecar::spawn_server_until(&sidecar, attempt_deadline)?;
        sidecar::wait_for_remote_ready(&sidecar, enabled, attempt_deadline)
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|result| result);

    match result {
        Ok(status) => {
            lifecycle.publish(&handle, enabled, RemotePhase::Idle, None);
            reload_main_window_after_response(&handle);
            Ok(status)
        }
        Err(message) if failure_recovery(previous, enabled) == FailureRecovery::RestoreLocal => {
            lifecycle.publish(
                &handle,
                false,
                RemotePhase::Recovering,
                Some(message.clone()),
            );
            let failed_sidecar = lifecycle.sidecar.clone();
            let failed_cleanup_deadline =
                bounded_cleanup_deadline(recovery_attempt_deadline(deadline));
            let failed_attempt_cleanup = tauri::async_runtime::spawn_blocking(move || {
                sidecar::kill_server_until(&failed_sidecar, failed_cleanup_deadline)
            })
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result);
            let restore = tauri::async_runtime::spawn_blocking(|| {
                crate::config::update(|config| config.host_mode = false)
            })
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result.map_err(|error| error.to_string()));
            let restore_succeeded = restore.is_ok();
            // Recovery must launch explicitly local-only. If persistence could
            // not be restored, this temporary value is reconciled below with
            // the still-enabled persisted preference.
            *lifecycle.sidecar.host_mode.lock().unwrap() = false;
            let sidecar = lifecycle.sidecar.clone();
            let recovery_deadline = recovery_attempt_deadline(deadline);
            let recovery = if let Err(cleanup_error) = failed_attempt_cleanup {
                Err(format!(
                    "failed to stop the unverified LAN server: {cleanup_error}"
                ))
            } else if !restore_succeeded {
                Err(restore
                    .as_ref()
                    .err()
                    .cloned()
                    .unwrap_or_else(|| "failed to restore local-only preference".to_owned()))
            } else if Instant::now() < recovery_deadline {
                tauri::async_runtime::spawn_blocking(move || {
                    sidecar::spawn_server_until(&sidecar, recovery_deadline)?;
                    match sidecar::wait_for_remote_ready(&sidecar, false, recovery_deadline) {
                        Ok(status) => Ok(status),
                        Err(error) => match sidecar::kill_server_until(&sidecar, deadline) {
                            Ok(()) => Err(error),
                            Err(cleanup_error) => Err(format!(
                                "{error} Failed to stop the recovery server: {cleanup_error}"
                            )),
                        },
                    }
                })
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result)
                .map(|_| ())
            } else {
                Err("transition deadline exhausted before local-only recovery".to_owned())
            };
            let detail = match recovery {
                Ok(()) => {
                    reload_main_window_after_response(&handle);
                    format!("{message} Local-only service was restored.")
                }
                Err(recovery_error) => {
                    format!("{message} Local-only recovery failed: {recovery_error}")
                }
            };
            let persisted_mode = if restore_succeeded { false } else { enabled };
            *lifecycle.sidecar.host_mode.lock().unwrap() = persisted_mode;
            lifecycle.publish(
                &handle,
                persisted_mode,
                RemotePhase::Failed,
                Some(detail.clone()),
            );
            Err(lifecycle.error(RemoteErrorCode::ServerStartFailed, detail))
        }
        Err(message) => {
            // A replacement that did not pass readiness is never retained as
            // an unverified owned child. This is mandatory for disable and is
            // the safer result for an explicit restart of enabled hosting.
            let failed_sidecar = lifecycle.sidecar.clone();
            let cleanup_deadline = bounded_cleanup_deadline(deadline);
            let cleanup = tauri::async_runtime::spawn_blocking(move || {
                sidecar::kill_server_until(&failed_sidecar, cleanup_deadline)
            })
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result);
            let detail = match cleanup {
                Ok(()) => message,
                Err(cleanup_error) => {
                    format!("{message} Failed to stop the unverified server: {cleanup_error}")
                }
            };
            lifecycle.publish(&handle, enabled, RemotePhase::Failed, Some(detail.clone()));
            let code = if detail.contains("state mismatch") {
                RemoteErrorCode::StateMismatch
            } else {
                RemoteErrorCode::ServerStartFailed
            };
            Err(lifecycle.error(code, detail))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_permit_rejects_a_second_surface() {
        let sidecar = SidecarState::new(false);
        let lifecycle = RemoteLifecycle::new(sidecar, false);
        let first = lifecycle.try_begin().expect("first operation starts");
        let second = lifecycle
            .try_begin()
            .expect_err("second operation rejected");
        assert_eq!(second.code, RemoteErrorCode::TransitionInProgress);
        drop(first);
        assert!(lifecycle.try_begin().is_ok());
    }

    #[test]
    fn state_uses_the_configured_mode_not_an_optimistic_request() {
        let sidecar = SidecarState::new(false);
        let lifecycle = RemoteLifecycle::new(sidecar, false);
        assert_eq!(
            lifecycle.snapshot(),
            DesktopRemoteState {
                configured_host_mode: false,
                phase: RemotePhase::Idle,
                last_error: None,
            }
        );
    }

    #[test]
    fn failed_enable_recovers_local_but_failed_disable_stays_closed() {
        assert_eq!(failure_recovery(false, true), FailureRecovery::RestoreLocal);
        assert_eq!(failure_recovery(true, false), FailureRecovery::FailClosed);
        assert_eq!(failure_recovery(false, false), FailureRecovery::FailClosed);
    }

    #[test]
    fn desktop_capability_rejects_external_addresses_and_nondefault_ports() {
        assert_eq!(
            desktop_capability_override(Some("127.0.0.1:17600"), Some("17600")),
            Some("NOOR_ADDR")
        );
        assert_eq!(
            desktop_capability_override(None, Some("17601")),
            Some("NOOR_PORT")
        );
        assert_eq!(desktop_capability_override(None, Some("17600")), None);
        assert_eq!(desktop_capability_override(None, Some("invalid")), None);
        assert_eq!(desktop_capability_override(None, None), None);
    }

    #[test]
    fn failed_enable_reserves_time_to_stop_unverified_recovery_child() {
        let started = Instant::now();
        let deadline = started + TRANSITION_TIMEOUT;
        let enable_deadline = enable_attempt_deadline(started, deadline);
        let recovery_deadline = recovery_attempt_deadline(deadline);

        assert!(enable_deadline < recovery_deadline);
        assert_eq!(deadline.duration_since(recovery_deadline), CLEANUP_RESERVE);
    }

    #[test]
    fn every_attempt_deadline_reserves_final_cleanup_inside_fifteen_seconds() {
        let started = Instant::now();
        let deadline = started + TRANSITION_TIMEOUT;
        for attempt in [
            enable_attempt_deadline(started, deadline),
            recovery_attempt_deadline(deadline),
        ] {
            assert!(attempt <= deadline - CLEANUP_RESERVE);
        }
        assert_eq!(deadline.duration_since(started), Duration::from_secs(15));
        assert!(bounded_cleanup_deadline(deadline) <= deadline);
    }
}
