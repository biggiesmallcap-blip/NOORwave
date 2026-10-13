//! The Atlas: the whole map in one columnar payload, lazy names, artist
//! and scene cards, probes into the crawler, and routes between artists.
//!
//! Scaffold from Atlas and Air; the atlas worker owns this file. Every
//! handler answers 501 until implemented.
use axum::{extract::State, http::StatusCode, response::Json};
use serde_json::{Value, json};

use crate::SharedState;

fn not_yet() -> (StatusCode, Json<Value>) {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({ "error": "not implemented" })),
    )
}

/// GET /api/videos/atlas
pub(super) async fn get_video_atlas(
    State(_state): State<SharedState>,
) -> (StatusCode, Json<Value>) {
    not_yet()
}

/// GET /api/videos/atlas/names
pub(super) async fn get_video_atlas_names(
    State(_state): State<SharedState>,
) -> (StatusCode, Json<Value>) {
    not_yet()
}

/// GET /api/videos/atlas/artists/{id}
pub(super) async fn get_video_atlas_artist(
    State(_state): State<SharedState>,
) -> (StatusCode, Json<Value>) {
    not_yet()
}

/// GET /api/videos/atlas/scenes/{id}
pub(super) async fn get_video_atlas_scene(
    State(_state): State<SharedState>,
) -> (StatusCode, Json<Value>) {
    not_yet()
}

/// POST /api/videos/atlas/probe
pub(super) async fn post_video_atlas_probe(
    State(_state): State<SharedState>,
) -> (StatusCode, Json<Value>) {
    not_yet()
}

/// GET /api/videos/atlas/route?from&to
pub(super) async fn get_video_atlas_route(
    State(_state): State<SharedState>,
) -> (StatusCode, Json<Value>) {
    not_yet()
}
