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
use crate::db::Database;
use crate::services::library_videos;
use crate::services::tidal::client::TidalClient;
use crate::services::video_discovery::crawler::{self, Urgent};
use crate::services::video_discovery::{graph, names, roots};
use crate::services::video_radio;
use crate::services::video_sets::{
    self, ALBUM_LOVE_SLUG, Archetype, DAILY_PICKS_SLUG, DJ_SETS_SLUG, ERA_SLUG, GENRE_SLUG_PREFIX,
    ONE_STEP_OUT_SLUG, RECENTLY_WATCHED_DAYS, SetPlan, VideoSet,
};
use axum::{
    extract::{Path, State},
    response::Json,
};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

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
    let (plans, known_artists, recently_watched, existing_sets) =
        tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let conn = plan_db.open_isolated()?;
            let plans = video_sets::plan_missing_sets(&conn, today)?;
            let known = video_sets::known_artist_tidal_ids(&conn)?;
            let recent = video_sets::recently_watched_video_ids(&conn, RECENTLY_WATCHED_DAYS)?;
            let existing = video_sets::load_latest_sets(&conn)?;
            Ok((plans, known, recent, existing))
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

    let mut shown_video_ids: HashSet<i64> = HashSet::new();
    let mut artist_exposure: HashMap<String, usize> = HashMap::new();
    for set in existing_sets.iter().filter(|set| !is_stale(set, today)) {
        record_set_exposure(set, &mut shown_video_ids, &mut artist_exposure);
    }
    let mut built = 0usize;
    for plan in plans {
        let groups = fetch_for_plan(&client, &plan, &known_artists).await;
        db.with_conn(|conn| video_radio::cache_groups(conn, &groups))?;
        let Some(set) = video_sets::assemble_set_with_context(
            &plan,
            &groups,
            &recently_watched,
            &shown_video_ids,
            &artist_exposure,
        ) else {
            tracing::debug!(
                "video set build: {} produced too few items, skipping",
                plan.slug
            );
            continue;
        };
        db.with_conn(|conn| video_sets::store_set(conn, &set))?;
        record_set_exposure(&set, &mut shown_video_ids, &mut artist_exposure);
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

/// A refill request carries the active session's queue window. The server also
/// applies recent watch history, so navigating away does not reset freshness.
#[derive(Deserialize)]
pub(super) struct VideoRadioRequest {
    pub seed_artist_id: Option<i64>,
    pub seed_artist_name: Option<String>,
    #[serde(default)]
    pub seed_video_id: Option<i64>,
    #[serde(default)]
    pub exclude_video_ids: Vec<i64>,
    #[serde(default)]
    pub recent_video_ids: Vec<i64>,
    #[serde(default)]
    pub recent_songs: Vec<RecentVideoSong>,
    #[serde(default)]
    pub recent_artist_ids: Vec<i64>,
}

#[derive(Deserialize)]
pub(super) struct RecentVideoSong {
    pub artist_id: Option<i64>,
    pub artist_name: Option<String>,
    pub title: String,
}

/// How long a cold station waits for its first crawl before answering.
const COLD_START_WAIT: Duration = if cfg!(test) {
    Duration::from_millis(50)
} else {
    Duration::from_secs(5)
};
const RELATED_HEALTHY: usize = 6;

fn video_radio_payload(
    items: &[video_sets::VideoSetItem],
    familiar: &HashSet<i64>,
    building: bool,
) -> Value {
    let unfamiliar_video_ids: Vec<i64> = items
        .iter()
        .filter(|item| item.artist_id.is_some_and(|id| !familiar.contains(&id)))
        .map(|item| item.tidal_id)
        .collect();
    json!({ "items": items, "unfamiliar_video_ids": unfamiliar_video_ids, "building": building })
}

fn lookup_seed(
    db: &Database,
    artist_id: Option<i64>,
    artist_name: Option<&str>,
    video_id: Option<i64>,
) -> Option<i64> {
    if let Some(id) = artist_id.filter(|id| *id > 0) {
        return Some(id);
    }
    if let Some(name) = artist_name.filter(|name| !name.trim().is_empty() && name.len() <= 120)
        && let Ok(Some(id)) = db.with_conn(|conn| names::find_local(conn, name))
    {
        return Some(id);
    }
    let video_id = video_id.filter(|id| *id > 0)?;
    db.with_conn(|conn| {
        Ok(conn.query_row(
            "SELECT COALESCE(
                (SELECT artist_tidal_id FROM video_catalog WHERE tidal_video_id = ?1 AND artist_tidal_id > 0),
                (SELECT a.tidal_id FROM library_videos lv
                   JOIN tracks t ON t.id = lv.track_id
                   JOIN artists a ON a.id = t.artist_id
                  WHERE lv.tidal_video_id = ?1 AND a.tidal_id > 0 LIMIT 1))",
            [video_id],
            |row| row.get::<_, Option<i64>>(0),
        )?)
    })
    .ok()
    .flatten()
}

