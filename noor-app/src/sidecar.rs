use crate::sidecar_paths::SidecarPaths;
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct SidecarState {
    pub child: Mutex<Option<Child>>,
    pub host_mode: Mutex<bool>,
    pub server_token: Mutex<Option<String>>,
    pub paths: OnceLock<SidecarPaths>,
}

impl SidecarState {
    pub fn new(host_mode: bool) -> Arc<Self> {
        Arc::new(Self {
            child: Mutex::new(None),
            host_mode: Mutex::new(host_mode),
            server_token: Mutex::new(None),
            paths: OnceLock::new(),
        })
    }
}

const MAX_LOG_BYTES: u64 = 50 * 1024 * 1024;
const READY_REQUEST_TIMEOUT_MS: u64 = 500;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerRemoteStatus {
    pub control: String,
    pub configured_host_mode: bool,
    pub effective_host_mode: bool,
    pub state: String,
    pub bind_address: String,
    pub port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaybackRestartImpact {
    pub playback_active: bool,
    pub queue_count: usize,
}

fn playback_restart_impact_from_json(
    value: serde_json::Value,
) -> Result<PlaybackRestartImpact, String> {
    let playback_active = value
        .get("state")
        .and_then(|state| state.get("is_playing"))
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| "playback snapshot did not include is_playing".to_owned())?;
    let queue_count = value
        .get("queue")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .ok_or_else(|| "playback snapshot did not include the queue".to_owned())?;
    Ok(PlaybackRestartImpact {
        playback_active,
        queue_count,
    })
}

fn server_command(paths: &SidecarPaths, host_mode: bool) -> Command {
    let mut command = Command::new(&paths.binary);
    command.env(
        "NOOR_MANAGED_HOST_MODE",
        if host_mode { "true" } else { "false" },
    );
    if let Some(data) = &paths.data {
        command.env("NOOR_DATA_DIR", data);
    }
    if let Some(www) = &paths.www {
        command.env("NOOR_WWW_DIR", www);
    }
    command
}

fn ready_http_client() -> Option<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(READY_REQUEST_TIMEOUT_MS))
        .build()
        .ok()
}

fn rotate_log_if_oversized(path: &PathBuf) {
    let size = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(_) => return,
    };
    if size <= MAX_LOG_BYTES {
        return;
    }
    let rotated = path.with_extension("log.old");
    let _ = std::fs::remove_file(&rotated);
    let _ = std::fs::rename(path, &rotated);
}

fn should_shutdown_stale_server_before_spawn(has_owned_child: bool, server_ready: bool) -> bool {
    !has_owned_child && server_ready
}

fn localhost_server_is_ready(client: &reqwest::blocking::Client) -> bool {
    client
        .get(crate::server_url::api("ping"))
        .send()
        .is_ok_and(|resp| resp.status().is_success())
}

fn wait_until_localhost_server_stops(client: &reqwest::blocking::Client, deadline: Instant) {
    while Instant::now() < deadline {
        if !localhost_server_is_ready(client) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn shutdown_stale_server_before_spawn(state: &Arc<SidecarState>, deadline: Instant) {
    let has_owned_child = state.child.lock().unwrap().is_some();
    let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(std::cmp::min(
            deadline.saturating_duration_since(Instant::now()),
            Duration::from_millis(500),
        ))
        .build()
    else {
        return;
    };
    if !should_shutdown_stale_server_before_spawn(
        has_owned_child,
        localhost_server_is_ready(&client),
    ) {
        return;
    }
    let _ = client.post(crate::server_url::api("shutdown")).send();
    wait_until_localhost_server_stops(&client, deadline);
}

pub fn spawn_server_until(state: &Arc<SidecarState>, deadline: Instant) -> Result<(), String> {
    if Instant::now() >= deadline {
        return Err("transition deadline exhausted before server spawn".to_owned());
    }
    shutdown_stale_server_before_spawn(state, deadline);
    if Instant::now() >= deadline {
        return Err("transition deadline exhausted while stopping a stale server".to_owned());
    }

    let host_mode = *state.host_mode.lock().unwrap();
    let paths = state.paths.get().expect("sidecar paths resolved");
    let path = paths.log.clone();
    rotate_log_if_oversized(&path);
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok();

    let mut cmd = server_command(paths, host_mode);
    if let Some(f) = &log_file {
        let stderr = f.try_clone().ok();
        let stdout = f.try_clone().ok();
        if let (Some(out), Some(err)) = (stdout, stderr) {
            cmd.stdout(out).stderr(err);
        } else {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
    } else {
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
    }

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    match cmd.spawn() {
        Ok(child) => {
            *state.child.lock().unwrap() = Some(child);
            Ok(())
        }
        Err(e) => Err(format!("failed to spawn noor-server: {e}")),
    }
}

pub fn spawn_server(state: &Arc<SidecarState>) -> Result<(), String> {
    spawn_server_until(state, Instant::now() + Duration::from_secs(3))
}

fn force_terminate_until(child: &mut Child, deadline: Instant) -> Result<(), String> {
    if let Err(error) = child.kill() {
        // The process can exit naturally between the preceding try_wait and
        // this kill. Treat that race as success, but retain a genuinely live
        // child for the caller to track and report.
        return match child.try_wait() {
            Ok(Some(_)) => Ok(()),
            _ => Err(format!("failed to terminate server: {error}")),
        };
    }
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) if Instant::now() >= deadline => {
                return Err(
                    "server did not exit after forced termination before the deadline".to_owned(),
                );
            }
            Ok(None) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                std::thread::sleep(std::cmp::min(remaining, Duration::from_millis(25)));
            }
            Err(error) => {
                return Err(format!(
                    "failed to inspect forcibly terminated server: {error}"
                ));
            }
        }
    }
}

