//! Catalog intelligence reads: census, the Archive lens query, a song's
//! versions, a concert's setlist, and a manual rebuild trigger.
//!
//! Scaffold from Atlas and Air; the catalog worker owns this file. Every
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

/// GET /api/videos/census
pub(super) async fn get_video_census(
    State(_state): State<SharedState>,
) -> (StatusCode, Json<Value>) {
    not_yet()
}

/// GET /api/videos/archive?scene&cut&era_from&era_to&depth&length&source&sort&q&offset&limit
pub(super) async fn get_video_archive(
    State(_state): State<SharedState>,
) -> (StatusCode, Json<Value>) {
    not_yet()
}

/// GET /api/videos/songs/{key}/versions
pub(super) async fn get_video_song_versions(
    State(_state): State<SharedState>,
) -> (StatusCode, Json<Value>) {
    not_yet()
}

/// GET /api/videos/concerts/{id}
pub(super) async fn get_video_concert(
    State(_state): State<SharedState>,
) -> (StatusCode, Json<Value>) {
    not_yet()
}

/// POST /api/videos/intel/rebuild
pub(super) async fn post_video_intel_rebuild(
    State(_state): State<SharedState>,
) -> (StatusCode, Json<Value>) {
    not_yet()
}