async fn resolve_seed(
    db: &Database,
    artist_id: Option<i64>,
    artist_name: Option<&str>,
    video_id: Option<i64>,
) -> Option<i64> {
    if let Some(seed) = lookup_seed(db, artist_id, artist_name, video_id) {
        return Some(seed);
    }
    let video_id = video_id.filter(|id| *id > 0)?;
    crawler::request(Urgent::Video(video_id));
    crawler::wait_until_built(Urgent::Video(video_id), COLD_START_WAIT).await;
    lookup_seed(db, None, None, Some(video_id))
}

pub(super) async fn post_videos_radio_next(
    State(state): State<SharedState>,
    Json(body): Json<VideoRadioRequest>,
) -> Json<Value> {
    let db = { state.read().await.db.clone() };
    let seed_id = resolve_seed(
        &db,
        body.seed_artist_id,
        body.seed_artist_name.as_deref(),
        body.seed_video_id,
    )
    .await;
    if let Some(seed) = seed_id {
        roots::touch_station(seed);
    }
    let excluded: HashSet<i64> = body
        .exclude_video_ids
        .into_iter()
        .take(256)
        .filter(|id| *id > 0)
        .collect();
    let recent_seen: HashSet<i64> = body
        .recent_video_ids
        .into_iter()
        .take(128)
        .filter(|id| *id > 0)
        .collect();
    let recent_song_keys: HashSet<String> = body
        .recent_songs
        .iter()
        .take(128)
        .filter(|song| song.title.len() <= 500)
        .map(|song| {
            video_radio::video_song_key(song.artist_id, song.artist_name.as_deref(), &song.title)
        })
        .collect();
    let recent_artists: Vec<i64> = body.recent_artist_ids.into_iter().take(24).collect();
    let (familiar, anchors) = match db.with_conn(|conn| -> anyhow::Result<_> {
        Ok((
            video_radio::familiar_artist_ids(conn)?,
            video_sets::load_anchor_pool(conn)?,
        ))
    }) {
        Ok(values) => values,
        Err(e) => {
            tracing::warn!("video radio taste read failed: {e}");
            return Json(video_radio_payload(&[], &HashSet::new(), false));
        }
    };
    let load = || -> anyhow::Result<(Vec<video_sets::VideoSetItem>, usize)> {
        db.with_conn(|conn| {
            let graph = graph::cached(conn)?;
            let (recent, library): (&[i64], &[video_sets::AnchorArtist]) = if seed_id.is_some() {
                (&[], &[])
            } else {
                (&recent_artists, &anchors)
            };
            let pool = video_radio::station_pool(conn, &graph, seed_id, recent, library)?;
            let candidates = video_radio::load_candidates(conn, &pool)?;
            let watched = video_sets::recently_watched_video_ids(conn, RECENTLY_WATCHED_DAYS)?;
            let strict_exclude = excluded.union(&recent_seen).copied().collect();
            let items = if seed_id.is_some() {
                video_radio::select_seeded_batch(
                    &candidates,
                    &strict_exclude,
                    &recent_song_keys,
                    &watched,
                    &recent_artists,
                    12,
                )
            } else {
                video_radio::select_batch(
                    &candidates,
                    &strict_exclude,
                    &watched,
                    &recent_artists,
                    &familiar,
                    12,
                )
            };
            let unfamiliar = video_radio::unfamiliar_artist_count(&items, &familiar, seed_id);
            Ok((items, unfamiliar))
        })
    };
    let (mut items, unfamiliar) = match load() {
        Ok(value) => value,
        Err(e) => {
            tracing::warn!("video radio cache read failed: {e}");
            return Json(video_radio_payload(&[], &familiar, false));
        }
    };
    let mut building = false;
    if let Some(seed) = seed_id
        && video_radio::queue_needs_discovery(items.len(), unfamiliar)
    {
        crawler::request(Urgent::Station(seed));
        if items.iter().all(|item| item.artist_id == Some(seed)) {
            crawler::wait_until_built(Urgent::Station(seed), COLD_START_WAIT).await;
            if let Ok((next, _)) = load() {
                items = next;
            }
        }
        building = crawler::is_building(Urgent::Station(seed));
    }
    Json(video_radio_payload(&items, &familiar, building))
}

