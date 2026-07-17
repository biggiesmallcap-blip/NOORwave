//! Editorial video sets for the /videos browse state.
//!
//! `GET /api/videos/discover` is stale-while-revalidate over the persisted
//! `video_sets` snapshots: a fresh bucket is served as-is; a miss serves the
//! most recent older snapshot immediately (marked `stale`) and kicks exactly
//! one background build for the new bucket. The page never blocks on the
//! TIDAL fan-out, and a fresh install / logged-out session degrades to an
//! empty `sets` array, which the frontend renders as today's search-first
//! page.

use crate::SharedState;
use crate::services::tidal::client::TidalClient;
use crate::services::video_sets::{self, DAILY_PICKS_SLUG, VideoSet};
use axum::{extract::State, response::Json};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

/// One build at a time, process-wide. A skipped kick is retried by the next
/// request after the running build finishes, so this never wedges.
static VIDEO_SET_BUILD_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

pub(super) async fn get_videos_discover(State(state): State<SharedState>) -> Json<Value> {
    let bucket = video_sets::daily_bucket_key(chrono::Local::now().date_naive());

    let fresh = {
        let s = state.read().await;
        s.db.with_conn(|conn| video_sets::load_set(conn, DAILY_PICKS_SLUG, &bucket))
            .unwrap_or_default()
    };
    if let Some(set) = fresh {
        return Json(json!({ "sets": [set_json(&set, false)], "building": false }));
    }

    let stale = {
        let s = state.read().await;
        s.db.with_conn(|conn| video_sets::load_latest_set(conn, DAILY_PICKS_SLUG))
            .unwrap_or_default()
    };
    let building = kick_background_build(&state, bucket);
    let sets: Vec<Value> = stale.iter().map(|s| set_json(s, true)).collect();
    Json(json!({ "sets": sets, "building": building }))
}

fn set_json(set: &VideoSet, stale: bool) -> Value {
    json!({
        "slug": set.slug,
        "bucket_key": set.bucket_key,
        "title": set.title,
        "blurb": set.blurb,
        "items": set.items,
        "stale": stale,
    })
}

/// Spawn the daily build unless one is already running. Returns whether a
/// build is (now or already) in flight.
fn kick_background_build(state: &SharedState, bucket: String) -> bool {
    if VIDEO_SET_BUILD_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return true;
    }
    let state = state.clone();
    tokio::spawn(async move {
        let result = build_daily_set(&state, &bucket).await;
        VIDEO_SET_BUILD_IN_FLIGHT.store(false, Ordering::SeqCst);
        match result {
            Ok(true) => tracing::info!("video set build: daily picks ready for {bucket}"),
            Ok(false) => {
                tracing::debug!("video set build: skipped for {bucket} (no anchors or session)")
            }
            Err(e) => tracing::warn!("video set build failed for {bucket}: {e}"),
        }
    });
    true
}

/// Build and persist the daily picks set for one bucket. `Ok(false)` means a
/// clean skip: no TIDAL session, no listen history to anchor on, or too few
/// usable videos to make a set worth showing.
async fn build_daily_set(state: &SharedState, bucket: &str) -> anyhow::Result<bool> {
    let (tokens, tidal_http_client, db) = {
        let s = state.read().await;
        (
            s.tidal_tokens.clone(),
            s.tidal_http_client.clone(),
            s.db.clone(),
        )
    };
    let tokens = match tokens {
        Some(t) => Some(t),
        None => super::load_persisted_tidal_tokens(state)
            .await
            .ok()
            .flatten(),
    };
    let Some(tokens) = tokens else {
        return Ok(false);
    };

    let seed = video_sets::build_seed(DAILY_PICKS_SLUG, bucket);
    let Some(inputs) = db.with_conn(|conn| video_sets::read_daily_build_inputs(conn, seed))? else {
        return Ok(false);
    };

    let client = TidalClient::with_http(
        tidal_http_client,
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );
    let videos_by_anchor = video_sets::fetch_anchor_videos(&client, inputs.anchors.clone()).await;

    let Some(set) = video_sets::assemble_daily_picks(bucket, seed, &inputs, &videos_by_anchor)
    else {
        return Ok(false);
    };
    db.with_conn(|conn| video_sets::store_set(conn, &set))?;
    Ok(true)
}