pub fn kill_server_until(state: &Arc<SidecarState>, deadline: Instant) -> Result<(), String> {
    let mut guard = state.child.lock().unwrap();
    let result = if let Some(mut child) = guard.take() {
        // Best-effort graceful shutdown. POST returns once the server has
        // flushed its in-flight listen session (and signaled axum to stop);
        // 1s is plenty for a localhost round-trip + DB write. If the server
        // is wedged or unreachable we fall through to child.kill().
        let remaining = deadline.saturating_duration_since(Instant::now());
        if !remaining.is_zero() {
            if let Ok(client) = reqwest::blocking::Client::builder()
                .timeout(std::cmp::min(remaining, Duration::from_millis(1000)))
                .build()
            {
                let _ = client.post(crate::server_url::api("shutdown")).send();
            }
        }
        // Preserve a small part of the caller's budget for forced termination.
        // Child::wait is deliberately avoided: it has no deadline and could
        // violate the lifecycle's 15-second wall-clock contract.
        let force_at = deadline
            .checked_sub(Duration::from_millis(500))
            .unwrap_or(deadline);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break Ok(()),
                Ok(None) if Instant::now() >= force_at => {
                    let result = force_terminate_until(&mut child, deadline);
                    if result.is_err() {
                        *guard = Some(child);
                    }
                    break result;
                }
                Ok(None) => {
                    let remaining = force_at.saturating_duration_since(Instant::now());
                    std::thread::sleep(std::cmp::min(remaining, Duration::from_millis(50)));
                }
                Err(error) => {
                    let result =
                        force_terminate_until(&mut child, deadline).map_err(|kill_error| {
                            format!("failed to inspect server ({error}); {kill_error}")
                        });
                    if result.is_err() {
                        *guard = Some(child);
                    }
                    break result;
                }
            }
        }
    } else {
        Ok(())
    };
    *state.server_token.lock().unwrap() = None;
    result
}

pub fn kill_server(state: &Arc<SidecarState>) {
    let _ = kill_server_until(state, Instant::now() + Duration::from_secs(3));
}