/// General library anchors never appear in this section.
#[derive(Deserialize)]
pub(super) struct RelatedVideosRequest {
    pub seed_artist_id: Option<i64>,
    pub seed_artist_name: Option<String>,
    #[serde(default)]
    pub exclude_video_ids: Vec<i64>,
}

pub(super) async fn post_videos_related(
    State(state): State<SharedState>,
    Json(body): Json<RelatedVideosRequest>,
) -> Json<Value> {
    let db = { state.read().await.db.clone() };
    let Some(seed_id) = lookup_seed(
        &db,
        body.seed_artist_id,
        body.seed_artist_name.as_deref(),
        None,
    ) else {
        return Json(json!({ "items": [], "building": false }));
    };
    let items = db
        .with_conn(|conn| -> anyhow::Result<Vec<video_sets::VideoSetItem>> {
            let graph = graph::cached(conn)?;
            let pool = video_radio::station_pool(conn, &graph, Some(seed_id), &[], &[])?;
            let related: Vec<_> = pool
                .iter()
                .filter(|(_, _, lane)| lane.is_close())
                .cloned()
                .collect();
            let candidates = video_radio::load_candidates(conn, &related)?;
            let excluded: HashSet<i64> = body.exclude_video_ids.iter().take(64).copied().collect();
            let watched = video_sets::recently_watched_video_ids(conn, RECENTLY_WATCHED_DAYS)?;
            let mut selected = video_radio::select_seeded_batch(
                &candidates,
                &excluded,
                &HashSet::new(),
                &watched,
                &[seed_id],
                12,
            );
            for video in &mut selected {
                video.why = match video.artist_id.and_then(|id| {
                    related
                        .iter()
                        .find(|(artist, _, _)| *artist == id)
                        .map(|(_, _, lane)| *lane)
                }) {
                    Some(video_radio::SourceLane::Seed) => "More from this artist".into(),
                    Some(video_radio::SourceLane::Direct) => "Related artist".into(),
                    Some(video_radio::SourceLane::Genre) => "Shared genre".into(),
                    _ => String::new(),
                };
            }
            Ok(selected)
        })
        .unwrap_or_default();
    if items.len() < RELATED_HEALTHY {
        crawler::request(Urgent::Station(seed_id));
    }
    Json(json!({ "items": items, "building": crawler::is_building(Urgent::Station(seed_id)) }))
}

