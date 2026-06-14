pub mod radio_pipeline;
pub mod routes;
pub mod ws;

use crate::SharedState;
use anyhow::Result;
use axum::{
    Extension, Router,
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{Json, Response},
    routing::{get, post},
};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Notify;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

pub async fn start(state: SharedState, addr: &str) -> Result<()> {
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin, _| {
            is_trusted_local_origin_value(origin)
        }))
        .allow_methods(Any)
        .allow_headers(Any);

    // Resolve www/ before building the router so the fallback can go on the
    // public router (no auth). If it were on the outer merged router, axum's
    // Router::layer would run require_token even for unmatched routes,
    // causing static file requests to return 401.
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let www_dir = std::env::var("NOOR_WWW_DIR")
        .ok()
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(|| {
            exe_dir.as_ref().and_then(|d| {
                let p = d.join("www");
                if p.is_dir() { Some(p) } else { None }
            })
        })
        .or_else(|| {
            // Dev fallback: walk up from the exe looking for frontend/build/.
            // Lets `cargo build && target/release/noor-app.exe` work without
            // copying www/ into target/. Portable builds hit the www/ branch
            // above first and never reach this.
            let mut cursor = exe_dir.as_deref();
            while let Some(dir) = cursor {
                let candidate = dir.join("frontend").join("build");
                if candidate.is_dir() {
                    return Some(candidate);
                }
                cursor = dir.parent();
            }
            None
        });

    // Coordinates graceful shutdown: the /api/shutdown handler triggers
    // notify_one(); axum's with_graceful_shutdown future awaits it (or
    // ctrl_c when running standalone). Notify is idempotent — multiple
    // shutdown POSTs collapse to a single trigger.
    let shutdown_notify = Arc::new(Notify::new());

    // Public — no auth required. Static file serving lives here so it is
    // never touched by the require_token middleware.
    let public_base = Router::new()
        .route("/api/ping", get(ping_handler))
        .route("/api/setup/token", get(setup_token_handler))
        .route("/api/setup/onboarding", get(onboarding_status_handler))
        .route(
            "/api/setup/onboarding/complete",
            post(onboarding_complete_handler),
        )
        .route("/api/shutdown", post(shutdown_handler))
        .with_state(state.clone())
        .layer(Extension(shutdown_notify.clone()));

    let public = match www_dir {
        Some(www) => public_base
            .fallback_service(static_assets_service(&www))
            .layer(axum::middleware::from_fn(no_store_cache)),
        None => public_base,
    };

    // Protected — all routes require a valid Bearer token (or ?token= for WS).
    // Use route_layer, NOT layer: Router::layer also wraps the fallback, which
    // means the auth middleware would intercept every unmatched path (including
    // static file requests) after merge and return 401. route_layer only runs
    // for paths that actually match a protected route.
    let protected = Router::new()
        .merge(routes::api_routes(state.clone()))
        .merge(ws::ws_routes(state.clone()))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_token,
        ));

    // Compress responses (gzip/brotli, negotiated via Accept-Encoding). Large JSON
    // payloads - library lists, the ~100KB genre galaxy snapshot, recommendations -
    // shrink 40-60% on the wire. tower-http's default predicate skips already-
    // compressed content types (images, etc.), so artwork/audio aren't recompressed.
    let app = public
        .merge(protected)
        .layer(cors)
        .layer(CompressionLayer::new());

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        // ctrl_c arm only fires when running standalone (`cargo run -p
        // noor-server`); in the bundled Tauri build the child has no
        // controlling terminal, so the registered handler never fires —
        // shutdown comes via /api/shutdown -> notify.
        tokio::select! {
            _ = shutdown_notify.notified() => {},
            _ = tokio::signal::ctrl_c() => {},
        }
    })
    .await?;
    Ok(())
}

fn static_assets_service(www: &std::path::Path) -> ServeDir<ServeFile> {
    ServeDir::new(www).fallback(ServeFile::new(www.join("index.html")))
}

