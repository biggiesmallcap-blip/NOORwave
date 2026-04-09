pub mod routes;
pub mod ws;

use crate::SharedState;
use anyhow::Result;
use axum::Router;
use tower_http::cors::{Any, CorsLayer};

pub async fn start(state: SharedState, addr: &str) -> Result<()> {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .merge(routes::api_routes(state.clone()))
        .merge(ws::ws_routes(state))
        .layer(cors);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