fn record_set_exposure(
    set: &VideoSet,
    video_ids: &mut HashSet<i64>,
    artists: &mut HashMap<String, usize>,
) {
    for item in &set.items {
        video_ids.insert(item.tidal_id);
        if let Some(name) = &item.artist_name {
            *artists.entry(name.to_lowercase()).or_insert(0) += 1;
        }
    }
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
    crate::services::video_discovery::governor::note_video_activity();
    let s = state.read().await;
    let result = s.db.with_conn(|conn| -> anyhow::Result<i64> {
        conn.execute(
            "INSERT INTO video_history (tidal_video_id, title, artist_tidal_id, artist_name) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                body.tidal_video_id,
                body.title,
                body.artist_tidal_id,
                body.artist_name
            ],
        )?;
        Ok(conn.last_insert_rowid())
    });
    match result {
        Ok(id) => Json(json!({ "ok": true, "id": id })),
        Err(e) => {
            tracing::warn!(
                target = "noor.videos",
                event = "history_write_failed",
                "video history write failed: {e}"
            );
            Json(json!({ "ok": true }))
        }
    }
}

#[derive(Deserialize)]
pub(super) struct FinishVideoPlay {
    pub watched_ms: i64,
    #[serde(default)]
    pub video_duration_ms: Option<i64>,
    #[serde(default)]
    pub completed: bool,
}

/// `POST /api/videos/history/{id}/finish`. Watch time feeds enjoyed roots.
pub(super) async fn post_videos_history_finish(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    Json(body): Json<FinishVideoPlay>,
) -> Json<Value> {
    const MAX_MS: i64 = 24 * 3600 * 1000;
    let valid = id > 0
        && (0..=MAX_MS).contains(&body.watched_ms)
        && body
            .video_duration_ms
            .is_none_or(|d| (0..=MAX_MS).contains(&d));
    if !valid {
        return Json(json!({ "ok": false }));
    }
    let s = state.read().await;
    let updated =
        s.db.with_conn(|conn| {
            Ok(conn.execute(
                "UPDATE video_history SET
                     duration_watched_ms = MAX(COALESCE(duration_watched_ms, 0), ?2),
                     video_duration_ms = COALESCE(?3, video_duration_ms),
                     completed = MAX(completed, ?4)
                 WHERE id = ?1",
                params![
                    id,
                    body.watched_ms,
                    body.video_duration_ms,
                    i64::from(body.completed)
                ],
            )?)
        })
        .unwrap_or(0);
    Json(json!({ "ok": updated > 0 }))
}

