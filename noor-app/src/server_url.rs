//! Single source of truth for the local noor-server URL the shell talks to.
//! Honors a port embedded in `NOOR_ADDR` (the power-user override), then
//! `NOOR_PORT`, falling back to 17600 — so the listen port can be changed by env
//! without recompiling. Mirrors `noor_server::server::noor_port`; the spawned
//! sidecar inherits this process's env, so both sides agree automatically.

pub fn port() -> u16 {
    if let Ok(addr) = std::env::var("NOOR_ADDR") {
        if let Some(p) = addr
            .trim()
            .rsplit(':')
            .next()
            .and_then(|s| s.parse::<u16>().ok())
        {
            return p;
        }
    }
    std::env::var("NOOR_PORT")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(17600)
}

/// `http://127.0.0.1:<port>` — the loopback base the shell uses regardless of
/// host mode (the server may also bind 0.0.0.0, but loopback always works).
pub fn base() -> String {
    format!("http://127.0.0.1:{}", port())
}

/// `http://127.0.0.1:<port>/api/<path>`. `path` may include or omit a leading slash.
pub fn api(path: &str) -> String {
    format!("{}/api/{}", base(), path.trim_start_matches('/'))
}
