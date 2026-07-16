use crate::SharedState;
use crate::db::queries;
use crate::metadata::lastfm::{
    LastFmChartAlbum, LastFmChartArtist, LastFmChartTrack, LastFmClient,
};
use axum::{extract::State, http::StatusCode, response::Json};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
/// Get new album releases from AllMusic RSS
pub(super) async fn get_home_releases(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use crate::services::lastfm;

    // Pull api_key from the existing Last.fm credentials row. If Last.fm
    // isn't configured, we 503 so the frontend renders the connect/empty
    // state instead of falling back to the old AllMusic RSS feed.
    let (http, api_key) = {
        let s = state.read().await;
        let api_key =
            s.db.with_conn(|conn| Ok(lastfm::auth::load_credentials(conn).ok().flatten()))
                .ok()
                .flatten()
                .map(|c| c.api_key);
        (s.http_client.clone(), api_key)
    };
    let Some(api_key) = api_key.filter(|k| !k.is_empty()) else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };

    match lastfm::releases::fetch_new_releases_cached(&http, &api_key).await {
        Ok(releases) => Ok(Json(json!({
            "releases": releases,
            "source": "lastfm_api",
        }))),
        Err(e) => {
            tracing::warn!("Last.fm new-releases pipeline failed: {e}");
            Err(StatusCode::BAD_GATEWAY)
        }
    }
}

