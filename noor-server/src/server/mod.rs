pub mod radio_pipeline;
pub mod remote;
pub mod routes;
pub mod ws;

use crate::SharedState;
use anyhow::Result;
use axum::{
    Extension, Router,
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
};
use serde_json::json;
use std::net::SocketAddr;
use tokio::sync::watch;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

pub async fn start(
    state: SharedState,
    addr: &str,
    control: remote::HostControl,
    configured_host_mode: bool,
) -> Result<()> {
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

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let actual_addr = listener.local_addr()?;
    let remote = state.read().await.remote.clone();
    let remote_assets_available = has_remote_assets(www_dir.as_deref());
    remote
        .set_bound_listener(
            actual_addr,
            control,
            configured_host_mode,
            remote_assets_available,
        )
        .await;
    // One observed shutdown state starts HTTP draining and discovery withdrawal
    // together. A watch channel retains the requested state even if one task is
    // still initializing when /api/shutdown arrives.
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let discovery = remote::discovery::DiscoveryHandle::start(
        remote.clone(),
        actual_addr,
        remote_assets_available,
        shutdown_rx.clone(),
    )
    .await;

    let app = build_router(state, remote.clone(), www_dir, shutdown_tx.clone());
    let mut http_shutdown = shutdown_rx;
    let signal_shutdown = shutdown_tx;

    let serve_result = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        // ctrl_c arm only fires when running standalone (`cargo run -p
        // noor-server`); in the bundled Tauri build the child has no
        // controlling terminal, so the registered handler never fires —
        // shutdown comes via /api/shutdown -> retained watch state.
        tokio::select! {
            _ = http_shutdown.changed() => {},
            _ = tokio::signal::ctrl_c() => {},
        }
        let _ = signal_shutdown.send(true);
    })
    .await;
    if let Some(discovery) = discovery {
        discovery.shutdown().await;
    } else {
        remote.invalidate_ticket().await;
    }
    serve_result?;
    Ok(())
}

fn build_router(
    state: SharedState,
    remote: remote::RemoteService,
    www_dir: Option<std::path::PathBuf>,
    shutdown_tx: watch::Sender<bool>,
) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin, _| {
            is_trusted_local_origin_value(origin)
        }))
        .allow_methods(Any)
        .allow_headers(Any);
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
        .merge(remote::public_routes(remote.clone()))
        .layer(Extension(shutdown_tx.clone()));

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
        .merge(ws::ws_routes(state.clone(), shutdown_tx.subscribe()))
        .merge(remote::management_routes(remote))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_token,
        ));

    // Compress responses (gzip/brotli, negotiated via Accept-Encoding). Large JSON
    // payloads - library lists, the ~100KB genre galaxy snapshot, recommendations -
    // shrink 40-60% on the wire. tower-http's default predicate skips already-
    // compressed content types (images, etc.), so artwork/audio aren't recompressed.
    public
        .merge(protected)
        .layer(cors)
        .layer(CompressionLayer::new())
}

fn static_assets_service(www: &std::path::Path) -> ServeDir<ServeFile> {
    ServeDir::new(www).fallback(ServeFile::new(www.join("index.html")))
}

fn has_remote_assets(www: Option<&std::path::Path>) -> bool {
    www.is_some_and(|directory| directory.join("index.html").is_file())
}

