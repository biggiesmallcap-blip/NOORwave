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
use crate::services::library_videos;
use crate::services::tidal::client::TidalClient;
use crate::services::video_radio;
use crate::services::video_sets::{
    self, ALBUM_LOVE_SLUG, Archetype, DAILY_PICKS_SLUG, DJ_SETS_SLUG, ERA_SLUG, GENRE_SLUG_PREFIX,
    ONE_STEP_OUT_SLUG, RECENTLY_WATCHED_DAYS, SetPlan, VideoSet,
};
use axum::{extract::State, response::Json};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
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
    pub exclude_video_ids: Vec<i64>,
    #[serde(default)]
    pub recent_video_ids: Vec<i64>,
    #[serde(default)]
    pub recent_artist_ids: Vec<i64>,
}

static VIDEO_RADIO_FETCH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn video_radio_payload(items: &[video_sets::VideoSetItem], familiar: &HashSet<i64>) -> Value {
    let unfamiliar_video_ids: Vec<i64> = items
        .iter()
        .filter(|item| item.artist_id.is_some_and(|id| !familiar.contains(&id)))
        .map(|item| item.tidal_id)
        .collect();
    json!({ "items": items, "unfamiliar_video_ids": unfamiliar_video_ids })
}

pub(super) async fn post_videos_radio_next(
    State(state): State<SharedState>,
    Json(body): Json<VideoRadioRequest>,
) -> Json<Value> {
    let (db, http, tidal_http, tokens) = {
        let s = state.read().await;
        (
            s.db.clone(),
            s.http_client.clone(),
            s.tidal_http_client.clone(),
            s.tidal_tokens.clone(),
        )
    };
    let seed_id = body.seed_artist_id.filter(|id| *id > 0).or_else(|| {
        body.seed_artist_name
            .as_deref()
            .filter(|name| name.len() <= 120)
            .and_then(|name| {
                db.with_conn(|conn| video_radio::local_artist_id(conn, name))
                    .ok()
                    .flatten()
            })
    });
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
            return Json(json!({ "items": [], "unfamiliar_video_ids": [] }));
        }
    };
    let load = || -> anyhow::Result<_> {
        db.with_conn(|conn| {
            let pool = video_radio::artist_pool(conn, seed_id, &recent_artists, &anchors)?;
            let candidates = video_radio::load_candidates(conn, &pool)?;
            let watched = video_sets::recently_watched_video_ids(conn, RECENTLY_WATCHED_DAYS)?;
            let strict_exclude = excluded.union(&recent_seen).copied().collect();
            let fresh = video_radio::select_batch(
                &candidates,
                &strict_exclude,
                &watched,
                &recent_artists,
                &familiar,
                12,
            );
            let items = if fresh.len() >= 4 {
                fresh.clone()
            } else {
                let soft_watched = watched.union(&recent_seen).copied().collect();
                video_radio::select_batch(
                    &candidates,
                    &excluded,
                    &soft_watched,
                    &recent_artists,
                    &familiar,
                    12,
                )
            };
            let unfamiliar_count = items
                .iter()
                .filter(|item| item.artist_id.is_some_and(|id| !familiar.contains(&id)))
                .count();
            Ok::<_, anyhow::Error>((pool, items, fresh.len(), unfamiliar_count))
        })
    };
    let (mut pool, mut items, mut fresh_count, mut unfamiliar_count) = match load() {
        Ok(value) => value,
        Err(e) => {
            tracing::warn!("video radio cache read failed: {e}");
            return Json(json!({ "items": [], "unfamiliar_video_ids": [] }));
        }
    };

    // Single-flight across all sessions. Wait for any active fill, then
    // re-read the cache so simultaneous listeners do not duplicate API calls.
    // The scan ledger also throttles empty catalogs and empty relationships.
    let relationship_due = seed_id.is_some_and(|id| {
        db.with_conn(|conn| video_radio::related_due(conn, id))
            .unwrap_or(false)
    });
    if fresh_count < 8 || unfamiliar_count < video_radio::UNFAMILIAR_PER_BATCH || relationship_due {
        let _guard = VIDEO_RADIO_FETCH_LOCK.lock().await;
        if let Ok((current_pool, current_items, current_fresh_count, current_unfamiliar_count)) =
            load()
        {
            pool = current_pool;
            items = current_items;
            fresh_count = current_fresh_count;
            unfamiliar_count = current_unfamiliar_count;
        }
        if fresh_count >= 8
            && unfamiliar_count >= video_radio::UNFAMILIAR_PER_BATCH
            && !seed_id.is_some_and(|id| {
                db.with_conn(|conn| video_radio::related_due(conn, id))
                    .unwrap_or(false)
            })
        {
            return Json(video_radio_payload(&items, &familiar));
        }
        let tokens = match tokens {
            Some(tokens) => Some(tokens),
            None => super::load_persisted_tidal_tokens(&state)
                .await
                .ok()
                .flatten(),
        };
        if let Some(tokens) = tokens {
            let client =
                TidalClient::with_http(tidal_http, tokens.access_token, tokens.country_code);
            if let Some(id) = seed_id {
                let due = db
                    .with_conn(|conn| video_radio::related_due(conn, id))
                    .unwrap_or(false);
                if due {
                    let mut related: Vec<(i64, String, &'static str)> = Vec::new();
                    if let Some(name) = body
                        .seed_artist_name
                        .as_deref()
                        .filter(|s| !s.trim().is_empty() && s.len() <= 120)
                        && let Some(lastfm) = crate::metadata::lastfm::LastFmClient::load(http, &db)
                    {
                        if let Ok(similar) = lastfm.artist_get_similar(name, 8).await {
                            let mut resolved_external = 0;
                            for artist in similar {
                                if let Ok(Some(local_id)) = db.with_conn(|conn| {
                                    video_radio::local_artist_id(conn, &artist.name)
                                }) {
                                    related.push((local_id, artist.name, "lastfm"));
                                } else if resolved_external < 2 {
                                    // Last.fm provides names, while TIDAL videos
                                    // need artist IDs. Resolve only two exact
                                    // names per weekly relationship refresh.
                                    resolved_external += 1;
                                    if let Ok(found) =
                                        client.search_catalog_core(&artist.name, 3, 0).await
                                        && let Some(matched) =
                                            found.artists.into_iter().find(|item| {
                                                item.name.eq_ignore_ascii_case(&artist.name)
                                            })
                                    {
                                        related.push((matched.id, matched.name, "lastfm"));
                                    }
                                }
                            }
                        }
                        if let Ok(tags) = lastfm.artist_top_tags(name).await {
                            let genres: Vec<String> =
                                tags.into_iter().take(5).map(|(name, _)| name).collect();
                            if let Err(e) = db
                                .with_conn(|conn| video_radio::store_seed_genres(conn, id, &genres))
                            {
                                tracing::debug!("video radio genre cache failed: {e}");
                            }
                        }
                    }
                    if let Ok(similar) = client.get_artist_similar(id, 10, 0).await {
                        related.extend(
                            similar
                                .items
                                .into_iter()
                                .map(|artist| (artist.id, artist.name, "tidal")),
                        );
                    }
                    if let Err(e) =
                        db.with_conn(|conn| video_radio::store_related(conn, id, &related))
                    {
                        tracing::warn!("video radio relationship cache failed: {e}");
                    }
                    if let Ok(next) = db.with_conn(|conn| {
                        video_radio::artist_pool(conn, seed_id, &recent_artists, &anchors)
                    }) {
                        pool = next;
                    }
                }
            }
            // Fetch related unfamiliar artists first, even when familiar cache
            // entries could already fill the batch. The two-call ceiling keeps
            // this lane deliberate and cheap.
            let mut fetch_pool = pool.clone();
            fetch_pool.sort_by_key(|(id, _, lane)| {
                (
                    if !familiar.contains(id) && *lane <= 2 {
                        0
                    } else if *lane == 0 {
                        1
                    } else if !familiar.contains(id) {
                        2
                    } else {
                        3
                    },
                    *lane,
                )
            });
            let mut fetched = 0;
            for (id, name, _) in &fetch_pool {
                if fetched >= 2 {
                    break;
                }
                if *id <= 0
                    || db
                        .with_conn(|conn| video_radio::artist_due(conn, *id))
                        .unwrap_or(false)
                        == false
                {
                    continue;
                }
                // Mark even empty or failed scans: an unavailable artist should
                // not be requested again on every song boundary.
                let _ = db.with_conn(|conn| video_radio::mark_artist_scanned(conn, *id));
                fetched += 1;
                match client.get_artist_videos(*id, 20, 0).await {
                    Ok(page) => {
                        let anchor = video_sets::AnchorArtist {
                            tidal_id: *id,
                            name: name.clone(),
                            listens: 1,
                            via: None,
                        };
                        let videos = page
                            .items
                            .iter()
                            .map(video_sets::VideoCandidate::from)
                            .collect();
                        if let Err(e) = db
                            .with_conn(|conn| video_radio::cache_groups(conn, &[(anchor, videos)]))
                        {
                            tracing::warn!("video radio catalog write failed: {e}");
                        }
                    }
                    Err(e) => tracing::debug!("video radio artist {id} fetch failed: {e}"),
                }
            }
            if let Ok((_, refreshed, _, unfamiliar)) = load() {
                items = refreshed;
                unfamiliar_count = unfamiliar;
            }
            // A genre search is a separate exploration lane. It can surface
            // artists outside both the library and the similar-artist graph.
            // The genre ledger permits one search per genre per 14 days and
            // one search globally per 30 minutes.
            if unfamiliar_count < video_radio::UNFAMILIAR_PER_BATCH
                && let Some(id) = seed_id
                && let Ok(Some(genre)) = db.with_conn(|conn| video_radio::seed_genre(conn, id))
                && db
                    .with_conn(|conn| video_radio::genre_due(conn, &genre))
                    .unwrap_or(false)
            {
                let _ = db.with_conn(|conn| video_radio::mark_genre_scanned(conn, &genre));
                let query = format!("{genre} music video");
                if let Ok(found) = client.search_videos(&query, 20, 0).await {
                    let videos: Vec<video_sets::VideoCandidate> =
                        found.iter().map(video_sets::VideoCandidate::from).collect();
                    let anchor = video_sets::AnchorArtist {
                        tidal_id: -1,
                        name: query,
                        listens: 1,
                        via: None,
                    };
                    if let Err(e) = db.with_conn(|conn| {
                        video_radio::cache_groups(conn, &[(anchor, videos.clone())])?;
                        video_radio::store_genre_artists(conn, id, &videos)
                    }) {
                        tracing::warn!("video radio genre catalog write failed: {e}");
                    }
                    if let Ok((_, refreshed, _, _)) = load() {
                        items = refreshed;
                    }
                }
            }
        }
    }
    Json(video_radio_payload(&items, &familiar))
}

