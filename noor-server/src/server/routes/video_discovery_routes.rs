//! Editorial video sets for the /videos browse state.
//!
//! `GET /api/videos/discover` is stale-while-revalidate over the persisted
//! `video_sets` snapshots: whatever has been built is served immediately, and
//! anything missing for the current bucket is built in one background pass.
//! The page never blocks on the TIDAL fan-out, and a fresh install /
//! logged-out session degrades to an empty `sets` array, which the frontend
//! renders as the plain search-first page.
//!
//! Sets build sequentially inside that pass so a slow archetype cannot starve
//! the others, and each is persisted as soon as it is ready: the page fills in
//! shelf by shelf across a few client polls rather than all at once at the end.

use crate::SharedState;
use crate::services::tidal::client::TidalClient;
use crate::services::video_sets::{
    self, ALBUM_LOVE_SLUG, Archetype, DAILY_PICKS_SLUG, DJ_SETS_SLUG, ERA_SLUG, GENRE_SLUG_PREFIX,
    ONE_STEP_OUT_SLUG, RECENTLY_WATCHED_DAYS, SetPlan, VideoSet,
};
use axum::{extract::State, response::Json};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

/// One build pass at a time, process-wide. A skipped kick is retried by the
/// next request after the running pass finishes, so this never wedges.
static VIDEO_SET_BUILD_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

pub(super) async fn get_videos_discover(State(state): State<SharedState>) -> Json<Value> {
    let today = chrono::Local::now().date_naive();

    let (mut sets, stale_buckets) = {
        let s = state.read().await;
        let sets =
            s.db.with_conn(video_sets::load_latest_sets)
                .unwrap_or_default();
        // Cheap bucket check only: the real planner is heavy and runs inside
        // the background pass.
        let stale_buckets =
            s.db.with_conn(|conn| video_sets::needs_build(conn, today))
                .unwrap_or(false);
        (sets, stale_buckets)
    };

    sets.sort_by_key(|set| display_order(&set.slug));
    let building = if stale_buckets {
        kick_background_build(&state, today)
    } else {
        false
    };

    let payload: Vec<Value> = sets
        .iter()
        .map(|set| {
            json!({
                "slug": set.slug,
                "bucket_key": set.bucket_key,
                "title": set.title,
                "blurb": set.blurb,
                "items": set.items,
                "stale": is_stale(set, today),
            })
        })
        .collect();
    Json(json!({ "sets": payload, "building": building }))
}

/// Shelf order on the page: the daily mural leads, then the genre shelves,
/// then the taste-derived sets, with the long-form shelf last.
fn display_order(slug: &str) -> u8 {
    match slug {
        DAILY_PICKS_SLUG => 0,
        ALBUM_LOVE_SLUG => 2,
        ONE_STEP_OUT_SLUG => 3,
        ERA_SLUG => 4,
        DJ_SETS_SLUG => 5,
        s if s.starts_with(GENRE_SLUG_PREFIX) => 1,
        _ => 6,
    }
}

/// A snapshot is stale when it was built for an older bucket than the one its
/// slug's rhythm is currently in. Daily slugs turn over at midnight, weekly
/// ones on Monday.
fn is_stale(set: &VideoSet, today: chrono::NaiveDate) -> bool {
    let current = if set.slug == DAILY_PICKS_SLUG {
        video_sets::daily_bucket_key(today)
    } else {
        video_sets::weekly_bucket_key(today)
    };
    set.bucket_key != current
}

fn kick_background_build(state: &SharedState, today: chrono::NaiveDate) -> bool {
    if VIDEO_SET_BUILD_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return true;
    }
    let state = state.clone();
    tokio::spawn(async move {
        let result = build_missing_sets(&state, today).await;
        VIDEO_SET_BUILD_IN_FLIGHT.store(false, Ordering::SeqCst);
        match result {
            Ok(n) if n > 0 => tracing::info!("video set build: {n} set(s) ready"),
            Ok(_) => tracing::debug!("video set build: nothing to build"),
            Err(e) => tracing::warn!("video set build failed: {e}"),
        }
    });
    true
}