/// `GET /api/videos/discovery/status`. Crawler progress for verification.
pub(super) async fn get_video_discovery_status(State(state): State<SharedState>) -> Json<Value> {
    let db = { state.read().await.db.clone() };
    match db.with_conn(crawler::status) {
        Ok(status) => Json(json!(status)),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

// ── Liked videos ─────────────────────────────────────────────────────────────
//
// The library surface, as opposed to the editorial one above: a wall built from
// videos found for songs the user already favorited. Reads are pure SQL over
// what the background pass in `services::library_videos` has resolved so far,
// so this never touches TIDAL and never blocks.

/// `GET /api/videos/liked`. The wall plus how far the background resolve has
/// got, so a first run on an existing library reads as filling in rather than
/// as broken.
pub(super) async fn get_videos_liked(State(state): State<SharedState>) -> Json<Value> {
    let s = state.read().await;
    let wall =
        s.db.with_conn(library_videos::load_wall)
            .unwrap_or_default();
    let progress =
        s.db.with_conn(library_videos::scan_progress)
            .unwrap_or(library_videos::ScanProgress {
                scanned_artists: 0,
                total_artists: 0,
            });
    let running = s
        .library_video_scan_running
        .load(std::sync::atomic::Ordering::SeqCst);
    let connected = s.tidal_tokens.is_some();

    Json(json!({
        "videos": wall,
        "scanned_artists": progress.scanned_artists,
        "total_artists": progress.total_artists,
        "running": running,
        "tidal_connected": connected,
    }))
}

/// Exact video cuts deliberately saved by the listener. This is a local read;
/// discovery and artwork are never fetched when opening the saved list.
pub(super) async fn get_saved_videos(State(state): State<SharedState>) -> Json<Value> {
    let s = state.read().await;
    let items =
        s.db.with_conn(|conn| -> anyhow::Result<Vec<Value>> {
            let mut stmt = conn
                .prepare("SELECT item_json FROM saved_videos ORDER BY saved_at DESC LIMIT 1000")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            Ok(rows
                .filter_map(|row| {
                    row.ok()
                        .and_then(|raw| serde_json::from_str::<video_sets::VideoSetItem>(&raw).ok())
                        .filter(valid_saved_video)
                        .and_then(|video| serde_json::to_value(video).ok())
                })
                .collect())
        })
        .unwrap_or_default();
    Json(json!({ "items": items }))
}

#[derive(Deserialize)]
pub(super) struct SaveVideoRequest {
    pub video: video_sets::VideoSetItem,
    pub saved: bool,
}

fn valid_saved_video(video: &video_sets::VideoSetItem) -> bool {
    video.tidal_id > 0
        && !video.title.trim().is_empty()
        && video.title.len() <= 500
        && video.duration_ms.is_none_or(|duration| duration >= 0)
        && video.artist_id.is_none_or(|id| id > 0)
        && video.album_tidal_id.is_none_or(|id| id > 0)
        && video
            .artist_name
            .as_ref()
            .is_none_or(|name| name.len() <= 500)
        && video
            .artwork_url
            .as_ref()
            .is_none_or(|url| url.len() <= 2048)
        && !video.kind.trim().is_empty()
        && video.kind.len() <= 80
        && video.why.len() <= 500
        && video
            .quality
            .as_ref()
            .is_none_or(|quality| quality.len() <= 80)
}

pub(super) async fn post_saved_video(
    State(state): State<SharedState>,
    Json(body): Json<SaveVideoRequest>,
) -> Json<Value> {
    if !valid_saved_video(&body.video) {
        return Json(json!({ "ok": false }));
    }
    let id = body.video.tidal_id;
    let s = state.read().await;
    let result = s.db.with_conn(|conn| -> anyhow::Result<()> {
        if body.saved {
            conn.execute(
                "INSERT INTO saved_videos (tidal_video_id, item_json) VALUES (?1, ?2) \
                 ON CONFLICT(tidal_video_id) DO UPDATE SET item_json = excluded.item_json",
                params![id, serde_json::to_string(&body.video)?],
            )?;
        } else {
            conn.execute("DELETE FROM saved_videos WHERE tidal_video_id = ?1", [id])?;
        }
        Ok(())
    });
    if let Err(e) = &result {
        tracing::warn!("saved video write failed: {e}");
    }
    Json(json!({ "ok": result.is_ok() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn state(db: crate::db::Database) -> SharedState {
        Arc::new(tokio::sync::RwLock::new(
            crate::server::routes::tests::fresh_test_state(db),
        ))
    }

    #[tokio::test]
    async fn related_reads_the_cache_and_asks_the_crawler_for_more() {
        let db = crate::server::routes::tests::fresh_migrated_db();
        let response = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            post_videos_related(
                State(state(db)),
                Json(RelatedVideosRequest {
                    seed_artist_id: Some(4242),
                    seed_artist_name: None,
                    exclude_video_ids: Vec::new(),
                }),
            ),
        )
        .await
        .expect("related never waits on TIDAL");
        assert_eq!(response.0["items"], json!([]));
        assert_eq!(response.0["building"], json!(true));
    }

    #[tokio::test]
    async fn radio_refill_answers_from_the_graph_without_tidal() {
        let db = crate::server::routes::tests::fresh_migrated_db();
        db.with_conn(|conn| {
            let video = |id, artist| video_sets::VideoCandidate {
                tidal_id: id,
                title: format!("Song {id}"),
                artist_id: Some(artist),
                artist_name: Some(format!("Artist {artist}")),
                ..Default::default()
            };
            let anchor = |id| video_sets::AnchorArtist { tidal_id: id, name: format!("Artist {id}"), listens: 1, via: None };
            video_radio::cache_groups(conn, &[(anchor(4343), vec![video(1, 4343)]), (anchor(4344), vec![video(2, 4344)])])?;
            conn.execute(
                "INSERT INTO video_related_artists (seed_tidal_id, related_tidal_id, name, source, rank, weight)
                 VALUES (4343, 4344, 'Artist 4344', 'tidal', 0, 1.0)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            post_videos_radio_next(
                State(state(db)),
                Json(VideoRadioRequest {
                    seed_artist_id: Some(4343),
                    seed_artist_name: None,
                    seed_video_id: Some(1),
                    exclude_video_ids: vec![1],
                    recent_video_ids: Vec::new(),
                    recent_songs: Vec::new(),
                    recent_artist_ids: vec![4343],
                }),
            ),
        )
        .await
        .expect("refill answers from the cache");
        let ids: Vec<i64> = response.0["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["tidal_id"].as_i64().unwrap())
            .collect();
        assert_eq!(ids, vec![2]);
        assert!(response.0["building"].is_boolean());
    }

    #[tokio::test]
    async fn finishing_a_watch_records_time_and_completion() {
        let db = crate::server::routes::tests::fresh_migrated_db();
        let shared = state(db.clone());
        let started = post_videos_history(
            State(shared.clone()),
            Json(RecordVideoPlay {
                tidal_video_id: 77,
                title: Some("Song".into()),
                artist_tidal_id: Some(9),
                artist_name: None,
            }),
        )
        .await;
        let id = started.0["id"].as_i64().expect("history id");
        let finished = post_videos_history_finish(
            State(shared),
            Path(id),
            Json(FinishVideoPlay {
                watched_ms: 170_000,
                video_duration_ms: Some(180_000),
                completed: true,
            }),
        )
        .await;
        assert_eq!(finished.0["ok"], json!(true));
        let row: (i64, i64, bool) = db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT duration_watched_ms, video_duration_ms, completed FROM video_history WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?)
            })
            .unwrap();
        assert_eq!(row, (170_000, 180_000, true));
    }
}

/// `POST /api/videos/liked/refresh`. The manual affordance for the impatient;
/// the automatic path is the `LibrarySynced` listener and the daily sweep.
/// `run_if_idle` is a cheap no-op when a pass is already going or nothing is
/// due, so this is safe to hammer.
pub(super) async fn post_videos_liked_refresh(State(state): State<SharedState>) -> Json<Value> {
    library_videos::run_if_idle(state.clone()).await;
    let s = state.read().await;
    Json(json!({
        "running": s
            .library_video_scan_running
            .load(std::sync::atomic::Ordering::SeqCst),
    }))
}

/// The version to hide. Matching is loose on purpose, so the wrong video lands
/// on a song often enough to need a correction.
///
/// `track_ids` is the card's whole set of liked rows, not one row: a song
/// favorited twice draws a single card, and suppressing only half of it would
/// just redraw from the other half.
#[derive(Deserialize)]
pub(super) struct HideLikedVideo {
    pub track_ids: Vec<i64>,
    pub tidal_video_id: i64,
}

/// `POST /api/videos/liked/hide`. Flips one version to suppressed, which also
/// stops the 90-day re-check from bringing it back.
pub(super) async fn post_videos_liked_hide(
    State(state): State<SharedState>,
    Json(body): Json<HideLikedVideo>,
) -> Json<Value> {
    let s = state.read().await;
    let hidden =
        s.db.with_conn(|conn| library_videos::suppress(conn, &body.track_ids, body.tidal_video_id))
            .unwrap_or(false);
    Json(json!({ "ok": hidden }))
}
