pub mod routes;
pub mod ws;

use crate::SharedState;
use anyhow::Result;
use axum::{
    Router,
    extract::{ConnectInfo, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{Json, Response},
    routing::get,
};
use serde_json::json;
use std::net::SocketAddr;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

pub async fn start(state: SharedState, addr: &str) -> Result<()> {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Resolve www/ before building the router so the fallback can go on the
    // public router (no auth). If it were on the outer merged router, axum's
    // Router::layer would run require_token even for unmatched routes,
    // causing static file requests to return 401.
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let www_dir = exe_dir
        .as_ref()
        .and_then(|d| {
            let p = d.join("www");
            if p.is_dir() { Some(p) } else { None }
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

    // Public — no auth required. Static file serving lives here so it is
    // never touched by the require_token middleware.
    let public_base = Router::new()
        .route("/api/ping", get(ping_handler))
        .route("/api/setup/token", get(setup_token_handler))
        .with_state(state.clone());

    let public = match www_dir {
        Some(www) => {
            let index_html = www.join("index.html");
            public_base.fallback_service(
                ServeDir::new(&www).not_found_service(ServeFile::new(index_html)),
            )
        }
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

    let app = public.merge(protected).layer(cors);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await?;
    Ok(())
}

async fn ping_handler() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "name": "NOOR" }))
}

/// Returns the server token ONLY for requests arriving from loopback (127.0.0.1 / ::1).
/// Lets the frontend auto-configure on the local machine without needing the terminal.
async fn setup_token_handler(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if !addr.ip().is_loopback() {
        return Err(StatusCode::FORBIDDEN);
    }
    let token = state.read().await.server_token.clone();
    Ok(Json(json!({ "token": token })))
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
    let query_token: Option<&str> = query_string
        .split('&')
        .find_map(|part| {
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
