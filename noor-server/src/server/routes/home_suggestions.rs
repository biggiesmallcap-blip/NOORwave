// Home suggestions (library-resolved, cross-artist murals)
//
// Powers the Library home "Suggested tracks / albums" murals. The old client
// path expanded the seeds into "more tracks by the same artists / same albums",
// which produced clone-of-what-you-just-played suggestions. This endpoint runs
// the real radio blend (embedding neighbours + Last.fm similar + same-artist
// fallback) per seed, keeps only library-resolved candidates so the murals can
// still play tracks and open albums, then ranks with a per-artist cap and hub
// suppression so no single artist floods a panel. Cached 6h per seed set.

use crate::SharedState;
use axum::{extract::State, http::StatusCode, response::Json};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

use super::home_routes::{rotate_take, unix_now_secs};

// Seed budget. 3 recent plays keep the panel reactive to today's listening;
// 5 long-term seeds anchor it to the user's actual taste so one genre session
// cannot hijack the whole mural. Both slices are deduped against each other and
// the long-term slice rotates on the 6h salt so the panel refreshes through the
// day rather than freezing on the same five favourites.
const HOME_SUGGESTIONS_RECENT_SEEDS: usize = 3;
const HOME_SUGGESTIONS_LONG_TERM_SEEDS: usize = 5;
const HOME_SUGGESTIONS_SEED_LIMIT: usize =
    HOME_SUGGESTIONS_RECENT_SEEDS + HOME_SUGGESTIONS_LONG_TERM_SEEDS;
const HOME_SUGGESTIONS_PER_SEED: usize = 24;
const HOME_SUGGESTIONS_DEFAULT_LIMIT: usize = 50;
const HOME_SUGGESTIONS_MAX_PER_ARTIST: usize = 2;
// How far back a play disqualifies a track (and its album) from being suggested.
// Named so it can be tuned without restructuring: 30 days is aggressive for a
// small library but right for a large, mostly-unplayed one.
const HOME_SUGGESTIONS_RECENCY_EXCLUSION_DAYS: i64 = 30;