/// Related videos read the cached catalog immediately. A separate, bounded
/// background pass fills missing links without waiting for a radio refill.
static VIDEO_RELATED_BUILD_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

fn kick_related_build(state: &SharedState, seed_id: i64) -> bool {
    if VIDEO_RELATED_BUILD_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return true;
    }
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(e) = build_related_cache(&state, seed_id).await {
            tracing::warn!("video related cache build failed: {e}");
        }
        VIDEO_RELATED_BUILD_IN_FLIGHT.store(false, Ordering::SeqCst);
    });
    true
}

async fn build_related_cache(state: &SharedState, seed_id: i64) -> anyhow::Result<()> {
    let (db, tidal_http, tokens) = {
        let s = state.read().await;
        (
            s.db.clone(),
            s.tidal_http_client.clone(),
            s.tidal_tokens.clone(),
        )
    };
    let tokens = match tokens {
        Some(tokens) => Some(tokens),
        None => super::load_persisted_tidal_tokens(state)
            .await
            .ok()
            .flatten(),
    };
    let Some(tokens) = tokens else {
        return Ok(());
    };
    if !db.with_conn(|conn| video_radio::related_due(conn, seed_id))? {
        return Ok(());
    }
    let client = TidalClient::with_http(tidal_http, tokens.access_token, tokens.country_code);
    // One relationship request and at most two artist catalogs per pass. The
    // scan ledgers suppress repeated requests for empty or unavailable artists.
    let similar = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        client.get_artist_similar(seed_id, 10, 0),
    )
    .await;
    let related: Vec<(i64, String, &'static str)> = match similar {
        Ok(Ok(page)) => page
            .items
            .into_iter()
            .map(|artist| (artist.id, artist.name, "tidal"))
            .collect(),
        Ok(Err(e)) => {
            tracing::debug!("video related artists fetch failed: {e}");
            Vec::new()
        }
        Err(_) => Vec::new(),
    };
    db.with_conn(|conn| video_radio::store_related(conn, seed_id, &related))?;
    let mut pool = db.with_conn(|conn| video_radio::artist_pool(conn, Some(seed_id), &[], &[]))?;
    pool.sort_by_key(|(_, _, lane)| if *lane == 0 { 2 } else { *lane });
    let mut fetched = 0;
    for (id, name, _) in pool.into_iter().filter(|(_, _, lane)| *lane <= 2) {
        if fetched >= 2 {
            break;
        }
        if !db.with_conn(|conn| video_radio::artist_due(conn, id))? {
            continue;
        }
        db.with_conn(|conn| video_radio::mark_artist_scanned(conn, id))?;
        fetched += 1;
        match tokio::time::timeout(
            std::time::Duration::from_secs(8),
            client.get_artist_videos(id, 20, 0),
        )
        .await
        {
            Ok(Ok(page)) => {
                let anchor = video_sets::AnchorArtist {
                    tidal_id: id,
                    name,
                    listens: 1,
                    via: None,
                };
                let videos = page
                    .items
                    .iter()
                    .map(video_sets::VideoCandidate::from)
                    .collect();
                db.with_conn(|conn| video_radio::cache_groups(conn, &[(anchor, videos)]))?;
            }
            Ok(Err(e)) => tracing::debug!("video related artist {id} fetch failed: {e}"),
            Err(_) => tracing::debug!("video related artist {id} fetch timed out"),
        }
    }
    Ok(())
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
    let seed_id = body.seed_artist_id.filter(|id| *id > 0).or_else(|| {
        body.seed_artist_name
            .as_deref()
            .filter(|name| name.len() <= 120)
            .and_then(|name| {
                db.with_conn(|conn| video_radio::local_artist_id(conn, name))
                    .ok()
                    .flatten()
            })
    });
    let Some(seed_id) = seed_id else {
        return Json(json!({ "items": [], "building": false }));
    };
    let due = db
        .with_conn(|conn| video_radio::related_due(conn, seed_id))
        .unwrap_or(false);
    let building = if due {
        let connected = { state.read().await.tidal_tokens.is_some() };
        let connected = connected
            || super::load_persisted_tidal_tokens(&state)
                .await
                .ok()
                .flatten()
                .is_some();
        connected && kick_related_build(&state, seed_id)
    } else {
        VIDEO_RELATED_BUILD_IN_FLIGHT.load(Ordering::SeqCst)
    };
    let s = state.read().await;
    let items =
        s.db.with_conn(|conn| -> anyhow::Result<Vec<video_sets::VideoSetItem>> {
            let pool = video_radio::artist_pool(conn, Some(seed_id), &[], &[])?;
            let related: Vec<_> = pool
                .iter()
                .filter(|(_, _, lane)| *lane <= 2)
                .cloned()
                .collect();
            let candidates = video_radio::load_candidates(conn, &related)?;
            let excluded: HashSet<i64> = body.exclude_video_ids.iter().take(64).copied().collect();
            let watched = video_sets::recently_watched_video_ids(conn, RECENTLY_WATCHED_DAYS)?;
            let familiar = video_radio::familiar_artist_ids(conn)?;
            let mut selected = video_radio::select_batch(
                &candidates,
                &excluded,
                &watched,
                &[seed_id],
                &familiar,
                12,
            );
            for video in &mut selected {
                video.why = match video.artist_id.and_then(|id| {
                    related
                        .iter()
                        .find(|(artist, _, _)| *artist == id)
                        .map(|(_, _, lane)| *lane)
                }) {
                    Some(0) => "More from this artist".into(),
                    Some(1) => "Related artist".into(),
                    Some(2) => "Shared genre".into(),
                    _ => String::new(),
                };
            }
            Ok(selected)
        })
        .unwrap_or_default();
    Json(json!({ "items": items, "building": building }))
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

    #[tokio::test]
    async fn related_response_does_not_wait_for_radio_fetch_lock() {
        let db = crate::server::routes::tests::fresh_migrated_db();
        let state = Arc::new(tokio::sync::RwLock::new(
            crate::server::routes::tests::fresh_test_state(db),
        ));
        let _radio_guard = VIDEO_RADIO_FETCH_LOCK.lock().await;
        let response = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            post_videos_related(
                State(state),
                Json(RelatedVideosRequest {
                    seed_artist_id: Some(42),
                    seed_artist_name: Some("Artist".into()),
                    exclude_video_ids: Vec::new(),
                }),
            ),
        )
        .await
        .expect("related response must use cache without waiting for radio");
        assert!(response.0["items"].is_array());
        assert!(response.0["building"].is_boolean());
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
