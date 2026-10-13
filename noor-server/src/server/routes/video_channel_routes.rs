//! On Air: numbered channels on a daily clock, the guide, tune-in at the
//! scheduled offset, refills, and the explore knob.
//!
//! Scaffold from Atlas and Air; the air worker owns this file. Every
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

/// GET /api/videos/channels
pub(super) async fn get_video_channels(
    State(_state): State<SharedState>,
) -> (StatusCode, Json<Value>) {
    not_yet()
}

/// GET /api/videos/channels/{id}/guide?from_s&to_s
pub(super) async fn get_video_channel_guide(
    State(_state): State<SharedState>,
) -> (StatusCode, Json<Value>) {
    not_yet()
}

/// POST /api/videos/channels/{id}/tune
pub(super) async fn post_video_channel_tune(
    State(_state): State<SharedState>,
) -> (StatusCode, Json<Value>) {
    not_yet()
}

/// POST /api/videos/channels/{id}/next
pub(super) async fn post_video_channel_next(
    State(_state): State<SharedState>,
) -> (StatusCode, Json<Value>) {
    not_yet()
}

/// GET /api/videos/explore
pub(super) async fn get_video_explore(
    State(_state): State<SharedState>,
) -> (StatusCode, Json<Value>) {
    not_yet()
}

/// PUT /api/videos/explore
pub(super) async fn put_video_explore(
    State(_state): State<SharedState>,
) -> (StatusCode, Json<Value>) {
    not_yet()
}