#[derive(Debug, serde::Deserialize)]
pub(crate) struct HomeSuggestionsRequest {
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

/// Track and album ids the user has played inside the recency window. Both are
/// excluded from candidacy: the track because they just heard it, the album
/// because "more from the record you just played" is the exact failure this
/// endpoint exists to avoid.
#[derive(Debug, Default)]
pub(crate) struct RecentPlayExclusions {
    pub(crate) track_ids: HashSet<i64>,
    pub(crate) album_ids: HashSet<i64>,
}

fn recent_play_exclusions(
    conn: &rusqlite::Connection,
    days: i64,
) -> anyhow::Result<RecentPlayExclusions> {
    let cutoff = format!("-{} days", days.max(0));

    let mut track_stmt = conn.prepare(
        "SELECT DISTINCT lh.track_id
         FROM listen_history lh
         WHERE lh.started_at >= datetime('now', ?1)",
    )?;
    let track_ids: HashSet<i64> = track_stmt
        .query_map(params![cutoff], |row| row.get::<_, i64>(0))?
        .collect::<Result<HashSet<_>, _>>()?;

    let mut album_stmt = conn.prepare(
        "SELECT DISTINCT t.album_id
         FROM listen_history lh
         JOIN tracks t ON t.id = lh.track_id
         WHERE lh.started_at >= datetime('now', ?1) AND t.album_id IS NOT NULL",
    )?;
    let album_ids: HashSet<i64> = album_stmt
        .query_map(params![cutoff], |row| row.get::<_, i64>(0))?
        .collect::<Result<HashSet<_>, _>>()?;

    Ok(RecentPlayExclusions {
        track_ids,
        album_ids,
    })
}

/// One representative track per top-listened artist, most-listened artist first.
/// The representative is that artist's most-recently-added track, which biases
/// toward material the user has had least opportunity to wear out.
fn long_term_seed_pool(conn: &rusqlite::Connection, artist_limit: i64) -> anyhow::Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "WITH top_artist AS (
             SELECT t.artist_id AS artist_id, COUNT(lh.id) AS listens
             FROM listen_history lh
             JOIN tracks t ON t.id = lh.track_id
             WHERE t.artist_id IS NOT NULL
             GROUP BY t.artist_id
             ORDER BY listens DESC
             LIMIT ?1
         )
         SELECT (
             SELECT t2.id FROM tracks t2
             WHERE t2.artist_id = ta.artist_id
             ORDER BY t2.id DESC
             LIMIT 1
         ) AS track_id
         FROM top_artist ta
         ORDER BY ta.listens DESC",
    )?;
    let ids = stmt
        .query_map(params![artist_limit.max(1)], |row| {
            row.get::<_, Option<i64>>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    Ok(ids)
}

/// Distinct most-recently-played track ids, newest first.
fn recent_seed_pool(conn: &rusqlite::Connection, limit: i64) -> anyhow::Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT lh.track_id
         FROM listen_history lh
         GROUP BY lh.track_id
         ORDER BY MAX(lh.started_at) DESC
         LIMIT ?1",
    )?;
    let ids = stmt
        .query_map(params![limit.max(1)], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

/// Blend the recent and long-term seed pools into one ordered seed set.
/// Recent seeds come first, newest first, and never rotate. Long-term seeds are
/// rotated by `salt` so successive 6h windows pick different favourites. If
/// either pool underdelivers, the other backfills up to the total budget, so a
/// cold-start user still gets whatever seeds exist. No DB access -> unit-testable.
pub(crate) fn blend_suggestion_seeds(recent: &[i64], long_term: &[i64], salt: usize) -> Vec<i64> {
    let mut seen = HashSet::new();
    let mut out: Vec<i64> = Vec::with_capacity(HOME_SUGGESTIONS_SEED_LIMIT);

    for &id in recent.iter().filter(|&&id| id > 0) {
        if out.len() >= HOME_SUGGESTIONS_RECENT_SEEDS {
            break;
        }
        if seen.insert(id) {
            out.push(id);
        }
    }

    let rotated = rotate_take(long_term, long_term.len(), salt);
    for id in rotated.iter().copied().filter(|&id| id > 0) {
        if out.len() >= HOME_SUGGESTIONS_SEED_LIMIT {
            break;
        }
        if seen.insert(id) {
            out.push(id);
        }
    }

    // Backfill from any recent plays beyond the recent quota when the long-term
    // pool was too thin to reach the budget.
    for &id in recent.iter().filter(|&&id| id > 0) {
        if out.len() >= HOME_SUGGESTIONS_SEED_LIMIT {
            break;
        }
        if seen.insert(id) {
            out.push(id);
        }
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
pub(crate) async fn get_home_suggestions(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, schema};

    /// A fresh, fully-migrated in-memory database. Mirrors the helper in
    /// `routes/tests.rs`: in-memory keeps the suite off the filesystem, where a
    /// temp `.db` + WAL per test churns enough I/O to stall on Defender scans.
    fn fresh_migrated_db() -> Database {
        let db = Database::open_in_memory().expect("db opened");
        db.run_migrations().expect("migrations");
        db.with_conn(|conn| schema::run_migrations(conn))
            .expect("schema migrations");
        db
    }

    #[test]
    fn long_term_seed_pool_takes_one_track_per_top_artist() {
        let db = fresh_migrated_db();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO artists (id, name) VALUES (1, 'Favourite'), (2, 'Occasional')",
                [],
            )?;
            conn.execute(
                "INSERT INTO albums (id, title, artist_id) VALUES (10, 'A', 1), (11, 'B', 2)",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, album_id, file_path) VALUES
                    (100, 'Fav One', 1, 10, '/a1'),
                    (101, 'Fav Two', 1, 10, '/a2'),
                    (102, 'Occ One', 2, 11, '/b1')",
                [],
            )?;
            // Favourite has three listens, Occasional one.
            conn.execute(
                "INSERT INTO listen_history (track_id, started_at) VALUES
                    (100, '2026-01-01T00:00:00Z'),
                    (101, '2026-01-02T00:00:00Z'),
                    (100, '2026-01-03T00:00:00Z'),
                    (102, '2026-01-04T00:00:00Z')",
                [],
            )?;
            Ok(())
        })
        .expect("seed fixture");

        let pool = db
            .with_conn(|conn| long_term_seed_pool(conn, 20))
            .expect("pool");

        // One track per artist, most-listened artist first.
        assert_eq!(pool.len(), 2);
        assert!(
            pool[0] == 100 || pool[0] == 101,
            "first seed is a Favourite track, got {}",
            pool[0]
        );
        assert_eq!(pool[1], 102);
    }

    #[test]
    fn recent_seed_pool_returns_newest_plays_first() {
        let db = fresh_migrated_db();
        db.with_conn(|conn| {
            conn.execute("INSERT INTO artists (id, name) VALUES (1, 'A')", [])?;
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, file_path) VALUES
                    (100, 'One', 1, '/1'), (101, 'Two', 1, '/2')",
                [],
            )?;
            conn.execute(
                "INSERT INTO listen_history (track_id, started_at) VALUES
                    (100, '2026-01-01T00:00:00Z'),
                    (101, '2026-06-01T00:00:00Z')",
                [],
            )?;
            Ok(())
        })
        .expect("seed fixture");

        let pool = db
            .with_conn(|conn| recent_seed_pool(conn, 10))
            .expect("pool");
        assert_eq!(pool, vec![101, 100]);
    }

    #[test]
    fn recent_play_exclusions_cover_tracks_and_their_albums() {
        let db = fresh_migrated_db();
        db.with_conn(|conn| {
            conn.execute("INSERT INTO artists (id, name) VALUES (1, 'A')", [])?;
            conn.execute(
                "INSERT INTO albums (id, title, artist_id) VALUES (10, 'Fresh', 1), (11, 'Stale', 1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, album_id, file_path) VALUES
                    (100, 'Played Recently', 1, 10, '/1'),
                    (101, 'Sibling On Same Album', 1, 10, '/2'),
                    (102, 'Played Long Ago', 1, 11, '/3')",
                [],
            )?;
            conn.execute(
                "INSERT INTO listen_history (track_id, started_at) VALUES
                    (100, datetime('now', '-5 days')),
                    (102, datetime('now', '-60 days'))",
                [],
            )?;
            Ok(())
        })
        .expect("seed fixture");

        let excl = db
            .with_conn(|conn| recent_play_exclusions(conn, 30))
            .expect("exclusions");

        assert!(excl.track_ids.contains(&100), "5-day-old play is excluded");
        assert!(
            !excl.track_ids.contains(&102),
            "60-day-old play is outside the window"
        );
        assert!(
            excl.album_ids.contains(&10),
            "album of the recent play is excluded"
        );
        assert!(
            !excl.album_ids.contains(&11),
            "album of the old play is not excluded"
        );
    }

    fn home_cand(track_id: i64, artist: &str, score: f64) -> HomeSuggestionCandidate {
        HomeSuggestionCandidate {
            track_id,
            artist_key: artist.to_string(),
            score,
            seed_hits: 1,
            hub_pct: 0.0,
        }
    }

    #[test]
    fn home_suggestions_rank_by_score_and_respect_limit() {
        let ranked = merge_home_suggestions(
            vec![
                home_cand(1, "a", 0.2),
                home_cand(2, "b", 0.9),
                home_cand(3, "c", 0.5),
            ],
            2,
            2,
        );
        assert_eq!(ranked, vec![2, 3]);
    }

    #[test]
    fn home_suggestions_cap_prevents_one_artist_flooding_the_panel() {
        // Same artist has the four highest scores; the cap of 2 forces the other
        // artist's lower-scored track into the panel instead of a third clone.
        let ranked = merge_home_suggestions(
            vec![
                home_cand(1, "hoggy", 0.99),
                home_cand(2, "hoggy", 0.98),
                home_cand(3, "hoggy", 0.97),
                home_cand(4, "other", 0.10),
            ],
            3,
            2,
        );
        assert_eq!(ranked, vec![1, 2, 4]);
    }

    #[test]
    fn home_suggestions_consensus_and_hub_shape_the_order() {
        // Base scores are equal; the candidate surfaced by two seeds outranks the
        // single-seed one, and a heavy graph hub is pushed below both.
        let consensus = HomeSuggestionCandidate {
            seed_hits: 2,
            ..home_cand(1, "a", 0.5)
        };
        let single = home_cand(2, "b", 0.5);
        let hub = HomeSuggestionCandidate {
            hub_pct: 1.0,
            ..home_cand(3, "c", 0.5)
        };
        let ranked = merge_home_suggestions(vec![single, hub, consensus], 3, 2);
        assert_eq!(ranked, vec![1, 2, 3]);
    }

    #[test]
    fn seed_blend_takes_three_recent_then_five_long_term() {
        let recent = vec![10, 11, 12, 13, 14, 15];
        let long_term = vec![20, 21, 22, 23, 24, 25, 26];
        let seeds = blend_suggestion_seeds(&recent, &long_term, 0);
        assert_eq!(seeds.len(), 8);
        assert_eq!(&seeds[..3], &[10, 11, 12], "recent slice, newest first");
        assert_eq!(&seeds[3..], &[20, 21, 22, 23, 24]);
    }

    #[test]
    fn seed_blend_rotates_long_term_with_the_salt() {
        let recent = vec![10, 11, 12];
        let long_term = vec![20, 21, 22, 23, 24, 25, 26];
        let a = blend_suggestion_seeds(&recent, &long_term, 0);
        let b = blend_suggestion_seeds(&recent, &long_term, 1);
        assert_eq!(&a[..3], &b[..3], "recent slice does not rotate");
        assert_ne!(&a[3..], &b[3..], "long-term slice rotates with the salt");
    }

    #[test]
    fn seed_blend_dedups_and_backfills_a_short_recent_slice() {
        // Only one recent play, and it is also a top-artist track. The blend must
        // not emit it twice, and must top up from the long-term pool so a user
        // with a thin session still gets a full seed set.
        let recent = vec![20];
        let long_term = vec![20, 21, 22, 23, 24, 25, 26, 27];
        let seeds = blend_suggestion_seeds(&recent, &long_term, 0);
        assert_eq!(seeds.len(), 8);
        assert_eq!(seeds[0], 20);
        let mut sorted = seeds.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 8, "no duplicate seeds");
    }

    #[test]
    fn seed_blend_survives_empty_history() {
        assert!(blend_suggestion_seeds(&[], &[], 0).is_empty());
    }

    #[test]
    fn home_suggestions_empty_artist_key_is_never_capped() {
        // Missing artist names must not collapse into one synthetic bucket and get
        // dropped by the cap.
        let ranked = merge_home_suggestions(
            vec![
                home_cand(1, "", 0.9),
                home_cand(2, "", 0.8),
                home_cand(3, "", 0.7),
            ],
            3,
            2,
        );
        assert_eq!(ranked, vec![1, 2, 3]);
    }
}
