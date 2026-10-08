//! Settings > Library > Artwork cache: the size cap and how much is used.

use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::SharedState;
use crate::services::artwork_cache;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ArtworkCacheSettings {
    max_mb: u64,
}

fn body(max_mb: u64) -> Json<Value> {
    Json(json!({
        "max_mb": max_mb,
        "used_bytes": artwork_cache::used_bytes(),
        "options_mb": artwork_cache::ALLOWED_MAX_MB,
    }))
}

/// `GET /api/artwork-cache`.
pub(super) async fn get_artwork_cache(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let db = { state.read().await.db.clone() };
    let max_mb = db
        .with_conn(artwork_cache::load_max_mb)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(body(max_mb))
}

/// `PUT /api/artwork-cache`. One of the offered sizes; 0 turns the cache off
/// and empties it. A smaller cap trims right away.
pub(super) async fn put_artwork_cache(
    State(state): State<SharedState>,
    Json(settings): Json<ArtworkCacheSettings>,
) -> Result<Json<Value>, StatusCode> {
    if !artwork_cache::ALLOWED_MAX_MB.contains(&settings.max_mb) {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let db = { state.read().await.db.clone() };
    db.with_conn(|conn| artwork_cache::save_max_mb(conn, settings.max_mb))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    artwork_cache::apply_max_mb(settings.max_mb);
    Ok(body(settings.max_mb))
}
