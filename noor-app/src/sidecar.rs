use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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
    let mut path = std::env::current_exe().expect("cannot determine current exe path");
    path.set_file_name(if cfg!(windows) {
        "noor-server.exe"
    } else {
        "noor-server"
    });
    path
}

fn log_path() -> PathBuf {
    std::env::current_exe()
        .expect("cannot determine current exe path")
        .parent()
        .expect("exe has no parent directory")
        .join("noor-server.log")
}

const MAX_LOG_BYTES: u64 = 50 * 1024 * 1024;

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

pub fn spawn_server(state: &Arc<SidecarState>) {
    let host_mode = *state.host_mode.lock().unwrap();
    let path = log_path();
    rotate_log_if_oversized(&path);
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
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

    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

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
        // Best-effort graceful shutdown. POST returns once the server has
        // flushed its in-flight listen session (and signaled axum to stop);
        // 1s is plenty for a localhost round-trip + DB write. If the server
        // is wedged or unreachable we fall through to child.kill().
        if let Ok(client) = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(1000))
            .build()
        {
            let _ = client
                .post("http://127.0.0.1:3334/api/shutdown")
                .send();
        }
        // Give the server up to 2s to exit cleanly after the shutdown
        // signal. Poll try_wait in 50ms ticks — std::process has no
        // wait_timeout and the wait-timeout crate isn't worth a dep here.
        let deadline = Instant::now() + Duration::from_millis(2000);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
            }
        }
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
