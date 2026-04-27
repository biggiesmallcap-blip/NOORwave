use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct SidecarState {
    pub child: Mutex<Option<Child>>,
    pub host_mode: Mutex<bool>,
    pub server_token: Mutex<Option<String>>,
}

impl SidecarState {
    pub fn new(host_mode: bool) -> Arc<Self> {
        Arc::new(Self {
            child: Mutex::new(None),
            host_mode: Mutex::new(host_mode),
            server_token: Mutex::new(None),
        })
    }
}

fn server_exe_path() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.set_file_name(if cfg!(windows) {
        "noor-server.exe"
    } else {
        "noor-server"
    });
    path
}

fn log_path() -> PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .join("noor-server.log")
}

pub fn spawn_server(state: &Arc<SidecarState>) {
    let host_mode = *state.host_mode.lock().unwrap();
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
        .ok();

    let mut cmd = Command::new(server_exe_path());
    if host_mode {
        cmd.arg("--host");
    }
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

    match cmd.spawn() {
        Ok(child) => {
            *state.child.lock().unwrap() = Some(child);
        }
        Err(e) => {
            eprintln!("Failed to spawn noor-server: {e}");
        }
    }
}

pub fn kill_server(state: &Arc<SidecarState>) {
    let mut guard = state.child.lock().unwrap();
    if let Some(mut child) = guard.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    *state.server_token.lock().unwrap() = None;
}

pub fn restart_server(state: &Arc<SidecarState>) {
    kill_server(state);
    std::thread::sleep(Duration::from_millis(200));
    spawn_server(state);
}

/// Blocks until noor-server responds to /api/ping (max 10 s).
/// Returns the server auth token fetched from /api/setup/token.
pub fn wait_for_ready(state: &Arc<SidecarState>) -> Option<String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(resp) = reqwest::blocking::get("http://127.0.0.1:3334/api/ping") {
            if resp.status().is_success() {
                if let Ok(r) = reqwest::blocking::get("http://127.0.0.1:3334/api/setup/token") {
                    if let Ok(body) = r.json::<serde_json::Value>() {
                        let token = body["token"].as_str().map(|s| s.to_owned());
                        *state.server_token.lock().unwrap() = token.clone();
                        return token;
                    }
                }
                return None;
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    None
}