async fn shutdown_handler(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Extension(shutdown_notify): Extension<Arc<Notify>>,
    headers: HeaderMap,
) -> StatusCode {
    if require_public_loopback_request(addr, &headers).is_err() {
        return StatusCode::FORBIDDEN;
    }
    // Signal axum's graceful shutdown FIRST so a stuck write lock can't keep
    // the server alive — the listener stops accepting new connections while
    // we attempt the flush. If the lock is contended, the Tauri sidecar's
    // 1s POST timeout drops us and falls through to child.kill().
    shutdown_notify.notify_one();
    let mut s = state.write().await;
    if let Err(err) = routes::flush_active_listen_session_locked(
        &mut s,
        chrono::Utc::now(),
        crate::playback::player::ListenSessionEndReason::Stopped,
    ) {
        tracing::warn!("flush on shutdown failed: {err}");
    }
    StatusCode::OK
}

async fn ping_handler() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "name": "NOOR" }))
}

/// Shared loopback gate for the unauthenticated /api/setup/* endpoints. Keeps
/// the three handlers from drifting on what counts as "local-only".
fn require_loopback(addr: SocketAddr) -> Result<(), StatusCode> {
    if addr.ip().is_loopback() {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

fn require_public_loopback_request(
    addr: SocketAddr,
    headers: &HeaderMap,
) -> Result<(), StatusCode> {
    require_loopback(addr)?;
    require_trusted_browser_origin(headers)
}

fn require_trusted_browser_origin(headers: &HeaderMap) -> Result<(), StatusCode> {
    if let Some(origin) = headers.get(header::ORIGIN) {
        return if is_trusted_local_origin_value(origin) {
            Ok(())
        } else {
            Err(StatusCode::FORBIDDEN)
        };
    }

    if let Some(referer) = headers.get(header::REFERER) {
        let Ok(referer) = referer.to_str() else {
            return Err(StatusCode::FORBIDDEN);
        };
        let Some(origin) = origin_from_url(referer) else {
            return Err(StatusCode::FORBIDDEN);
        };
        return if is_trusted_local_origin_str(&origin) {
            Ok(())
        } else {
            Err(StatusCode::FORBIDDEN)
        };
    }

    Ok(())
}

fn is_trusted_local_origin_value(origin: &HeaderValue) -> bool {
    origin.to_str().is_ok_and(is_trusted_local_origin_str)
}

/// Local listen port for the server. A port embedded in `NOOR_ADDR` (the
/// power-user override) wins, then `NOOR_PORT`, falling back to 17600. Kept in
/// sync with `resolve_bind_addr` and the Tauri shell's `server_url` helper so
/// the listen port can be changed by env without recompiling.
pub fn noor_port() -> u16 {
    if let Ok(addr) = std::env::var("NOOR_ADDR")
        && let Some(p) = addr
            .trim()
            .rsplit(':')
            .next()
            .and_then(|s| s.parse::<u16>().ok())
    {
        return p;
    }
    std::env::var("NOOR_PORT")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(17600)
}

/// Vite dev-server port trusted for CORS in development. Honors `NOOR_DEV_PORT`,
/// default 17601.
fn noor_dev_port() -> u16 {
    std::env::var("NOOR_DEV_PORT")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(17601)
}

fn is_trusted_local_origin_str(origin: &str) -> bool {
    let trimmed = origin.trim().trim_end_matches('/');
    let Some(rest) = trimmed.strip_prefix("http://") else {
        return false;
    };
    // rsplit keeps the IPv6 "[::1]:port" host intact (brackets and all).
    let Some((host, port)) = rest.rsplit_once(':') else {
        return false;
    };
    if !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
        return false;
    }
    match port.parse::<u16>() {
        Ok(p) => p == noor_port() || p == noor_dev_port(),
        Err(_) => false,
    }
}

fn origin_from_url(raw: &str) -> Option<String> {
    let (scheme, rest) = raw.trim().split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority_end = rest
        .find(|ch| matches!(ch, '/' | '?' | '#'))
        .unwrap_or(rest.len());
    if authority_end == 0 {
        return None;
    }
    Some(format!("{scheme}://{}", &rest[..authority_end]))
}

/// Returns the server token ONLY for requests arriving from loopback (127.0.0.1 / ::1).
/// Lets the frontend auto-configure on the local machine without needing the terminal.
async fn setup_token_handler(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_public_loopback_request(addr, &headers)?;
    let token = state.read().await.server_token.clone();
    Ok(Json(json!({ "token": token })))
}

async fn onboarding_status_handler(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_public_loopback_request(addr, &headers)?;
    let complete = state
        .read()
        .await
        .db
        .with_conn(crate::db::queries::get_onboarding_complete)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "complete": complete })))
}