async fn shutdown_handler(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Extension(shutdown_tx): Extension<watch::Sender<bool>>,
    headers: HeaderMap,
) -> StatusCode {
    if require_public_loopback_request(addr, &headers).is_err() {
        return StatusCode::FORBIDDEN;
    }
    // Signal axum's graceful shutdown FIRST so a stuck write lock can't keep
    // the server alive — the listener stops accepting new connections while
    // we attempt the flush. If the lock is contended, the Tauri sidecar's
    // 1s POST timeout drops us and falls through to child.kill().
    let _ = shutdown_tx.send(true);
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
    let ip = match addr.ip() {
        std::net::IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(std::net::IpAddr::V4)
            .unwrap_or(std::net::IpAddr::V6(ip)),
        other => other,
    };
    if ip.is_loopback() {
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
) -> Result<Response, StatusCode> {
    require_public_loopback_request(addr, &headers)?;
    let remote = state.read().await.remote.clone();
    let token = remote.shared_pin().await;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({ "token": token })),
    )
        .into_response())
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
    mut req: Request,
    next: Next,
) -> Result<Response, Response> {
    let is_remote_management = req.uri().path().starts_with("/api/server/remote");
    let remote = {
        let s = state.read().await;
        s.remote.clone()
    };

    // Authorization: Bearer <token>
    let header_token = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    // ?token=<token> — used by WebSocket upgrades (browsers can't set WS headers)
    let query_string = req.uri().query().unwrap_or("").to_owned();
    let query_tokens: Vec<&str> = query_string
        .split('&')
        .filter_map(|part| {
            let mut it = part.splitn(2, '=');
            match (it.next(), it.next()) {
                (Some("token"), Some(value)) => Some(value),
                _ => None,
            }
        })
        .collect();
    let query_token = query_tokens.first().copied();

    // Keep the historical PIN header/query precedence for compatibility, but
    // fail closed whenever a new per-device credential is supplied through
    // more than one authority channel or as a duplicate query parameter.
    let includes_device_token = header_token
        .into_iter()
        .chain(query_tokens.iter().copied())
        .any(|token| token.starts_with("nrp_"));
    let ambiguous = includes_device_token
        && (query_tokens.len() > 1 || header_token.is_some() && query_token.is_some());
    if ambiguous {
        let mut response = if is_remote_management {
            remote::remote_error(
                StatusCode::UNAUTHORIZED,
                "AUTHENTICATION_REQUIRED",
                "Authentication required.",
            )
        } else {
            StatusCode::UNAUTHORIZED.into_response()
        };
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return Err(response);
    }

    let provided = header_token.or(query_token);

    let source_ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip())
        .unwrap_or_else(|| "0.0.0.0".parse().expect("static IP"));
    let principal = match remote.authenticate(provided, source_ip).await {
        Ok(principal) => principal,
        Err(remote::AuthError::Invalid) => {
            return Err(if is_remote_management {
                remote::remote_error(
                    StatusCode::UNAUTHORIZED,
                    "AUTHENTICATION_REQUIRED",
                    "Authentication required.",
                )
            } else {
                StatusCode::UNAUTHORIZED.into_response()
            });
        }
        Err(remote::AuthError::RateLimited { retry_after }) => {
            if is_remote_management {
                return Err(remote::rate_error(retry_after));
            }
            let mut response = StatusCode::TOO_MANY_REQUESTS.into_response();
            response.headers_mut().insert(
                header::RETRY_AFTER,
                retry_after.to_string().parse().expect("valid retry header"),
            );
            return Err(response);
        }
    };

    let path = req.uri().path();
    if path.starts_with("/api/server/") && path != "/api/server/info" {
        let peer = req
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| *addr);
        let allowed = matches!(principal, remote::Principal::SharedPin { .. })
            && peer.is_some_and(|addr| require_loopback(addr).is_ok())
            && require_trusted_browser_origin(req.headers()).is_ok();
        if !allowed {
            return Err(if is_remote_management {
                remote::remote_error(StatusCode::FORBIDDEN, "FORBIDDEN", "Forbidden.")
            } else {
                StatusCode::FORBIDDEN.into_response()
            });
        }
    }
    req.extensions_mut().insert(principal);
    Ok(next.run(req).await)
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
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tower::Service;

    async fn assembled_app(
        pin: &str,
        paired_token: Option<&str>,
        www_dir: Option<std::path::PathBuf>,
    ) -> (Router, SharedState) {
        let (app, state, _) = assembled_app_with_shutdown(pin, paired_token, www_dir).await;
        (app, state)
    }

    async fn assembled_app_with_shutdown(
        pin: &str,
        paired_token: Option<&str>,
        www_dir: Option<std::path::PathBuf>,
    ) -> (Router, SharedState, watch::Sender<bool>) {
        let db = routes::tests::fresh_migrated_db();
        if let Some(token) = paired_token {
            crate::db::remote::create_device(
                &db,
                &crate::db::remote::RemoteDeviceRow {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: "Paired phone".into(),
                    token_hash: Sha256::digest(token.as_bytes()).into(),
                    paired_at: chrono::Utc::now(),
                    last_seen_at: None,
                },
            )
            .unwrap();
        }
        let remote = remote::RemoteService::new(db.clone(), pin.to_string()).unwrap();
        let mut app_state = routes::tests::fresh_test_state(db);
        app_state.server_token = pin.to_string();
        app_state.remote = remote.clone();
        let state = Arc::new(tokio::sync::RwLock::new(app_state));
        let (shutdown_tx, _) = watch::channel(false);
        let app = build_router(state.clone(), remote, www_dir, shutdown_tx.clone());
        (app, state, shutdown_tx)
    }

    fn authenticated_request(
        path: &str,
        token: &str,
        peer: SocketAddr,
        origin: Option<&str>,
    ) -> Request<Body> {
        let mut request = Request::builder()
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        if let Some(origin) = origin {
            request = request.header(header::ORIGIN, origin);
        }
        let mut request = request.body(Body::empty()).unwrap();
        request.extensions_mut().insert(ConnectInfo(peer));
        request
    }

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
    fn remote_asset_readiness_requires_index_html() {
        let dir = std::env::temp_dir().join(format!(
            "noorwave-incomplete-www-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        assert!(!has_remote_assets(Some(&dir)));
        fs::write(dir.join("index.html"), "<main>remote</main>").unwrap();
        assert!(has_remote_assets(Some(&dir)));
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn assembled_router_keeps_static_remote_public() {
        let dir = static_test_dir();
        let (mut app, _) = assembled_app("123456", None, Some(dir.clone())).await;
        let mut request = Request::builder()
            .uri("/remote/deep-link")
            .body(Body::empty())
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(remote_addr()));

        let response = app.call(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn assembled_router_enforces_loopback_origin_and_shared_pin_for_management() {
        let paired_token = "nrp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let (app, _) = assembled_app("123456", Some(paired_token), None).await;

        let cases = [
            (
                "123456",
                remote_addr(),
                Some("http://127.0.0.1:17600"),
                StatusCode::FORBIDDEN,
            ),
            (
                "123456",
                loopback_addr(),
                Some("https://example.com"),
                StatusCode::FORBIDDEN,
            ),
            (
                paired_token,
                loopback_addr(),
                Some("http://127.0.0.1:17600"),
                StatusCode::FORBIDDEN,
            ),
            (
                "123456",
                loopback_addr(),
                Some("http://127.0.0.1:17600"),
                StatusCode::OK,
            ),
        ];
        for (token, peer, origin, expected) in cases {
            let mut service = app.clone();
            let response = service
                .call(authenticated_request(
                    "/api/server/token",
                    token,
                    peer,
                    origin,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), expected, "token={token} peer={peer}");
        }
    }

    #[tokio::test]
    async fn server_info_reports_actual_listener_not_saved_preference() {
        let (mut app, state) = assembled_app("123456", None, None).await;
        state
            .read()
            .await
            .remote
            .set_bound_listener(
                "0.0.0.0:32123".parse().unwrap(),
                remote::HostControl::Standalone,
                false,
                true,
            )
            .await;
        let response = app
            .call(authenticated_request(
                "/api/server/info",
                "123456",
                remote_addr(),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["host_mode"], false);
        assert_eq!(value["effective_host_mode"], true);
        assert_eq!(value["restart_required"], true);
        assert_eq!(value["bind_address"], "0.0.0.0:32123");
    }

    #[tokio::test]
    async fn reset_closes_an_authenticated_websocket_with_terminal_auth_code() {
        use futures::StreamExt;
        use tokio_tungstenite::tungstenite::Message as ClientMessage;

        let paired_token = "nrp_BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
        let (app, state) = assembled_app("123456", Some(paired_token), None).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        let (mut socket, _) =
            tokio_tungstenite::connect_async(format!("ws://{address}/ws?token={paired_token}"))
                .await
                .unwrap();
        let connected = socket.next().await.unwrap().unwrap();
        assert!(matches!(connected, ClientMessage::Text(_)));

        let remote = state.read().await.remote.clone();
        remote::reset_all(&remote).await.unwrap();
        let close = tokio::time::timeout(std::time::Duration::from_secs(1), socket.next())
            .await
            .expect("socket close deadline")
            .expect("close frame")
            .expect("valid close frame");
        match close {
            ClientMessage::Close(Some(frame)) => assert_eq!(u16::from(frame.code), 4001),
            other => panic!("expected authentication close frame, got {other:?}"),
        }
        server.abort();
    }

    #[tokio::test]
    async fn graceful_shutdown_closes_a_live_websocket_with_going_away_code() {
        use futures::StreamExt;
        use tokio_tungstenite::tungstenite::Message as ClientMessage;

        let (app, _, shutdown) = assembled_app_with_shutdown("123456", None, None).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut server_shutdown = shutdown.subscribe();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                while !*server_shutdown.borrow() {
                    if server_shutdown.changed().await.is_err() {
                        return;
                    }
                }
            })
            .await
            .unwrap();
        });
        let (mut socket, _) =
            tokio_tungstenite::connect_async(format!("ws://{address}/ws?token=123456"))
                .await
                .unwrap();
        assert!(matches!(
            socket.next().await.unwrap().unwrap(),
            ClientMessage::Text(_)
        ));

        let shutdown_response = reqwest::Client::new()
            .post(format!("http://{address}/api/shutdown"))
            .header(header::ORIGIN, "http://127.0.0.1:17600")
            .send()
            .await
            .unwrap();
        assert_eq!(shutdown_response.status(), StatusCode::OK);
        let close = tokio::time::timeout(std::time::Duration::from_secs(1), socket.next())
            .await
            .expect("socket close deadline")
            .expect("close frame")
            .expect("valid close frame");
        match close {
            ClientMessage::Close(Some(frame)) => assert_eq!(u16::from(frame.code), 1001),
            other => panic!("expected going-away close frame, got {other:?}"),
        }
        drop(socket);
        tokio::time::timeout(std::time::Duration::from_secs(1), server)
            .await
            .expect("graceful server shutdown deadline")
            .unwrap();
    }

    #[tokio::test]
    async fn assembled_reset_endpoint_rotates_authority_for_subsequent_requests() {
        let (mut app, _) = assembled_app("123456", None, None).await;
        let mut reset = Request::builder()
            .method("POST")
            .uri("/api/server/token/regenerate")
            .header(header::AUTHORIZATION, "Bearer 123456")
            .header(header::ORIGIN, "http://127.0.0.1:17600")
            .body(Body::empty())
            .unwrap();
        reset.extensions_mut().insert(ConnectInfo(loopback_addr()));
        let response = app.call(reset).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let new_pin = value["token"].as_str().unwrap();
        assert_ne!(new_pin, "123456");

        let old = app
            .call(authenticated_request(
                "/api/server/token",
                "123456",
                loopback_addr(),
                Some("http://127.0.0.1:17600"),
            ))
            .await
            .unwrap();
        assert_eq!(old.status(), StatusCode::UNAUTHORIZED);
        let new = app
            .call(authenticated_request(
                "/api/server/token",
                new_pin,
                loopback_addr(),
                Some("http://127.0.0.1:17600"),
            ))
            .await
            .unwrap();
        assert_eq!(new.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn setup_token_reads_the_authoritative_rotated_pin() {
        let (mut app, state) = assembled_app("123456", None, None).await;
        let remote = state.read().await.remote.clone();
        let (new_pin, _) = remote::reset_all(&remote).await.unwrap();

        // Deliberately leave AppState::server_token stale to model a reset
        // racing with a reader waiting on the broad application-state lock.
        assert_eq!(state.read().await.server_token, "123456");
        let mut request = Request::builder()
            .uri("/api/setup/token")
            .header(header::ORIGIN, "http://127.0.0.1:17600")
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(loopback_addr()));
        let response = app.call(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["token"], new_pin);
    }

    #[tokio::test]
    async fn paired_credentials_reject_ambiguous_header_or_query_authority() {
        let paired_token = "nrp_CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC";
        let (mut app, _) = assembled_app("123456", Some(paired_token), None).await;

        let mut mixed = Request::builder()
            .uri(format!("/api/server/info?token={paired_token}"))
            .header(header::AUTHORIZATION, "Bearer 123456")
            .body(Body::empty())
            .unwrap();
        mixed.extensions_mut().insert(ConnectInfo(remote_addr()));
        assert_eq!(
            app.call(mixed).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );

        let mut duplicated = Request::builder()
            .uri(format!(
                "/api/server/info?token={paired_token}&token={paired_token}"
            ))
            .body(Body::empty())
            .unwrap();
        duplicated
            .extensions_mut()
            .insert(ConnectInfo(remote_addr()));
        assert_eq!(
            app.call(duplicated).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn paired_credentials_authenticate_by_header_and_existing_query_path() {
        let paired_token = "nrp_DDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD";
        let (mut app, _) = assembled_app("123456", Some(paired_token), None).await;

        let header_response = app
            .call(authenticated_request(
                "/api/server/info",
                paired_token,
                remote_addr(),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(header_response.status(), StatusCode::OK);

        let mut query_request = Request::builder()
            .uri(format!("/api/server/info?token={paired_token}"))
            .body(Body::empty())
            .unwrap();
        query_request
            .extensions_mut()
            .insert(ConnectInfo(remote_addr()));
        assert_eq!(
            app.call(query_request).await.unwrap().status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn new_management_auth_errors_are_json_and_no_store() {
        let (mut app, _) = assembled_app("123456", None, None).await;
        let mut request = Request::builder()
            .uri("/api/server/remote")
            .header(header::ORIGIN, "http://127.0.0.1:17600")
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(loopback_addr()));
        let response = app.call(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"],
            "AUTHENTICATION_REQUIRED"
        );
    }

    #[tokio::test]
    async fn malformed_pairing_json_uses_the_remote_error_contract() {
        let (mut app, state) = assembled_app("123456", None, None).await;
        state
            .read()
            .await
            .remote
            .set_bound_listener(
                "192.168.1.10:17600".parse().unwrap(),
                remote::HostControl::Standalone,
                true,
                true,
            )
            .await;
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/remote/pair")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::HOST, "192.168.1.10:17600")
            .body(Body::from("{\"ticket\":"))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(remote_addr()));
        let response = app.call(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"],
            "INVALID_REQUEST"
        );
    }

    #[tokio::test]
    async fn pairing_body_is_capped_before_json_processing() {
        let (mut app, state) = assembled_app("123456", None, None).await;
        state
            .read()
            .await
            .remote
            .set_bound_listener(
                "192.168.1.10:17600".parse().unwrap(),
                remote::HostControl::Standalone,
                true,
                true,
            )
            .await;
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/remote/pair")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::HOST, "192.168.1.10:17600")
            .body(Body::from(vec![b'x'; 2049]))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(remote_addr()));
        let response = app.call(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn assembled_invalid_auth_throttles_spoofed_forwarding_headers_but_not_valid_auth() {
        let (mut app, _) = assembled_app("123456", None, None).await;
        for attempt in 0..8 {
            let mut request =
                authenticated_request("/api/server/info", "wrong", remote_addr(), None);
            request.headers_mut().insert(
                "x-forwarded-for",
                format!("198.51.100.{attempt}").parse().unwrap(),
            );
            assert_eq!(
                app.call(request).await.unwrap().status(),
                StatusCode::UNAUTHORIZED
            );
        }
        let ninth = app
            .call(authenticated_request(
                "/api/server/info",
                "wrong",
                remote_addr(),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(ninth.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(ninth.headers().contains_key(header::RETRY_AFTER));

        let valid = app
            .call(authenticated_request(
                "/api/server/info",
                "123456",
                remote_addr(),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(valid.status(), StatusCode::OK);
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