/// Get daily picks curated from user's library using learning model
pub(super) async fn get_home_picks(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    const PICKS_TTL: std::time::Duration = std::time::Duration::from_secs(2 * 60 * 60);
    let state_guard = state.read().await;

    // Serve from the in-process TTL cache when fresh. Avoids the ORDER BY RANDOM()
    // full scan (and the top-played query) on every home remount / tab switch.
    {
        let cache = state_guard.home_picks_cache.lock().unwrap();
        if let Some((computed_at, payload)) = cache.as_ref() {
            if computed_at.elapsed() < PICKS_TTL {
                return Ok(Json(payload.clone()));
            }
        }
    }

    let db = &state_guard.db;

    // Get top tracks from listening history with variety
    let picks = db
        .with_conn(|conn| {
            // Fetch recent top tracks that aren't played in last 7 days (rediscovery)
            let tracks = queries::get_tracks(conn, "play_count", "desc", 20, 0, false, false)?;

            // Get tracks from different genres for variety
            let mut genre_tracks = conn.prepare(
                "SELECT t.*, g.name as genre_name
             FROM tracks t
             JOIN track_genres tg ON t.id = tg.track_id
             JOIN genres g ON tg.genre_id = g.id
             WHERE t.play_count > 0
             ORDER BY RANDOM()
             LIMIT 10",
            )?;

            let genre_picks: Vec<serde_json::Value> = genre_tracks
                .query_map([], |row| {
                    Ok(serde_json::json!({
                        "id": row.get::<_, i64>(0)?,
                        "title": row.get::<_, String>(1)?,
                        "artist_name": row.get::<_, Option<String>>(2)?,
                        "album_title": row.get::<_, Option<String>>(3)?,
                        "artwork_url": row.get::<_, Option<String>>(4)?,
                        "duration_ms": row.get::<_, Option<i64>>(5)?,
                        "play_count": row.get::<_, i64>(6)?,
                        "genre": row.get::<_, String>(7)?,
                    }))
                })?
                .filter_map(|r| r.ok())
                .collect();

            Ok((tracks, genre_picks))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let (top_tracks, genre_picks) = picks;

    let payload = json!({
        "top_picks": top_tracks.iter().take(10).map(|t| serde_json::json!({
            "id": t.id,
            "title": t.title,
            "artist_name": t.artist_name,
            "album_title": t.album_title,
            "artwork_url": t.artwork_url,
            "duration_ms": t.duration_ms,
            "play_count": t.play_count,
            "reason": "Most played"
        })).collect::<Vec<_>>(),
        "genre_variety": genre_picks,
        "source": "library_curation"
    });

    {
        let mut cache = state_guard.home_picks_cache.lock().unwrap();
        *cache = Some((std::time::Instant::now(), payload.clone()));
    }

    Ok(Json(payload))
}

pub(super) async fn get_home_recommendations(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let lastfm = load_or_fetch_recommendation_shelf(state.clone(), "lastfm").await;
    let listenbrainz = load_or_fetch_recommendation_shelf(state.clone(), "listenbrainz").await;
    Ok(Json(json!({
        "shelves": [
            recommendation_shelf_json("lastfm", "Last.fm recommended tracks", Some("track"), &lastfm),
            recommendation_shelf_json("lastfm", "Last.fm recommended artists", Some("artist"), &lastfm),
            recommendation_shelf_json("lastfm", "Last.fm recommended albums", Some("album"), &lastfm),
            recommendation_shelf_json("listenbrainz", "ListenBrainz recommends", Some("track"), &listenbrainz),
        ]
    })))
}

const RECOMMENDATION_HOME_CACHE_KEY: &str = "home:v6";
const LASTFM_HOME_RECOMMENDATION_LIMIT: usize = 20;
const LASTFM_HOME_SEED_LIMIT: usize = 12;
const LASTFM_HOME_PROFILE_SOURCE_LIMIT: usize = 30;
const LASTFM_HOME_RECENT_SEED_TARGET: usize = 8;
const LASTFM_HOME_LOVED_SEED_TARGET: usize = 8;
const LASTFM_HOME_TOP_SEED_TARGET: usize = 6;
const LASTFM_HOME_SIMILAR_LIMIT: usize = 20;
const LASTFM_HOME_ARTIST_LIMIT: usize = 20;
const LASTFM_HOME_ALBUM_LIMIT: usize = 20;
const LASTFM_HOME_ALBUM_SIMILAR_ARTIST_LIMIT: usize = 8;
const LASTFM_HOME_ALBUMS_PER_ARTIST_LIMIT: usize = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LastFmTrackSeed {
    pub(crate) artist: String,
    pub(crate) title: String,
    pub(crate) reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LastFmArtistSeed {
    pub(crate) name: String,
    pub(crate) reason: String,
}

fn recommendation_shelf_json(
    provider: &str,
    title: &str,
    entity_type: Option<&str>,
    result: &anyhow::Result<Vec<Value>>,
) -> Value {
    match result {
        Ok(items) => {
            let filtered = filter_recommendation_items(items, entity_type);
            json!({
                "provider": provider,
                "title": title,
                "entity_type": entity_type.unwrap_or("track"),
                "status": if filtered.is_empty() { "empty" } else { "ok" },
                "items": filtered,
            })
        }
        Err(error) => json!({
            "provider": provider,
            "title": title,
            "entity_type": entity_type.unwrap_or("track"),
            "status": "error",
            "message": error.to_string(),
            "items": [],
        }),
    }
}

fn filter_recommendation_items(items: &[Value], entity_type: Option<&str>) -> Vec<Value> {
    let wanted = entity_type.unwrap_or("track");
    items
        .iter()
        .filter(|item| {
            item.get("entity_type")
                .and_then(Value::as_str)
                .unwrap_or("track")
                == wanted
        })
        .cloned()
        .collect()
}

async fn load_or_fetch_recommendation_shelf(
    state: SharedState,
    provider: &str,
) -> anyhow::Result<Vec<Value>> {
    if let Some(cached) = read_recommendation_cache(&state, provider).await {
        return Ok(cached);
    }
    let items = match provider {
        "lastfm" => fetch_lastfm_home_recommendations(&state).await?,
        "listenbrainz" => fetch_listenbrainz_home_recommendations(&state).await?,
        _ => Vec::new(),
    };
    write_recommendation_cache(&state, provider, &items).await;
    Ok(items)
}

async fn read_recommendation_cache(state: &SharedState, provider: &str) -> Option<Vec<Value>> {
    let now = unix_now_secs();
    let s = state.read().await;
    s.db.with_conn(|conn| {
        conn.query_row(
            "SELECT payload_json FROM provider_recommendation_cache
                  WHERE provider = ?1 AND cache_key = ?2 AND expires_at > ?3",
            params![provider, RECOMMENDATION_HOME_CACHE_KEY, now],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(Into::into)
    })
    .ok()
    .flatten()
    .and_then(|raw| serde_json::from_str(&raw).ok())
}

async fn write_recommendation_cache(state: &SharedState, provider: &str, items: &[Value]) {
    let now = unix_now_secs();
    let expires = now + 6 * 60 * 60;
    let Ok(payload) = serde_json::to_string(items) else {
        return;
    };
    let s = state.read().await;
    let _ = s.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO provider_recommendation_cache (provider, cache_key, payload_json, fetched_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(provider, cache_key) DO UPDATE SET
                 payload_json = excluded.payload_json,
                 fetched_at = excluded.fetched_at,
                 expires_at = excluded.expires_at",
            params![provider, RECOMMENDATION_HOME_CACHE_KEY, payload, now, expires],
        )?;
        Ok::<_, anyhow::Error>(())
    });
}

// ── Home suggestions (library-resolved, cross-artist murals) ────────────────
//
// Powers the Library home "Suggested tracks / albums" murals. The old client
// path expanded the seeds into "more tracks by the same artists / same albums",
// which produced clone-of-what-you-just-played suggestions. This endpoint runs
// the real radio blend (embedding neighbours + Last.fm similar + same-artist
// fallback) per seed, keeps only library-resolved candidates so the murals can
// still play tracks and open albums, then ranks with a per-artist cap and hub
// suppression so no single artist floods a panel. Cached 6h per seed set.

// Widened so the album panel (one card per distinct album) has enough
// cross-artist variety to fill without leaning on the same-artist tail: more
// seeds and more neighbours per seed pull in more distinct artists/albums.
const HOME_SUGGESTIONS_SEED_LIMIT: usize = 6;
const HOME_SUGGESTIONS_PER_SEED: usize = 24;
const HOME_SUGGESTIONS_DEFAULT_LIMIT: usize = 50;
const HOME_SUGGESTIONS_MAX_PER_ARTIST: usize = 2;

#[derive(Debug, serde::Deserialize)]
pub(super) struct HomeSuggestionsRequest {
    #[serde(default)]
    seed_track_ids: Vec<i64>,
    limit: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct HomeSuggestionCandidate {
    pub(crate) track_id: i64,
    pub(crate) artist_key: String,
    pub(crate) score: f64,
    pub(crate) seed_hits: u32,
    pub(crate) hub_pct: f64,
}

/// Pure ranking pass over the aggregated library candidates. Sorts by
/// similarity, boosted when several seeds surface the same candidate
/// (consensus) and dampened by how "hubby" the candidate is in the similarity
/// graph, then greedily caps how many tracks any one artist can contribute so a
/// single prolific neighbour cannot fill the whole panel. Returns track ids in
/// final display order. No DB access -> unit-testable.
pub(crate) fn merge_home_suggestions(
    mut candidates: Vec<HomeSuggestionCandidate>,
    limit: usize,
    max_per_artist: usize,
) -> Vec<i64> {
    fn rank_of(c: &HomeSuggestionCandidate) -> f64 {
        let consensus = 1.0 + 0.15 * (c.seed_hits.saturating_sub(1) as f64);
        let hub_damp = 1.0 - 0.40 * c.hub_pct.clamp(0.0, 1.0);
        c.score.max(0.0) * consensus * hub_damp
    }
    candidates.sort_by(|a, b| {
        rank_of(b)
            .partial_cmp(&rank_of(a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.track_id.cmp(&b.track_id))
    });
    let mut per_artist: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    for c in candidates {
        if out.len() >= limit {
            break;
        }
        // Empty artist key (missing name) is never capped so those candidates
        // don't all collapse into one synthetic "artist" bucket.
        if max_per_artist > 0 && !c.artist_key.is_empty() {
            let count = per_artist.entry(c.artist_key).or_insert(0);
            if *count >= max_per_artist {
                continue;
            }
            *count += 1;
        }
        out.push(c.track_id);
    }
    out
}

fn home_suggestions_cache_key(seed_ids: &[i64], limit: usize) -> String {
    let mut sorted = seed_ids.to_vec();
    sorted.sort_unstable();
    let joined = sorted
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join("-");
    format!("home_suggest:v1:{limit}:{joined}")
}

async fn read_home_suggestions_cache(state: &SharedState, cache_key: &str) -> Option<Value> {
    let now = unix_now_secs();
    let key = cache_key.to_string();
    let s = state.read().await;
    s.db.with_conn(|conn| {
        conn.query_row(
            "SELECT payload_json FROM provider_recommendation_cache
                  WHERE provider = 'home_suggestions' AND cache_key = ?1 AND expires_at > ?2",
            params![key, now],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(Into::into)
    })
    .ok()
    .flatten()
    .and_then(|raw| serde_json::from_str(&raw).ok())
}

async fn write_home_suggestions_cache(state: &SharedState, cache_key: &str, payload: &Value) {
    let now = unix_now_secs();
    let expires = now + 6 * 60 * 60;
    let Ok(serialized) = serde_json::to_string(payload) else {
        return;
    };
    let key = cache_key.to_string();
    let s = state.read().await;
    let _ = s.db.with_conn(|conn| {
        // Prune expired rows so the per-seed-set keys don't accumulate unbounded.
        let _ = conn.execute(
            "DELETE FROM provider_recommendation_cache
                  WHERE provider = 'home_suggestions' AND expires_at < ?1",
            params![now],
        );
        conn.execute(
            "INSERT INTO provider_recommendation_cache (provider, cache_key, payload_json, fetched_at, expires_at)
             VALUES ('home_suggestions', ?1, ?2, ?3, ?4)
             ON CONFLICT(provider, cache_key) DO UPDATE SET
                 payload_json = excluded.payload_json,
                 fetched_at = excluded.fetched_at,
                 expires_at = excluded.expires_at",
            params![key, serialized, now, expires],
        )?;
        Ok::<_, anyhow::Error>(())
    });
}

/// POST /api/home/suggestions - ranked, cross-artist, library-resolved picks
/// for the Library home murals. Body: `{ seed_track_ids, limit? }`.
pub(super) async fn get_home_suggestions(
    State(state): State<SharedState>,
    Json(req): Json<HomeSuggestionsRequest>,
) -> Result<Json<Value>, StatusCode> {
    let limit = req
        .limit
        .unwrap_or(HOME_SUGGESTIONS_DEFAULT_LIMIT)
        .clamp(1, 60);

    // Unique, positive seeds, capped. Recent-first order is preserved.
    let mut seen = HashSet::new();
    let seeds: Vec<i64> = req
        .seed_track_ids
        .iter()
        .copied()
        .filter(|&id| id > 0 && seen.insert(id))
        .take(HOME_SUGGESTIONS_SEED_LIMIT)
        .collect();

    if seeds.is_empty() {
        return Ok(Json(json!({ "tracks": [] })));
    }

    let cache_key = home_suggestions_cache_key(&seeds, limit);
    if let Some(cached) = read_home_suggestions_cache(&state, &cache_key).await {
        return Ok(Json(cached));
    }

    let (db, lastfm, lastfm_similar_cache) = {
        let g = state.read().await;
        let lastfm = crate::metadata::lastfm::LastFmClient::load(g.http_client.clone(), &g.db);
        (g.db.clone(), lastfm, g.lastfm_similar_cache.clone())
    };

    // Aggregate library-resolved candidates across seeds. Dedup by track id,
    // count how many seeds surfaced each (consensus) and keep the best score.
    // Run the per-seed blends concurrently. Each orchestrate_song makes Last.fm
    // calls, so a sequential loop over 6 seeds stacked their latency (~8s cold);
    // join_all overlaps the network waits while the shared DB lock serialises the
    // query parts. Cached 6h afterwards, so this cost is paid once per seed set.
    let queues = futures::future::join_all(seeds.iter().map(|&seed_id| {
        let db = &db;
        let lastfm = &lastfm;
        let cache = &lastfm_similar_cache;
        let seeds_ref = &seeds;
        async move {
            crate::services::radio::orchestrate_song(
                db,
                lastfm.as_ref(),
                Some(cache),
                seed_id,
                crate::services::radio::RadioBlend::Mixed,
                HOME_SUGGESTIONS_PER_SEED,
                seeds_ref,
            )
            .await
            .map_err(|e| (seed_id, e))
        }
    }))
    .await;

    let mut agg: HashMap<i64, HomeSuggestionCandidate> = HashMap::new();
    for result in queues {
        let queue = match result {
            Ok(q) => q,
            Err((seed_id, e)) => {
                tracing::warn!(seed_id, error = %e, "home_suggestions: orchestrate_song failed for seed");
                continue;
            }
        };
        for cand in queue.tracks {
            if !cand.is_in_library || seeds.contains(&cand.track_id) {
                continue;
            }
            let hub_pct = cand.candidate_in_degree_percentile.unwrap_or(0.0);
            let artist_key = cand.artist_name.trim().to_ascii_lowercase();
            agg.entry(cand.track_id)
                .and_modify(|existing| {
                    existing.seed_hits += 1;
                    if cand.similarity_score > existing.score {
                        existing.score = cand.similarity_score;
                    }
                })
                .or_insert(HomeSuggestionCandidate {
                    track_id: cand.track_id,
                    artist_key,
                    score: cand.similarity_score,
                    seed_hits: 1,
                    hub_pct,
                });
        }
    }

    let ranked_ids = merge_home_suggestions(
        agg.into_values().collect(),
        limit,
        HOME_SUGGESTIONS_MAX_PER_ARTIST,
    );

    // Hydrate to full Track rows, then restore rank order (get_tracks_by_ids
    // returns rows in arbitrary order).
    let ids_for_query = ranked_ids.clone();
    let tracks = db
        .with_conn(move |conn| crate::playback::queue::get_tracks_by_ids(conn, &ids_for_query))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut by_id: HashMap<i64, crate::db::models::Track> =
        tracks.into_iter().map(|t| (t.id, t)).collect();
    let ordered: Vec<crate::db::models::Track> = ranked_ids
        .iter()
        .filter_map(|id| by_id.remove(id))
        .collect();

    let payload = json!({ "tracks": ordered });
    write_home_suggestions_cache(&state, &cache_key, &payload).await;
    Ok(Json(payload))
}

fn recommendation_seed_window() -> usize {
    (unix_now_secs() / (6 * 60 * 60)) as usize
}

fn rotate_take<T: Clone>(items: &[T], limit: usize, salt: usize) -> Vec<T> {
    if items.is_empty() || limit == 0 {
        return Vec::new();
    }
    let offset = salt % items.len();
    items
        .iter()
        .cycle()
        .skip(offset)
        .take(limit.min(items.len()))
        .cloned()
        .collect()
}

pub(crate) fn merge_lastfm_track_seeds(
    recent: Vec<LastFmChartTrack>,
    loved: Vec<LastFmChartTrack>,
    top: Vec<LastFmChartTrack>,
    salt: usize,
    limit: usize,
) -> Vec<LastFmTrackSeed> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut push_track = |track: LastFmChartTrack, reason: String| {
        if out.len() >= limit {
            return;
        }
        let key = crate::services::radio::normalize_for_dedup(&track.artist, &track.title);
        if key.is_empty() || !seen.insert(key) {
            return;
        }
        out.push(LastFmTrackSeed {
            artist: track.artist,
            title: track.title,
            reason,
        });
    };

    for track in rotate_take(&recent, LASTFM_HOME_RECENT_SEED_TARGET, salt) {
        let reason = format!("Because you played {} recently", track.title);
        push_track(track, reason);
    }
    for track in rotate_take(&loved, LASTFM_HOME_LOVED_SEED_TARGET, salt + 3) {
        let reason = format!("Because you loved {}", track.title);
        push_track(track, reason);
    }
    for track in rotate_take(&top, LASTFM_HOME_TOP_SEED_TARGET, salt + 7) {
        let reason = format!("Near your top track {}", track.title);
        push_track(track, reason);
    }

    out
}

pub(crate) fn merge_lastfm_artist_seeds(
    track_seeds: &[LastFmTrackSeed],
    top_artists: Vec<LastFmChartArtist>,
    top_albums: Vec<LastFmChartAlbum>,
    salt: usize,
    limit: usize,
) -> Vec<LastFmArtistSeed> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut push_artist = |name: String, reason: String| {
        if out.len() >= limit {
            return;
        }
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return;
        }
        let key = trimmed.to_ascii_lowercase();
        if !seen.insert(key) {
            return;
        }
        out.push(LastFmArtistSeed {
            name: trimmed.to_string(),
            reason,
        });
    };

    for seed in rotate_take(track_seeds, LASTFM_HOME_RECENT_SEED_TARGET, salt) {
        push_artist(seed.artist.clone(), seed.reason.clone());
    }
    for artist in rotate_take(&top_artists, LASTFM_HOME_TOP_SEED_TARGET, salt + 5) {
        let reason = format!("Near your top artist {}", artist.name);
        push_artist(artist.name, reason);
    }
    for album in rotate_take(&top_albums, LASTFM_HOME_TOP_SEED_TARGET, salt + 11) {
        let reason = format!("Because you play albums by {}", album.artist);
        push_artist(album.artist, reason);
    }

    out
}

async fn load_lastfm_track_seeds(client: &LastFmClient, user: &str) -> Vec<LastFmTrackSeed> {
    let recent = client
        .user_recent_tracks(user, LASTFM_HOME_PROFILE_SOURCE_LIMIT)
        .await
        .unwrap_or_default();
    let loved = client
        .user_loved_tracks(user, LASTFM_HOME_PROFILE_SOURCE_LIMIT)
        .await
        .unwrap_or_default();
    let top = client
        .user_top_tracks(user, LASTFM_HOME_PROFILE_SOURCE_LIMIT)
        .await
        .unwrap_or_default();
    merge_lastfm_track_seeds(
        recent,
        loved,
        top,
        recommendation_seed_window(),
        LASTFM_HOME_SEED_LIMIT,
    )
}

async fn load_lastfm_artist_seeds(
    client: &LastFmClient,
    user: &str,
    track_seeds: &[LastFmTrackSeed],
) -> Vec<LastFmArtistSeed> {
    let top_artists = client
        .user_top_artists(user, LASTFM_HOME_PROFILE_SOURCE_LIMIT)
        .await
        .unwrap_or_default();
    let top_albums = client
        .user_top_albums(user, LASTFM_HOME_PROFILE_SOURCE_LIMIT)
        .await
        .unwrap_or_default();
    merge_lastfm_artist_seeds(
        track_seeds,
        top_artists,
        top_albums,
        recommendation_seed_window(),
        LASTFM_HOME_SEED_LIMIT,
    )
}

async fn fetch_lastfm_home_recommendations(state: &SharedState) -> anyhow::Result<Vec<Value>> {
    let (http, db, user) = {
        let s = state.read().await;
        let user = s.db.with_conn(|conn| {
            Ok::<_, anyhow::Error>(
                crate::services::lastfm::auth::load_credentials(conn)?.and_then(|c| c.session_user),
            )
        })?;
        (s.http_client.clone(), s.db.clone(), user)
    };
    let Some(user) = user else {
        return Ok(Vec::new());
    };
    let Some(client) = LastFmClient::load(http, &db) else {
        return Ok(Vec::new());
    };
    let track_seeds = load_lastfm_track_seeds(&client, &user).await;
    let artist_seeds = load_lastfm_artist_seeds(&client, &user, &track_seeds).await;
    let mut out = Vec::new();
    out.extend(fetch_lastfm_track_recommendations(state, &client, &track_seeds).await?);
    out.extend(fetch_lastfm_artist_recommendations(state, &client, &artist_seeds).await?);
    out.extend(fetch_lastfm_album_recommendations(state, &client, &artist_seeds).await?);
    Ok(out)
}

async fn fetch_lastfm_track_recommendations(
    state: &SharedState,
    client: &LastFmClient,
    seeds: &[LastFmTrackSeed],
) -> anyhow::Result<Vec<Value>> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for seed in seeds {
        for similar in client
            .track_get_similar_with_artist_fallback(
                &seed.artist,
                &seed.title,
                LASTFM_HOME_SIMILAR_LIMIT,
            )
            .await
            .unwrap_or_default()
        {
            let key = crate::services::radio::normalize_for_dedup(&similar.artist, &similar.title);
            if key.is_empty() || !seen.insert(key) {
                continue;
            }
            if let Some(item) = resolve_recommendation_item(
                state,
                "lastfm",
                &similar.artist,
                &similar.title,
                None,
                Some(similar.match_score),
                &seed.reason,
            )
            .await
            {
                out.push(item);
            } else {
                out.push(recommendation_placeholder_item(
                    "lastfm",
                    &similar.artist,
                    &similar.title,
                    similar.mbid.as_deref(),
                    Some(similar.match_score),
                    &seed.reason,
                ));
            }
            if out.len() >= LASTFM_HOME_RECOMMENDATION_LIMIT {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

async fn fetch_lastfm_artist_recommendations(
    state: &SharedState,
    client: &LastFmClient,
    seeds: &[LastFmArtistSeed],
) -> anyhow::Result<Vec<Value>> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for seed in seeds {
        for artist in client
            .artist_get_similar(&seed.name, LASTFM_HOME_SIMILAR_LIMIT)
            .await
            .unwrap_or_default()
        {
            let key = artist.name.trim().to_ascii_lowercase();
            if key.is_empty() || !seen.insert(key) {
                continue;
            }
            out.push(
                resolve_recommendation_artist_item(
                    state,
                    "lastfm",
                    &artist.name,
                    artist.mbid.as_deref(),
                    artist.match_score,
                    &seed.reason,
                    artist.image_url.as_deref(),
                )
                .await,
            );
            if out.len() >= LASTFM_HOME_ARTIST_LIMIT {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

async fn fetch_lastfm_album_recommendations(
    state: &SharedState,
    client: &LastFmClient,
    seeds: &[LastFmArtistSeed],
) -> anyhow::Result<Vec<Value>> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for seed in seeds {
        let similar_artists = client
            .artist_get_similar(&seed.name, LASTFM_HOME_ALBUM_SIMILAR_ARTIST_LIMIT)
            .await
            .unwrap_or_default();
        for artist in similar_artists {
            for album in client
                .artist_top_albums(&artist.name, LASTFM_HOME_ALBUMS_PER_ARTIST_LIMIT)
                .await
                .unwrap_or_default()
            {
                let key = crate::services::radio::normalize_for_dedup(&album.artist, &album.title);
                if key.is_empty() || !seen.insert(key) {
                    continue;
                }
                out.push(
                    resolve_recommendation_album_item(
                        state,
                        "lastfm",
                        &album.artist,
                        &album.title,
                        album.mbid.as_deref(),
                        artist
                            .match_score
                            .or_else(|| album.playcount.map(|count| count as f64)),
                        &seed.reason,
                        album.image_url.as_deref(),
                    )
                    .await,
                );
                if out.len() >= LASTFM_HOME_ALBUM_LIMIT {
                    return Ok(out);
                }
            }
        }
    }
    Ok(out)
}

async fn fetch_listenbrainz_home_recommendations(
    state: &SharedState,
) -> anyhow::Result<Vec<Value>> {
    let (http, token, user) = {
        let s = state.read().await;
        let (token, user) = s.db.with_conn(|conn| {
            let token = crate::services::listenbrainz::load_token(conn, &s.master_key)?;
            let user =
                crate::services::listenbrainz::load_credentials(conn)?.and_then(|c| c.user_name);
            Ok::<_, anyhow::Error>((token, user))
        })?;
        (s.http_client.clone(), token, user)
    };
    let Some(user) = user else {
        return Ok(Vec::new());
    };
    let recs =
        crate::services::listenbrainz::user_recommendations(&http, &user, token.as_deref()).await?;
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for rec in recs {
        let key = crate::services::radio::normalize_for_dedup(&rec.artist, &rec.title);
        if key.is_empty() || !seen.insert(key) {
            continue;
        }
        if let Some(item) = resolve_recommendation_item(
            state,
            "listenbrainz",
            &rec.artist,
            &rec.title,
            rec.mbid.as_deref(),
            rec.score,
            "Collaborative filtering",
        )
        .await
        {
            out.push(item);
        } else {
            out.push(recommendation_placeholder_item(
                "listenbrainz",
                &rec.artist,
                &rec.title,
                rec.mbid.as_deref(),
                rec.score,
                "Collaborative filtering",
            ));
        }
        if out.len() >= 12 {
            break;
        }
    }
    Ok(out)
}

async fn resolve_recommendation_item(
    state: &SharedState,
    provider: &str,
    artist: &str,
    title: &str,
    mbid: Option<&str>,
    score: Option<f64>,
    reason: &str,
) -> Option<Value> {
    let s = state.read().await;
    s.db
        .with_conn(|conn| {
            if let Some(mbid_value) = mbid.filter(|v| !v.trim().is_empty()) {
                let by_mbid = conn
                    .query_row(
                        "SELECT t.id, t.tidal_id, t.title, a.name, al.title, t.artwork_url
                           FROM external_track_candidates c
                           JOIN tracks t
                             ON t.id = c.resolved_track_id
                             OR (c.tidal_id IS NOT NULL AND t.tidal_id = c.tidal_id)
                           LEFT JOIN artists a ON a.id = t.artist_id
                           LEFT JOIN albums al ON al.id = t.album_id
                          WHERE c.mbid = ?1
                          ORDER BY (c.resolved_track_id IS NULL), t.is_favorite DESC, t.play_count DESC
                          LIMIT 1",
                        params![mbid_value],
                        |row| {
                            Ok(json!({
                                "provider": provider,
                                "entity_type": "track",
                                "local_track_id": row.get::<_, i64>(0)?,
                                "tidal_id": row.get::<_, Option<i64>>(1)?,
                                "title": row.get::<_, String>(2)?,
                                "artist_name": row.get::<_, Option<String>>(3)?,
                                "album_title": row.get::<_, Option<String>>(4)?,
                                "artwork_url": row.get::<_, Option<String>>(5)?,
                                "mbid": mbid,
                                "score": score,
                                "reason": reason,
                                "playable": true,
                            }))
                        },
                    )
                    .optional()?;
                if by_mbid.is_some() {
                    return Ok::<_, anyhow::Error>(by_mbid);
                }
            }

            conn.query_row(
                "SELECT t.id, t.tidal_id, t.title, a.name, al.title, t.artwork_url
                   FROM tracks t
                   LEFT JOIN artists a ON a.id = t.artist_id
                   LEFT JOIN albums al ON al.id = t.album_id
                  WHERE LOWER(t.title) = LOWER(?1)
                    AND LOWER(COALESCE(a.name, '')) = LOWER(?2)
                  ORDER BY t.is_favorite DESC, t.play_count DESC
                  LIMIT 1",
                params![title, artist],
                |row| {
                    Ok(json!({
                        "provider": provider,
                        "entity_type": "track",
                        "local_track_id": row.get::<_, i64>(0)?,
                        "tidal_id": row.get::<_, Option<i64>>(1)?,
                        "title": row.get::<_, String>(2)?,
                        "artist_name": row.get::<_, Option<String>>(3)?,
                        "album_title": row.get::<_, Option<String>>(4)?,
                        "artwork_url": row.get::<_, Option<String>>(5)?,
                        "mbid": mbid,
                        "score": score,
                        "reason": reason,
                        "playable": true,
                    }))
                },
            )
            .optional()
            .map_err(Into::into)
        })
        .ok()
        .flatten()
}

async fn resolve_recommendation_artist_item(
    state: &SharedState,
    provider: &str,
    artist: &str,
    mbid: Option<&str>,
    score: Option<f64>,
    reason: &str,
    image_url: Option<&str>,
) -> Value {
    let s = state.read().await;
    s.db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT id, tidal_id, name, photo_url
                   FROM artists
                  WHERE LOWER(name) = LOWER(?1)
                  ORDER BY tidal_id IS NULL, id ASC
                  LIMIT 1",
                params![artist],
                |row| {
                    Ok(json!({
                        "provider": provider,
                        "entity_type": "artist",
                        "local_artist_id": row.get::<_, i64>(0)?,
                        "tidal_artist_id": row.get::<_, Option<i64>>(1)?,
                        "local_track_id": null,
                        "tidal_id": null,
                        "title": row.get::<_, String>(2)?,
                        "artist_name": row.get::<_, String>(2)?,
                        "album_title": null,
                        "artwork_url": row.get::<_, Option<String>>(3)?.or_else(|| image_url.map(str::to_string)),
                        "mbid": mbid,
                        "score": score,
                        "reason": reason,
                        "playable": true,
                    }))
                },
            )
            .optional()
            .map_err(Into::into)
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| {
            json!({
                "provider": provider,
                "entity_type": "artist",
                "local_artist_id": null,
                "tidal_artist_id": null,
                "local_track_id": null,
                "tidal_id": null,
                "title": artist,
                "artist_name": artist,
                "album_title": null,
                "artwork_url": image_url,
                "mbid": mbid,
                "score": score,
                "reason": reason,
                "playable": false,
            })
        })
}

async fn resolve_recommendation_album_item(
    state: &SharedState,
    provider: &str,
    artist: &str,
    title: &str,
    mbid: Option<&str>,
    score: Option<f64>,
    reason: &str,
    image_url: Option<&str>,
) -> Value {
    let s = state.read().await;
    s.db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT al.id, al.tidal_id, al.title, a.id, a.tidal_id, a.name, al.artwork_url
                   FROM albums al
                   LEFT JOIN artists a ON a.id = al.artist_id
                  WHERE LOWER(al.title) = LOWER(?1)
                    AND LOWER(COALESCE(a.name, '')) = LOWER(?2)
                  ORDER BY al.tidal_id IS NULL, al.id ASC
                  LIMIT 1",
                params![title, artist],
                |row| {
                    Ok(json!({
                        "provider": provider,
                        "entity_type": "album",
                        "local_album_id": row.get::<_, i64>(0)?,
                        "tidal_album_id": row.get::<_, Option<i64>>(1)?,
                        "local_artist_id": row.get::<_, Option<i64>>(3)?,
                        "tidal_artist_id": row.get::<_, Option<i64>>(4)?,
                        "local_track_id": null,
                        "tidal_id": null,
                        "title": row.get::<_, String>(2)?,
                        "artist_name": row.get::<_, Option<String>>(5)?,
                        "album_title": row.get::<_, String>(2)?,
                        "artwork_url": row.get::<_, Option<String>>(6)?.or_else(|| image_url.map(str::to_string)),
                        "mbid": mbid,
                        "score": score,
                        "reason": reason,
                        "playable": true,
                    }))
                },
            )
            .optional()
            .map_err(Into::into)
        })
        .ok()
        .flatten()
        .unwrap_or_else(|| {
            json!({
                "provider": provider,
                "entity_type": "album",
                "local_album_id": null,
                "tidal_album_id": null,
                "local_artist_id": null,
                "tidal_artist_id": null,
                "local_track_id": null,
                "tidal_id": null,
                "title": title,
                "artist_name": artist,
                "album_title": title,
                "artwork_url": image_url,
                "mbid": mbid,
                "score": score,
                "reason": reason,
                "playable": false,
            })
        })
}

fn recommendation_placeholder_item(
    provider: &str,
    artist: &str,
    title: &str,
    mbid: Option<&str>,
    score: Option<f64>,
    reason: &str,
) -> Value {
    json!({
        "provider": provider,
        "entity_type": "track",
        "local_track_id": null,
        "tidal_id": 0,
        "title": title,
        "artist_name": artist,
        "album_title": null,
        "artwork_url": null,
        "mbid": mbid,
        "score": score,
        "reason": reason,
        "playable": false,
    })
}

fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Get weekly articles from AllMusic RSS
pub(super) async fn get_home_articles(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let aggregator = state.read().await.rss_aggregator.clone();
    let articles = aggregator.get_articles().await;

    Ok(Json(json!({
        "articles": articles,
        "source": "allmusic_rss"
    })))
}

/// Get music industry news from multiple RSS sources
pub(super) async fn get_home_news(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    let aggregator = state.read().await.rss_aggregator.clone();
    let news = aggregator.get_news().await;

    Ok(Json(json!({
        "news": news,
        "sources": ["billboard", "nme", "spin", "pitchfork", "rolling_stone", "consequence", "the_guardian"],
        "source": "aggregated_rss"
    })))
}