/// Wait for the managed server to expose both its refreshed setup PIN and the
/// actual listener facts required by the desktop transition contract.
pub fn wait_for_remote_ready(
    state: &Arc<SidecarState>,
    expected_host_mode: bool,
    deadline: Instant,
) -> Result<ServerRemoteStatus, String> {
    let client =
        ready_http_client().ok_or_else(|| "failed to create readiness client".to_owned())?;
    let mut last_error = "server did not become ready".to_owned();
    while Instant::now() < deadline {
        let request_timeout = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(READY_REQUEST_TIMEOUT_MS));
        match client
            .get(crate::server_url::api("ping"))
            .timeout(request_timeout)
            .send()
        {
            Ok(response) if response.status().is_success() => {
                let request_timeout = deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(READY_REQUEST_TIMEOUT_MS));
                if request_timeout.is_zero() {
                    break;
                }
                let token = match client
                    .get(crate::server_url::api("setup/token"))
                    .timeout(request_timeout)
                    .send()
                {
                    Ok(response) if response.status().is_success() => response
                        .json::<serde_json::Value>()
                        .ok()
                        .and_then(|body| body["token"].as_str().map(str::to_owned)),
                    Ok(response) => {
                        last_error = format!("setup token returned HTTP {}", response.status());
                        None
                    }
                    Err(error) => {
                        last_error = format!("setup token request failed: {error}");
                        None
                    }
                };
                if let Some(token) = token {
                    *state.server_token.lock().unwrap() = Some(token.clone());
                    let request_timeout = deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(READY_REQUEST_TIMEOUT_MS));
                    if request_timeout.is_zero() {
                        break;
                    }
                    match client
                        .get(crate::server_url::api("server/remote"))
                        .bearer_auth(token)
                        .timeout(request_timeout)
                        .send()
                    {
                        Ok(response) if response.status().is_success() => {
                            match response.json::<ServerRemoteStatus>() {
                                Ok(status)
                                    if status.control == "desktop"
                                        && status.configured_host_mode == expected_host_mode
                                        && status.effective_host_mode == expected_host_mode =>
                                {
                                    return Ok(status);
                                }
                                Ok(status) => {
                                    last_error = format!(
                                        "managed listener state mismatch: expected {expected_host_mode}, control={}, configured={}, effective={}",
                                        status.control,
                                        status.configured_host_mode,
                                        status.effective_host_mode
                                    );
                                }
                                Err(error) => {
                                    last_error = format!("invalid remote status response: {error}");
                                }
                            }
                        }
                        Ok(response) => {
                            last_error =
                                format!("remote status returned HTTP {}", response.status());
                        }
                        Err(error) => {
                            last_error = format!("remote status request failed: {error}");
                        }
                    }
                }
            }
            Ok(response) => last_error = format!("ping returned HTTP {}", response.status()),
            Err(error) => last_error = format!("ping failed: {error}"),
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        std::thread::sleep(std::cmp::min(remaining, Duration::from_millis(200)));
    }
    Err(last_error)
}

pub fn fetch_remote_status(state: &Arc<SidecarState>) -> Result<ServerRemoteStatus, String> {
    let token = state
        .server_token
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "server setup PIN is unavailable".to_owned())?;
    let client =
        ready_http_client().ok_or_else(|| "failed to create readiness client".to_owned())?;
    let response = client
        .get(crate::server_url::api("server/remote"))
        .bearer_auth(token)
        .send()
        .map_err(|error| format!("remote status request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("remote status returned HTTP {}", response.status()));
    }
    response
        .json::<ServerRemoteStatus>()
        .map_err(|error| format!("invalid remote status response: {error}"))
}

pub fn fetch_playback_restart_impact(
    state: &Arc<SidecarState>,
) -> Result<PlaybackRestartImpact, String> {
    let token = state
        .server_token
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "server setup PIN is unavailable".to_owned())?;
    let client =
        ready_http_client().ok_or_else(|| "failed to create playback client".to_owned())?;
    let response = client
        .get(crate::server_url::api("playback/state"))
        .bearer_auth(token)
        .send()
        .map_err(|error| format!("playback snapshot request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "playback snapshot returned HTTP {}",
            response.status()
        ));
    }
    let value = response
        .json::<serde_json::Value>()
        .map_err(|error| format!("invalid playback snapshot response: {error}"))?;
    playback_restart_impact_from_json(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_shutdown_only_runs_when_no_owned_child_is_tracked() {
        assert!(should_shutdown_stale_server_before_spawn(false, true));
        assert!(!should_shutdown_stale_server_before_spawn(true, true));
        assert!(!should_shutdown_stale_server_before_spawn(false, false));
    }

    #[test]
    fn ready_http_client_builds_with_timeout() {
        assert!(ready_http_client().is_some());
    }

    #[test]
    fn managed_launch_always_passes_explicit_true_or_false_without_host_flag() {
        let paths = SidecarPaths {
            binary: PathBuf::from("noor-server"),
            log: PathBuf::from("noor-server.log"),
            data: None,
            www: None,
        };
        for (host_mode, expected) in [(false, "false"), (true, "true")] {
            let command = server_command(&paths, host_mode);
            assert_eq!(command.get_args().count(), 0);
            let managed = command
                .get_envs()
                .find(|(key, _)| *key == "NOOR_MANAGED_HOST_MODE")
                .and_then(|(_, value)| value)
                .and_then(|value| value.to_str());
            assert_eq!(managed, Some(expected));
        }
    }

    #[test]
    fn playback_restart_impact_uses_actual_snapshot_values() {
        let impact = playback_restart_impact_from_json(
            serde_json::json!({ "state": { "is_playing": true }, "queue": [{"id": 1}, {"id": 2}] }),
        )
        .expect("valid snapshot");
        assert!(impact.playback_active);
        assert_eq!(impact.queue_count, 2);
    }
}