/// Build and persist every set missing for the current buckets. Returns how
/// many were written; a set that cannot reach the minimum size is skipped
/// silently and retried on the next bucket.
async fn build_missing_sets(
    state: &SharedState,
    today: chrono::NaiveDate,
) -> anyhow::Result<usize> {
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
        return Ok(0);
    };

    // Planning runs several aggregates over listen_history, which on a real
    // library takes long enough that holding the shared connection would stall
    // every other request. WAL lets this reader run beside them.
    let plan_db = db.clone();
    let (plans, known_artists, recently_watched) =
        tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let conn = plan_db.open_isolated()?;
            let plans = video_sets::plan_missing_sets(&conn, today)?;
            let known = video_sets::known_artist_tidal_ids(&conn)?;
            let recent = video_sets::recently_watched_video_ids(&conn, RECENTLY_WATCHED_DAYS)?;
            Ok((plans, known, recent))
        })
        .await??;
    if plans.is_empty() {
        db.with_conn(|conn| video_sets::mark_pass_complete(conn, today))?;
        return Ok(0);
    }

    let client = TidalClient::with_http(
        tidal_http_client,
        tokens.access_token.clone(),
        tokens.country_code.clone(),
    );

    let mut built = 0usize;
    for plan in plans {
        let groups = fetch_for_plan(&client, &plan, &known_artists).await;
        let Some(set) = video_sets::assemble_set_excluding(&plan, &groups, &recently_watched)
        else {
            tracing::debug!(
                "video set build: {} produced too few items, skipping",
                plan.slug
            );
            continue;
        };
        db.with_conn(|conn| video_sets::store_set(conn, &set))?;
        built += 1;
    }
    if built > 0 {
        db.with_conn(video_sets::prune_old_sets)?;
    }
    // Mark the pass done even when some shelves came up empty: a shelf with too
    // few candidates should wait for the next bucket, not re-run the whole pass
    // on every page load.
    db.with_conn(|conn| video_sets::mark_pass_complete(conn, today))?;
    Ok(built)
}

/// Candidate sourcing per archetype. Everything else about a set - scoring,
/// capping, copy - is shared.
async fn fetch_for_plan(
    client: &TidalClient,
    plan: &SetPlan,
    known_artists: &std::collections::HashSet<i64>,
) -> Vec<(video_sets::AnchorArtist, Vec<video_sets::VideoCandidate>)> {
    match plan.archetype {
        Archetype::DjSets => video_sets::fetch_long_form(client, &plan.queries).await,
        Archetype::OneStepOut => {
            let anchors =
                video_sets::expand_similar_anchors(client, &plan.anchors, known_artists).await;
            if anchors.is_empty() {
                return Vec::new();
            }
            video_sets::fetch_anchor_videos(client, anchors).await
        }
        _ => video_sets::fetch_anchor_videos(client, plan.anchors.clone()).await,
    }
}

/// A video the dock started playing. Recorded so the set builder can hold it out
/// of the next few rotations (see `recently_watched_video_ids`).
#[derive(Deserialize)]
pub(super) struct RecordVideoPlay {
    pub tidal_video_id: i64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub artist_tidal_id: Option<i64>,
    #[serde(default)]
    pub artist_name: Option<String>,
}

/// `POST /api/videos/history`. Fire-and-forget from the player; a failed write
/// only costs a repeat pick, so it never surfaces an error to the client.
pub(super) async fn post_videos_history(
    State(state): State<SharedState>,
    Json(body): Json<RecordVideoPlay>,
) -> Json<Value> {
    let s = state.read().await;
    let result = s.db.with_conn(|conn| -> anyhow::Result<()> {
        conn.execute(
            "INSERT INTO video_history (tidal_video_id, title, artist_tidal_id, artist_name) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                body.tidal_video_id,
                body.title,
                body.artist_tidal_id,
                body.artist_name,
            ],
        )?;
        Ok(())
    });
    if let Err(e) = result {
        tracing::warn!(
            target = "noor.videos",
            event = "history_write_failed",
            "video history write failed: {e}"
        );
    }
    Json(json!({ "ok": true }))
}