async fn onboarding_complete_handler(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_public_loopback_request(addr, &headers)?;
    state
        .read()
        .await
        .db
        .with_conn(crate::db::queries::set_onboarding_complete)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "complete": true })))
}

async fn require_token(
    State(state): State<SharedState>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let expected = {
        let s = state.read().await;
        s.server_token.clone()
    };

    // Authorization: Bearer <token>
    let header_token = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    // ?token=<token> — used by WebSocket upgrades (browsers can't set WS headers)
    let query_string = req.uri().query().unwrap_or("").to_owned();
    let query_token: Option<&str> = query_string.split('&').find_map(|part| {
        let mut it = part.splitn(2, '=');
        match (it.next(), it.next()) {
            (Some("token"), Some(v)) => Some(v),
            _ => None,
        }
    });

    let provided = header_token.or(query_token);

    if provided == Some(expected.as_str()) {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

async fn no_store_cache(req: Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tower::Service;

    fn loopback_addr() -> SocketAddr {
        "127.0.0.1:12345".parse().unwrap()
    }

    fn remote_addr() -> SocketAddr {
        "192.0.2.10:12345".parse().unwrap()
    }

    fn static_test_dir() -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "noor-static-service-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("index.html"), "<!doctype html><title>NOOR</title>").unwrap();
        fs::write(dir.join("favicon.ico"), "ico").unwrap();
        dir
    }

    #[tokio::test]
    async fn static_assets_service_serves_spa_routes_with_ok_status() {
        let dir = static_test_dir();
        let mut service = static_assets_service(&dir);
        let request = Request::builder()
            .uri("/remote")
            .body(Body::empty())
            .unwrap();

        let response = service.call(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn static_assets_service_serves_existing_assets() {
        let dir = static_test_dir();
        let mut service = static_assets_service(&dir);
        let request = Request::builder()
            .uri("/favicon.ico")
            .body(Body::empty())
            .unwrap();

        let response = service.call(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn public_loopback_request_allows_same_origin_app() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://127.0.0.1:17600"),
        );

        assert_eq!(
            require_public_loopback_request(loopback_addr(), &headers),
            Ok(())
        );
    }

    #[test]
    fn public_loopback_request_allows_headerless_sidecar_call() {
        let headers = HeaderMap::new();

        assert_eq!(
            require_public_loopback_request(loopback_addr(), &headers),
            Ok(())
        );
    }

    #[test]
    fn public_loopback_request_rejects_foreign_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.com"),
        );

        assert_eq!(
            require_public_loopback_request(loopback_addr(), &headers),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn public_loopback_request_rejects_foreign_referer() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::REFERER,
            HeaderValue::from_static("https://example.com/page"),
        );

        assert_eq!(
            require_public_loopback_request(loopback_addr(), &headers),
            Err(StatusCode::FORBIDDEN)
        );
    }

    #[test]
    fn public_loopback_request_still_requires_loopback_peer() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://127.0.0.1:17600"),
        );

        assert_eq!(
            require_public_loopback_request(remote_addr(), &headers),
            Err(StatusCode::FORBIDDEN)
        );
    }
}
